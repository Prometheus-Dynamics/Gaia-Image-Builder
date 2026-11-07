//! The veritysetup v1 superblock: hand-built bytes, a superblock
//! veritysetup 2.8.7 wrote (`fixtures/`), and a fresh one when a
//! veritysetup is at hand (`GAIA_TEST_VERITYSETUP`, else PATH).

use gaia_formats::verity::{
    Derived, Mismatch, Recorded, SUPERBLOCK_SIZE, Superblock, SuperblockError,
    appended_hash_offset, check, padded_len, parse_uuid,
};
use std::path::PathBuf;
use std::process::Command;

/// The fixture's data: 3 blocks of `(i * 7 + 3) % 256`.
fn fixture_data() -> Vec<u8> {
    (0..3 * 4096u32)
        .map(|i| ((i * 7 + 3) % 256) as u8)
        .collect()
}

const FIXTURE_SHA256: &str = "6657325332983e755558feb56e9f1339e61b3be8dd718b2f0d63774fdb57d5d3";
/// Gaia's exec derived this UUID for the fixture's data (before the
/// derivation moved here) and passed it to `veritysetup format --uuid`.
const FIXTURE_UUID: &str = "07871de1-7623-40cf-922f-478dff950674";

fn fixture_recorded() -> Recorded {
    Recorded {
        salt: FIXTURE_SHA256.into(),
        uuid: FIXTURE_UUID.into(),
        data_blocks: 3,
        data_block_size: 4096,
        hash_block_size: 4096,
    }
}

#[test]
fn the_derivation_is_gaias() {
    let derived = Derived::from_data(&fixture_data());
    assert_eq!(derived.salt, FIXTURE_SHA256);
    assert_eq!(derived.uuid, FIXTURE_UUID);
    assert_eq!(
        Derived::from_sha256_hex(&FIXTURE_SHA256.to_ascii_uppercase()),
        derived
    );
}

#[test]
fn a_hand_built_superblock() {
    let mut bytes = [0u8; SUPERBLOCK_SIZE];
    bytes[..8].copy_from_slice(b"verity\0\0");
    bytes[8] = 1; // version
    bytes[12] = 1; // hash type
    bytes[16..32].copy_from_slice(&[0x11; 16]);
    bytes[32..38].copy_from_slice(b"sha256");
    bytes[64..68].copy_from_slice(&4096u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&512u32.to_le_bytes());
    bytes[72..80].copy_from_slice(&768u64.to_le_bytes());
    bytes[80] = 4; // salt size
    bytes[88..92].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

    let superblock = Superblock::decode(&bytes).unwrap();
    assert_eq!(
        superblock,
        Superblock {
            hash_type: 1,
            uuid: [0x11; 16],
            algorithm: "sha256".into(),
            data_block_size: 4096,
            hash_block_size: 512,
            data_blocks: 768,
            salt: vec![0xde, 0xad, 0xbe, 0xef],
        }
    );
    assert_eq!(superblock.salt_hex(), "deadbeef");
    assert_eq!(
        superblock.uuid_string(),
        "11111111-1111-1111-1111-111111111111"
    );
    assert_eq!(superblock.data_sectors(), 6144);
    assert_eq!(superblock.encode().unwrap(), bytes);

    let mut bad = bytes;
    bad[0] = b'V';
    assert!(matches!(
        Superblock::decode(&bad),
        Err(SuperblockError::Signature(_))
    ));
    let mut bad = bytes;
    bad[8] = 2;
    assert_eq!(Superblock::decode(&bad), Err(SuperblockError::Version(2)));
    let mut bad = bytes;
    bad[81] = 1; // salt size 260
    assert_eq!(
        Superblock::decode(&bad),
        Err(SuperblockError::SaltSize(260))
    );
    let mut bad = bytes;
    bad[64..68].copy_from_slice(&1000u32.to_le_bytes());
    assert!(matches!(
        Superblock::decode(&bad),
        Err(SuperblockError::BlockSize { .. })
    ));
    assert_eq!(
        Superblock::decode(&bytes[..100]),
        Err(SuperblockError::TooShort(100))
    );
}

#[test]
fn veritysetups_superblock_matches_what_gaia_records() {
    let bytes = include_bytes!("fixtures/veritysetup-2.8.7-superblock.bin");
    let superblock = Superblock::decode(bytes).unwrap();
    assert_eq!(check(&superblock, &fixture_recorded()), []);
    assert_eq!(
        superblock.encode().unwrap(),
        *bytes,
        "re-encodes byte for byte"
    );

    let mut recorded = fixture_recorded();
    recorded.data_blocks = 4;
    recorded.salt = "00".into();
    assert_eq!(
        check(&superblock, &recorded),
        [
            Mismatch {
                field: "salt",
                superblock: FIXTURE_SHA256.into(),
                recorded: "00".into(),
            },
            Mismatch {
                field: "data_blocks",
                superblock: "3".into(),
                recorded: "4".into(),
            },
        ]
    );
}

#[test]
fn gaias_superblock_encodes_as_veritysetup_writes_it() {
    let derived = Derived::from_data(&fixture_data());
    let superblock = Superblock::gaia(
        parse_uuid(&derived.uuid).unwrap(),
        (0..32)
            .map(|i| u8::from_str_radix(&derived.salt[2 * i..2 * i + 2], 16).unwrap())
            .collect(),
        3,
        4096,
        4096,
    );
    assert_eq!(
        superblock.encode().unwrap(),
        *include_bytes!("fixtures/veritysetup-2.8.7-superblock.bin")
    );
}

fn veritysetup() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GAIA_TEST_VERITYSETUP") {
        return Some(path.into());
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("veritysetup"))
            .find(|candidate| candidate.is_file())
    })
}

#[test]
fn a_fresh_veritysetup_superblock_matches() {
    let Some(veritysetup) = veritysetup() else {
        eprintln!("skipped: no veritysetup (set GAIA_TEST_VERITYSETUP)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("gaia-formats-verity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("data.img");
    let mut data = fixture_data();
    data.truncate(3 * 4096 - 100);
    let padded = padded_len(data.len() as u64, 4096);
    let offset = appended_hash_offset(padded, 4096);
    let derived = Derived::from_data(&data);
    data.resize(padded as usize, 0);
    std::fs::write(&image, &data).unwrap();
    let output = Command::new(&veritysetup)
        .arg("format")
        .arg(&image)
        .arg(&image)
        .args([
            "--hash=sha256",
            "--data-block-size=4096",
            "--hash-block-size=4096",
        ])
        .arg(format!("--salt={}", derived.salt))
        .arg(format!("--uuid={}", derived.uuid))
        .arg(format!("--hash-offset={offset}"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let bytes = std::fs::read(&image).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let superblock = Superblock::decode(&bytes[offset as usize..]).unwrap();
    let recorded = Recorded {
        salt: derived.salt,
        uuid: derived.uuid,
        data_blocks: 3,
        data_block_size: 4096,
        hash_block_size: 4096,
    };
    assert_eq!(check(&superblock, &recorded), []);
}
