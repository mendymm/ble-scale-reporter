use std::sync::{mpsc::Sender, Arc, Mutex};
use std::thread;
use std::time::Duration;

use esp_idf_svc::bt::ble::gap::{
    AdvertisingDataType, BleGapEvent, EspBleGap, GapSearchEvent, GapSearchResult, ScanParams,
};
use esp_idf_svc::bt::ble::gatt::client::{
    CharacteristicElement, ConnectionId, DbAttrType, DescriptorElement, EspGattc, GattAuthReq,
    GattCreateConnParams, GattWriteType, GattcEvent,
};
use esp_idf_svc::bt::ble::gatt::{self, GattInterface, GattStatus, Handle, Property};
use esp_idf_svc::bt::{BdAddr, Ble, BtDriver, BtStatus, BtUuid};
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::modem::BluetoothModem;
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::sys::{EspError, ESP_FAIL};
use log::{debug, error, info, warn};

use crate::base64::Base64;
use crate::heartbeat::wait_for_valid_time;
use crate::protocol::{
    BasicGuestProfileCommand, FinalWeightFrame, HundredthsKg, LiveWeightFrame,
    SetDisplayUnitCommand,
};
use crate::status::StatusLight;

const APP_ID: u16 = 0;
const SCALE_COMPANY_ID_LE: [u8; 2] = [0x10, 0x1a];
const SCALE_SERVICE_UUID: BtUuid = BtUuid::uuid16(0x1a10);
const SCALE_ALT_SERVICE_UUID: BtUuid = BtUuid::uuid16(0xfff0);
const NOTIFY_CHARACTERISTIC_UUID: BtUuid = BtUuid::uuid16(0x2a10);
const WRITE_CHARACTERISTIC_UUID: BtUuid = BtUuid::uuid16(0x2a11);
const CCCD_UUID: BtUuid = BtUuid::uuid16(0x2902);
const NVS_NAMESPACE: &str = "scale";
const LAST_WEIGHT_KEY: &str = "last_weight";
const DEFAULT_LAST_WEIGHT: HundredthsKg = HundredthsKg::new(8_000);
const MINIMUM_STORED_WEIGHT: HundredthsKg = HundredthsKg::new(5_000);

type ScaleBtDriver = BtDriver<'static, Ble>;
type ScaleGap = Arc<EspBleGap<'static, Ble, Arc<ScaleBtDriver>>>;
type ScaleGattc = Arc<EspGattc<'static, Ble, Arc<ScaleBtDriver>>>;

#[derive(Default)]
struct State {
    gattc_if: Option<GattInterface>,
    conn_id: Option<ConnectionId>,
    remote_addr: Option<BdAddr>,
    connecting: bool,
    service_ranges: Vec<(Handle, Handle)>,
    notify_handle: Option<Handle>,
    cccd_handle: Option<Handle>,
    write_handle: Option<Handle>,
    profile_sent: bool,
}

#[derive(Clone)]
struct ScaleClient {
    gap: ScaleGap,
    gattc: ScaleGattc,
    state: Arc<Mutex<State>>,
    nvs: Arc<Mutex<EspDefaultNvs>>,
    reporter: Sender<HundredthsKg>,
}

pub fn run(
    bluetooth_modem: BluetoothModem<'static>,
    nvs_partition: EspDefaultNvsPartition,
    reporter: Sender<HundredthsKg>,
    status_light: StatusLight,
) -> anyhow::Result<()> {
    let nvs = EspNvs::new(nvs_partition.clone(), NVS_NAMESPACE, true)?;
    let last_weight = nvs.get_u32(LAST_WEIGHT_KEY)?.map(HundredthsKg::new);
    info!(
        "Previous weight: {}{}",
        last_weight.unwrap_or(DEFAULT_LAST_WEIGHT),
        if last_weight.is_some() {
            " (NVS)"
        } else {
            " (default)"
        }
    );

    let bt = Arc::new(BtDriver::new(bluetooth_modem, Some(nvs_partition))?);
    let client = ScaleClient {
        gap: Arc::new(EspBleGap::new(bt.clone())?),
        gattc: Arc::new(EspGattc::new(bt)?),
        state: Arc::new(Mutex::new(State::default())),
        nvs: Arc::new(Mutex::new(nvs)),
        reporter,
    };

    let gap_client = client.clone();
    client.gap.subscribe(move |event| {
        if let Err(err) = gap_client.on_gap_event(event) {
            error!("GAP error: {err:?}");
        }
    })?;

    let gatt_client = client.clone();
    client.gattc.subscribe(move |(gatt_if, event)| {
        if let Err(err) = gatt_client.on_gatt_event(gatt_if, event) {
            error!("GATT error: {err:?}");
        }
    })?;

    client.gattc.register_app(APP_ID)?;
    gatt::set_local_mtu(256)?;
    info!("BLE initialized; wake the scale by stepping on it");
    wait_for_valid_time();
    status_light.ready();

    loop {
        FreeRtos::delay_ms(10_000);
    }
}

impl ScaleClient {
    fn on_gap_event(&self, event: BleGapEvent) -> Result<(), EspError> {
        match event {
            BleGapEvent::ScanParameterConfigured(status) => {
                check_bt_status(status)?;
                self.gap.start_scanning(0)?;
                info!("Scanning for scale advertisements");
            }
            BleGapEvent::ScanResult(GapSearchEvent::InquiryResult(GapSearchResult {
                bda,
                ble_addr_type,
                rssi,
                ble_adv,
                ..
            })) => {
                let manufacturer_data = ble_adv.and_then(|data| {
                    self.gap
                        .resolve_adv_data_by_type(data, AdvertisingDataType::ManufacturerSpecific)
                });

                if let Some(data) = manufacturer_data
                    .filter(|data| data.len() >= 2 && data[..2] == SCALE_COMPANY_ID_LE)
                {
                    info!("Found scale candidate {bda}, RSSI {rssi}");
                    debug!("advertisement_base64={}", Base64(data));

                    let mut state = self.state.lock().unwrap();
                    if !state.connecting {
                        state.connecting = true;
                        self.gap.stop_scanning()?;
                        info!("Connecting to scale {bda}");
                        self.gattc.enh_open(
                            state.gattc_if.unwrap(),
                            &GattCreateConnParams::new(bda, ble_addr_type),
                        )?;
                    }
                }

                // Large bursts of scan callbacks can otherwise starve the watchdog.
                thread::sleep(Duration::from_millis(10));
            }
            BleGapEvent::ScanStarted(status) | BleGapEvent::ScanStopped(status) => {
                check_bt_status(status)?;
            }
            _ => {}
        }

        Ok(())
    }

    fn on_gatt_event(&self, gatt_if: GattInterface, event: GattcEvent) -> Result<(), EspError> {
        match event {
            GattcEvent::ClientRegistered { status, app_id } => {
                check_gatt_status(status)?;
                if app_id == APP_ID {
                    self.state.lock().unwrap().gattc_if = Some(gatt_if);
                    self.start_scan()?;
                }
            }
            GattcEvent::Connected { conn_id, addr, .. } => {
                let mut state = self.state.lock().unwrap();
                state.conn_id = Some(conn_id);
                state.remote_addr = Some(addr);
                drop(state);
                info!("Connected to {addr}");
                self.gattc.mtu_req(gatt_if, conn_id)?;
            }
            GattcEvent::Open {
                status, addr, mtu, ..
            } => {
                check_gatt_status(status)?;
                info!("GATT opened for {addr}, MTU {mtu}");
            }
            GattcEvent::DiscoveryCompleted { status, conn_id } => {
                check_gatt_status(status)?;
                self.gattc.search_service(gatt_if, conn_id, None)?;
            }
            GattcEvent::SearchResult {
                start_handle,
                end_handle,
                srvc_id,
                ..
            } => {
                if srvc_id.uuid == SCALE_SERVICE_UUID || srvc_id.uuid == SCALE_ALT_SERVICE_UUID {
                    info!("Found scale service {:?}", srvc_id.uuid);
                    self.state
                        .lock()
                        .unwrap()
                        .service_ranges
                        .push((start_handle, end_handle));
                }
            }
            GattcEvent::SearchComplete {
                status, conn_id, ..
            } => {
                check_gatt_status(status)?;
                self.find_scale_characteristics(gatt_if, conn_id)?;
            }
            GattcEvent::RegisterNotify { status, handle } => {
                check_gatt_status(status)?;
                self.find_and_enable_cccd(gatt_if, handle)?;
            }
            GattcEvent::WriteDescriptor { status, .. } => {
                check_gatt_status(status)?;
                info!("Notifications enabled; selecting kilograms");
                self.write_display_unit(gatt_if)?;
            }
            GattcEvent::Notify {
                addr,
                handle,
                value,
                is_notify,
                ..
            } => {
                if let Ok(frame) = FinalWeightFrame::decode(value) {
                    self.handle_final_weight(frame.weight)?;
                } else if let Ok(frame) = LiveWeightFrame::decode(value) {
                    debug!("Live weight: {}", frame.weight);
                }
                debug!(
                    "rx addr={addr} handle={handle} notify={is_notify} base64={}",
                    Base64(value)
                );
            }
            GattcEvent::WriteCharacteristic { status, .. } => {
                check_gatt_status(status)?;
                let profile_sent = self.state.lock().unwrap().profile_sent;
                if profile_sent {
                    info!("Basic guest profile sent; waiting for scale messages");
                } else {
                    info!("Kilograms selected; sending basic guest profile");
                    self.write_guest_profile(gatt_if)?;
                }
            }
            GattcEvent::Disconnected { addr, reason, .. } => {
                info!("Scale {addr} disconnected: {reason:?}");
                *self.state.lock().unwrap() = State {
                    gattc_if: Some(gatt_if),
                    ..State::default()
                };
                self.start_scan()?;
            }
            _ => {}
        }

        Ok(())
    }

    fn start_scan(&self) -> Result<(), EspError> {
        self.gap.set_scan_params(&ScanParams {
            scan_interval: 0x50,
            ..Default::default()
        })
    }

    fn find_scale_characteristics(
        &self,
        gatt_if: GattInterface,
        conn_id: ConnectionId,
    ) -> Result<(), EspError> {
        let ranges = self.state.lock().unwrap().service_ranges.clone();

        for (start_handle, end_handle) in ranges {
            let mut chars = [CharacteristicElement::new(); 1];
            if self
                .gattc
                .get_characteristic_by_uuid(
                    gatt_if,
                    conn_id,
                    start_handle,
                    end_handle,
                    &NOTIFY_CHARACTERISTIC_UUID,
                    &mut chars,
                )
                .unwrap_or(0)
                > 0
            {
                let characteristic = &chars[0];
                if characteristic.properties().contains(Property::Notify) {
                    let mut state = self.state.lock().unwrap();
                    state.notify_handle = Some(characteristic.handle());
                    let addr = state.remote_addr.unwrap();
                    drop(state);
                    self.gattc
                        .register_for_notify(gatt_if, &addr, characteristic.handle())?;
                }
            }

            if self
                .gattc
                .get_characteristic_by_uuid(
                    gatt_if,
                    conn_id,
                    start_handle,
                    end_handle,
                    &WRITE_CHARACTERISTIC_UUID,
                    &mut chars,
                )
                .unwrap_or(0)
                > 0
            {
                self.state.lock().unwrap().write_handle = Some(chars[0].handle());
            }
        }

        let state = self.state.lock().unwrap();
        if state.notify_handle.is_none() || state.write_handle.is_none() {
            error!("Scale service found, but required 0x2a10/0x2a11 characteristics are missing");
        }

        Ok(())
    }

    fn find_and_enable_cccd(
        &self,
        gatt_if: GattInterface,
        notify_handle: Handle,
    ) -> Result<(), EspError> {
        let mut state = self.state.lock().unwrap();
        let conn_id = state.conn_id.unwrap();
        let count = self
            .gattc
            .get_attr_count(
                gatt_if,
                conn_id,
                DbAttrType::Descriptor {
                    handle: notify_handle,
                },
            )
            .map_err(|_| EspError::from_infallible::<ESP_FAIL>())?;

        if count == 0 {
            error!("Notification characteristic has no CCCD");
            return Ok(());
        }

        let mut descriptors = [DescriptorElement::new(); 1];
        let found = self
            .gattc
            .get_descriptor_by_char_handle(
                gatt_if,
                conn_id,
                notify_handle,
                &CCCD_UUID,
                &mut descriptors,
            )
            .map_err(|_| EspError::from_infallible::<ESP_FAIL>())?;

        if found == 0 {
            error!("CCCD 0x2902 was not found");
            return Ok(());
        }

        let cccd_handle = descriptors[0].handle();
        state.cccd_handle = Some(cccd_handle);
        drop(state);
        self.gattc.write_descriptor(
            gatt_if,
            conn_id,
            cccd_handle,
            &1_u16.to_le_bytes(),
            GattWriteType::RequireResponse,
            GattAuthReq::None,
        )
    }

    fn write_display_unit(&self, gatt_if: GattInterface) -> Result<(), EspError> {
        let state = self.state.lock().unwrap();
        let (Some(conn_id), Some(write_handle)) = (state.conn_id, state.write_handle) else {
            warn!("Cannot select display unit: write characteristic is unavailable");
            return Ok(());
        };

        let command: Vec<u8> = SetDisplayUnitCommand::kilograms()
            .try_into()
            .map_err(|err| {
                error!("Failed to encode display-unit command: {err}");
                EspError::from_infallible::<ESP_FAIL>()
            })?;

        self.gattc.write_characteristic(
            gatt_if,
            conn_id,
            write_handle,
            &command,
            GattWriteType::RequireResponse,
            GattAuthReq::None,
        )
    }

    fn write_guest_profile(&self, gatt_if: GattInterface) -> Result<(), EspError> {
        let last_weight = self
            .nvs
            .lock()
            .unwrap()
            .get_u32(LAST_WEIGHT_KEY)?
            .map(HundredthsKg::new)
            .unwrap_or(DEFAULT_LAST_WEIGHT);
        let command = BasicGuestProfileCommand::new(last_weight)
            .to_bytes()
            .map_err(|err| {
                error!("Failed to encode basic guest profile: {err}");
                EspError::from_infallible::<ESP_FAIL>()
            })?;

        let mut state = self.state.lock().unwrap();
        let (Some(conn_id), Some(write_handle)) = (state.conn_id, state.write_handle) else {
            warn!("Cannot send guest profile: write characteristic is unavailable");
            return Ok(());
        };
        state.profile_sent = true;
        drop(state);

        info!("Guest profile previous weight: {last_weight}");
        self.gattc.write_characteristic(
            gatt_if,
            conn_id,
            write_handle,
            &command,
            GattWriteType::RequireResponse,
            GattAuthReq::None,
        )
    }

    fn handle_final_weight(&self, weight: HundredthsKg) -> Result<(), EspError> {
        info!("Authoritative weight: {weight}");

        if self.reporter.send(weight).is_err() {
            warn!("Could not queue authoritative weight report");
        }

        if weight.hundredths() <= MINIMUM_STORED_WEIGHT.hundredths() {
            info!("Not storing authoritative weight at or below {MINIMUM_STORED_WEIGHT}");
            return Ok(());
        }

        let nvs = self.nvs.lock().unwrap();
        if nvs.get_u32(LAST_WEIGHT_KEY)? != Some(weight.hundredths()) {
            nvs.set_u32(LAST_WEIGHT_KEY, weight.hundredths())?;
            info!("Stored authoritative weight for the next session");
        } else {
            info!("Authoritative weight matches NVS; not overwriting it");
        }

        Ok(())
    }
}

fn check_bt_status(status: BtStatus) -> Result<(), EspError> {
    if status == BtStatus::Success {
        Ok(())
    } else {
        warn!("Bluetooth status: {status:?}");
        Err(EspError::from_infallible::<ESP_FAIL>())
    }
}

fn check_gatt_status(status: GattStatus) -> Result<(), EspError> {
    if status == GattStatus::Ok {
        Ok(())
    } else {
        warn!("GATT status: {status:?}");
        Err(EspError::from_infallible::<ESP_FAIL>())
    }
}
