//! CRC-32 (IEEE 802.3, reflected, polynomial `0xedb88320`), the checksum of
//! GPT headers and, without its final inversion, of every UBI header.

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

/// Continues a reflected CRC-32 register from `crc`, with no inversions.
/// `update(!0, bytes)` is [`ubi_crc`], and feeding a stream in pieces gives
/// the same value as feeding it whole.
pub fn update(crc: u32, bytes: &[u8]) -> u32 {
    bytes.iter().fold(crc, |crc, byte| {
        TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8)
    })
}

/// The standard CRC-32 (zlib, PNG, GPT).
pub fn crc32(bytes: &[u8]) -> u32 {
    !update(!0, bytes)
}

/// UBI's CRC: the kernel's `crc32_le(UBI_CRC32_INIT, ...)`, the standard
/// CRC-32 without its final inversion.
pub fn ubi_crc(bytes: &[u8]) -> u32 {
    update(!0, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_values() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(ubi_crc(b"123456789"), 0x340b_c6d9);
        // An empty volume-table record's CRC, in every UBI layout volume.
        assert_eq!(ubi_crc(&[0u8; 168]), 0xf116_c36b);
        let streamed = update(update(!0, b"1234"), b"56789");
        assert_eq!(streamed, ubi_crc(b"123456789"));
    }
}
