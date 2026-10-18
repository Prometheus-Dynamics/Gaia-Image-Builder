//! Hex and SHA-256 helpers shared by the crates that record digests.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Read buffer for hashing a file in place: large enough to keep the reads
/// few, small enough that many files hash without a large allocation each.
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lowercase hex SHA-256 of a file's contents, hashed in-process as it is
/// read (the same digest `sha256sum` prints). The file is never held in
/// memory whole.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
    }

    #[test]
    fn sha256_file_matches_the_in_memory_digest_across_buffers() {
        let dir = TempDir::new("digest");
        let path = dir.join("big");
        // Larger than one read buffer, so the loop reads more than once.
        let bytes: Vec<u8> = (0..HASH_BUFFER_BYTES * 2 + 17)
            .map(|index| (index % 251) as u8)
            .collect();
        std::fs::write(&path, &bytes).expect("write");
        assert_eq!(sha256_file(&path).expect("hash"), sha256_hex_bytes(&bytes));
    }

    #[test]
    fn sha256_file_of_empty_file_is_the_empty_digest() {
        let dir = TempDir::new("digest");
        let path = dir.join("empty");
        std::fs::write(&path, b"").expect("write");
        assert_eq!(
            sha256_file(&path).expect("hash"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_file_reports_a_missing_file() {
        let dir = TempDir::new("digest");
        assert!(sha256_file(&dir.join("missing")).is_err());
    }
}
