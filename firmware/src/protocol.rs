use deku::{DekuRead, DekuWrite};
use std::fmt;

/// An exact fixed-point weight as transmitted by the scale.
#[derive(Clone, Copy, Eq, PartialEq, DekuRead, DekuWrite)]
#[deku(endian = "big")]
pub struct HundredthsKg(u32);

impl HundredthsKg {
    pub const fn new(hundredths: u32) -> Self {
        Self(hundredths)
    }

    pub const fn hundredths(self) -> u32 {
        self.0
    }
}

impl fmt::Display for HundredthsKg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{:02} kg", self.0 / 100, self.0 % 100)
    }
}

impl fmt::Debug for HundredthsKg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HundredthsKg")
            .field("hundredths", &self.0)
            .field("kg", &format_args!("{self}"))
            .finish()
    }
}

/// Command `0x90`: select kilograms without changing the scale's stored mode.
#[derive(Debug, Clone, Copy, Eq, PartialEq, DekuRead, DekuWrite)]
#[deku(magic = b"\x55\xaa")]
pub struct SetDisplayUnitCommand {
    #[deku(assert_eq = "0x90")]
    command: u8,
    #[deku(assert_eq = "0")]
    payload_length_high: u8,
    #[deku(assert_eq = "4")]
    payload_length_low: u8,
    unit_kg: u8,
    reserved: u8,
    keep_stored_mode: u8,
    reserved_tail: u8,
    check_byte: u8,
}

impl SetDisplayUnitCommand {
    pub const fn kilograms() -> Self {
        Self {
            command: 0x90,
            payload_length_high: 0,
            payload_length_low: 4,
            unit_kg: 1,
            reserved: 0,
            keep_stored_mode: 0,
            reserved_tail: 0,
            check_byte: 0x94,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, DekuWrite)]
struct BasicGuestProfilePayload {
    sex_and_slot: u8,
    #[deku(endian = "big")]
    birth_year: u16,
    birth_month: u8,
    birth_day: u8,
    #[deku(endian = "big")]
    height_tenths_cm: u16,
    #[deku(endian = "big")]
    last_weight_hundredths_kg: u32,
    flags: u8,
    guest_marker: u8,
    no_impedance: u8,
}

/// Command `0x96`: an ephemeral guest profile requesting weight only.
pub struct BasicGuestProfileCommand(BasicGuestProfilePayload);

impl BasicGuestProfileCommand {
    pub const fn new(last_weight: HundredthsKg) -> Self {
        Self(BasicGuestProfilePayload {
            // Male in bits 5:4 and guest slot 9 in bits 3:0.
            sex_and_slot: 0x19,
            birth_year: 2000,
            birth_month: 1,
            birth_day: 1,
            height_tenths_cm: 1_700,
            last_weight_hundredths_kg: last_weight.0,
            // Non-athlete, algorithm 3. These are structural placeholders when
            // the no-impedance trailer below is used.
            flags: 0xa9,
            guest_marker: 0xff,
            no_impedance: 0x06,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, deku::DekuError> {
        let payload: Vec<u8> = self.0.try_into()?;
        let mut frame = Vec::with_capacity(20);
        frame.extend_from_slice(&[0x55, 0xaa, 0x96, 0x00, 0x0e]);
        frame.extend_from_slice(&payload);
        frame.push(checksum(&frame));
        Ok(frame)
    }
}

/// A `0x14` notification containing the scale's current live weight.
#[derive(Debug, Clone, Copy, Eq, PartialEq, DekuRead)]
#[deku(magic = b"\x55\xaa")]
pub struct LiveWeightFrame {
    #[deku(assert_eq = "0x14")]
    command: u8,
    #[deku(endian = "big", assert_eq = "7")]
    payload_length: u16,
    status: u8,
    pub weight: HundredthsKg,
    #[deku(endian = "big")]
    resistance_ohms: u16,
    check_byte: u8,
}

impl LiveWeightFrame {
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        validate_frame(bytes, 0x14, 7)?;
        Self::try_from(bytes).map_err(FrameError::Decode)
    }
}

/// An authoritative `0x18` final report from an extended-protocol scale.
#[derive(Debug, Clone, Copy, Eq, PartialEq, DekuRead)]
#[deku(magic = b"\x55\xaa")]
pub struct FinalWeightFrame {
    #[deku(assert_eq = "0x18")]
    command: u8,
    #[deku(endian = "big", assert_eq = "13")]
    payload_length: u16,
    status: u8,
    reserved: u8,
    guest_slot: u8,
    pub weight: HundredthsKg,
    #[deku(endian = "big")]
    resistance_ohms: u16,
    #[deku(endian = "big")]
    bmi_tenths: u16,
    #[deku(endian = "big")]
    body_fat_tenths_percent: u16,
    check_byte: u8,
}

impl FinalWeightFrame {
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        validate_frame(bytes, 0x18, 13)?;
        let frame = Self::try_from(bytes).map_err(FrameError::Decode)?;
        if frame.status != 1 {
            return Err(FrameError::UnexpectedStatus(frame.status));
        }
        Ok(frame)
    }
}

#[derive(Debug)]
pub enum FrameError {
    WrongLength { expected: usize, actual: usize },
    WrongHeader,
    WrongCommand { expected: u8, actual: u8 },
    WrongPayloadLength { expected: u16, actual: u16 },
    BadChecksum { expected: u8, actual: u8 },
    UnexpectedStatus(u8),
    Decode(deku::DekuError),
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLength { expected, actual } => {
                write!(
                    formatter,
                    "wrong frame length: expected {expected}, got {actual}"
                )
            }
            Self::WrongHeader => formatter.write_str("wrong frame header"),
            Self::WrongCommand { expected, actual } => write!(
                formatter,
                "wrong command: expected 0x{expected:02x}, got 0x{actual:02x}"
            ),
            Self::WrongPayloadLength { expected, actual } => write!(
                formatter,
                "wrong payload length: expected {expected}, got {actual}"
            ),
            Self::BadChecksum { expected, actual } => write!(
                formatter,
                "bad checksum: expected 0x{expected:02x}, got 0x{actual:02x}"
            ),
            Self::UnexpectedStatus(status) => {
                write!(formatter, "unexpected final-frame status 0x{status:02x}")
            }
            Self::Decode(error) => write!(formatter, "cannot decode frame: {error}"),
        }
    }
}

impl std::error::Error for FrameError {}

fn validate_frame(
    bytes: &[u8],
    expected_command: u8,
    expected_payload: u16,
) -> Result<(), FrameError> {
    let expected_len = 6 + usize::from(expected_payload);
    if bytes.len() != expected_len {
        return Err(FrameError::WrongLength {
            expected: expected_len,
            actual: bytes.len(),
        });
    }
    if bytes[..2] != [0x55, 0xaa] {
        return Err(FrameError::WrongHeader);
    }
    if bytes[2] != expected_command {
        return Err(FrameError::WrongCommand {
            expected: expected_command,
            actual: bytes[2],
        });
    }
    let payload_length = u16::from_be_bytes([bytes[3], bytes[4]]);
    if payload_length != expected_payload {
        return Err(FrameError::WrongPayloadLength {
            expected: expected_payload,
            actual: payload_length,
        });
    }
    let actual = bytes[bytes.len() - 1];
    let expected = checksum(&bytes[..bytes.len() - 1]);
    if actual != expected {
        return Err(FrameError::BadChecksum { expected, actual });
    }
    Ok(())
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_observed_live_weight() {
        let bytes = [
            0x55, 0xaa, 0x14, 0x00, 0x07, 0x00, 0x00, 0x00, 0x1f, 0x94, 0x00, 0x00, 0xc9,
        ];

        let frame = LiveWeightFrame::decode(&bytes).unwrap();

        assert_eq!(frame.weight, HundredthsKg(8_084));
        assert_eq!(frame.weight.to_string(), "80.84 kg");
        assert_eq!(
            format!("{:?}", frame.weight),
            "HundredthsKg { hundredths: 8084, kg: 80.84 kg }"
        );
    }

    #[test]
    fn decodes_observed_authoritative_weight() {
        let bytes = [
            0x55, 0xaa, 0x18, 0x00, 0x0d, 0x01, 0x01, 0x09, 0x00, 0x00, 0x1f, 0x95, 0x00, 0x00,
            0x01, 0x20, 0x00, 0x00, 0x04,
        ];

        let frame = FinalWeightFrame::decode(&bytes).unwrap();

        assert_eq!(frame.weight, HundredthsKg(8_085));
    }

    #[test]
    fn serializes_set_display_unit_command() {
        let bytes: Vec<u8> = SetDisplayUnitCommand::kilograms().try_into().unwrap();

        assert_eq!(
            bytes,
            [0x55, 0xaa, 0x90, 0x00, 0x04, 0x01, 0x00, 0x00, 0x00, 0x94]
        );
    }

    #[test]
    fn serializes_basic_guest_profile() {
        let command = BasicGuestProfileCommand::new(HundredthsKg::new(8_000));
        let bytes = command.to_bytes().unwrap();

        assert_eq!(bytes.len(), 20);
        assert_eq!(&bytes[..5], &[0x55, 0xaa, 0x96, 0x00, 0x0e]);
        assert_eq!(bytes[19], checksum(&bytes[..19]));
        assert_eq!(&bytes[12..16], &[0x00, 0x00, 0x1f, 0x40]);
        assert_eq!(bytes[18], 0x06);
    }

    #[test]
    fn rejects_a_bad_checksum() {
        let bytes = [
            0x55, 0xaa, 0x14, 0x00, 0x07, 0x00, 0x00, 0x00, 0x1f, 0x94, 0x00, 0x00, 0x00,
        ];

        assert!(matches!(
            LiveWeightFrame::decode(&bytes),
            Err(FrameError::BadChecksum { .. })
        ));
    }
}
