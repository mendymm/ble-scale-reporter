use deku::{DekuRead, DekuWrite};
use std::fmt;

/// An exact fixed-point weight as transmitted by the scale.
#[derive(Clone, Copy, Eq, PartialEq, DekuRead, DekuWrite)]
#[deku(endian = "big")]
pub struct HundredthsKg(u16);

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

/// Command sent after subscribing to the scale's notification characteristic.
#[derive(Debug, Clone, Copy, Eq, PartialEq, DekuRead, DekuWrite)]
#[deku(magic = b"\x55\xaa")]
pub struct UnlockCommand {
    #[deku(assert_eq = "0x90")]
    command: u8,
    #[deku(assert_eq = "0")]
    reserved: u8,
    #[deku(assert_eq = "4")]
    payload_length: u8,
    enabled: u8,
    reserved_payload: [u8; 3],
    check_byte: u8,
}

impl UnlockCommand {
    pub const fn new() -> Self {
        Self {
            command: 0x90,
            reserved: 0,
            payload_length: 4,
            enabled: 1,
            reserved_payload: [0; 3],
            check_byte: 0x94,
        }
    }
}

/// A `0x14` notification containing the scale's current live weight.
#[derive(Debug, Clone, Copy, Eq, PartialEq, DekuRead, DekuWrite)]
#[deku(magic = b"\x55\xaa")]
pub struct LiveWeightFrame {
    #[deku(assert_eq = "0x14")]
    command: u8,
    reserved: u8,
    #[deku(assert_eq = "7")]
    payload_length: u8,
    status: [u8; 3],
    pub weight_hundredths_kg: HundredthsKg,
    #[deku(endian = "big")]
    auxiliary: u16,
    check_byte: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_observed_live_weight() {
        let bytes = [
            0x55, 0xaa, 0x14, 0x00, 0x07, 0x00, 0x00, 0x00, 0x1f, 0x94, 0x00, 0x00, 0xc9,
        ];

        let frame = LiveWeightFrame::try_from(bytes.as_slice()).unwrap();

        assert_eq!(frame.weight_hundredths_kg, HundredthsKg(8_084));
        assert_eq!(frame.weight_hundredths_kg.to_string(), "80.84 kg");
        assert_eq!(
            format!("{:?}", frame.weight_hundredths_kg),
            "HundredthsKg { hundredths: 8084, kg: 80.84 kg }"
        );
    }

    #[test]
    fn serializes_unlock_command() {
        let bytes: Vec<u8> = UnlockCommand::new().try_into().unwrap();

        assert_eq!(
            bytes,
            [0x55, 0xaa, 0x90, 0x00, 0x04, 0x01, 0x00, 0x00, 0x00, 0x94]
        );
    }
}
