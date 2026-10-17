use std::fmt;
use std::fmt::Write;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Formats bytes as standard padded Base64 without allocating.
pub struct Base64<'a>(pub &'a [u8]);

impl fmt::Display for Base64<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut chunks = self.0.chunks_exact(3);

        for chunk in &mut chunks {
            let bits =
                (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
            write_index(formatter, bits >> 18)?;
            write_index(formatter, bits >> 12)?;
            write_index(formatter, bits >> 6)?;
            write_index(formatter, bits)?;
        }

        match chunks.remainder() {
            [] => {}
            [first] => {
                let bits = u32::from(*first) << 16;
                write_index(formatter, bits >> 18)?;
                write_index(formatter, bits >> 12)?;
                formatter.write_str("==")?;
            }
            [first, second] => {
                let bits = (u32::from(*first) << 16) | (u32::from(*second) << 8);
                write_index(formatter, bits >> 18)?;
                write_index(formatter, bits >> 12)?;
                write_index(formatter, bits >> 6)?;
                formatter.write_char('=')?;
            }
            _ => unreachable!(),
        }

        Ok(())
    }
}

fn write_index(formatter: &mut fmt::Formatter<'_>, bits: u32) -> fmt::Result {
    formatter.write_char(char::from(ALPHABET[(bits & 0x3f) as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_standard_test_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foobar", "Zm9vYmFy"),
        ];

        for (input, expected) in cases {
            assert_eq!(Base64(input).to_string(), *expected);
        }
    }

    #[test]
    fn encodes_captured_scale_frame() {
        let frame = [
            0x55, 0xaa, 0x14, 0x00, 0x07, 0x00, 0x00, 0x00, 0x1f, 0x95, 0x00, 0x00, 0xce,
        ];

        assert_eq!(Base64(&frame).to_string(), "VaoUAAcAAAAflQAAzg==");
    }
}
