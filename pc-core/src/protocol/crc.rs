//! CRC-16/CCITT-FALSE (`poly = 0x1021`, `init = 0xFFFF`, `xorout = 0x0000`).
//!
//! Used optionally (`PacketFlags::CRC_PRESENT`) on serial / Bluetooth RFCOMM
//! links to detect bit corruption and aid frame resynchronization.

/// Compute CRC-16/CCITT-FALSE over one or more byte slices without copying.
pub fn crc16_ccitt_chunks(chunks: &[&[u8]]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for chunk in chunks {
        for &byte in *chunk {
            crc ^= (byte as u16) << 8;
            for _ in 0..8 {
                if (crc & 0x8000) != 0 {
                    crc = (crc << 1) ^ 0x1021;
                } else {
                    crc <<= 1;
                }
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_check_value() {
        // Standard CRC-16/CCITT-FALSE check value for ASCII "123456789" is 0x29B1.
        assert_eq!(crc16_ccitt_chunks(&[b"123456789"]), 0x29B1);
    }

    #[test]
    fn chunked_matches_contiguous() {
        let a = crc16_ccitt_chunks(&[b"1234", b"56789"]);
        let b = crc16_ccitt_chunks(&[b"123456789"]);
        assert_eq!(a, b);
    }
}
