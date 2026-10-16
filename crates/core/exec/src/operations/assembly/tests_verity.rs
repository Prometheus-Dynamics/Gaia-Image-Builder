//! `verity` transforms: the hash tree `veritysetup format` writes, in the
//! appended and the separate-file layouts, and the archive entries that read
//! it. The exec tests need `veritysetup` and skip without it; the reference
//! tree below is an independent check of the root hash (dm-verity format 1:
//! the salt is hashed in front of every block). Split from `tests.rs`.

use super::*;
use gaia_spec::{
    AssemblyArchiveMemberSpec, AssemblyArchiveSpec, AssemblyTransformKindSpec,
    AssemblyTransformSpec,
};
use gaia_testkit::TempDir;
use sha2::{Digest, Sha256};
use std::fs;
use std::process::Command;

const BLOCK: usize = 4096;
const HASH_BLOCK: usize = 4096;

fn veritysetup_present() -> bool {
    Command::new("veritysetup")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// The test spec with command timeouts long enough for the e2fsprogs and
/// veritysetup runs on a busy host; the default test timeout is one second.
fn roomy_spec(root: &Path) -> ResolvedBuildSpec {
    let mut spec = test_spec(root);
    spec.policy.providers.buildroot.timeout_seconds = 300;
    spec.policy.providers.starting_point.timeout_seconds = 300;
    spec
}

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

/// Three and a bit blocks of data: the last block is padded.
fn data_bytes() -> Vec<u8> {
    (0..(3 * BLOCK + 100))
        .map(|index| (index * 7 % 251) as u8)
        .collect()
}

/// The root hash of `data` (already padded to whole blocks) under `salt`,
/// per dm-verity format 1: every block, data or hash, is digested as
/// `sha256(salt || block)`; a level of digests is packed into hash blocks,
/// and the root is the digest of the single top block.
fn reference_root_hash(data: &[u8], salt: &[u8]) -> String {
    let digest = |block: &[u8]| -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(salt);
        hasher.update(block);
        hasher.finalize().to_vec()
    };
    let per_block = HASH_BLOCK / 32;
    let mut level: Vec<Vec<u8>> = data.chunks(BLOCK).map(digest).collect();
    loop {
        let blocks: Vec<Vec<u8>> = level
            .chunks(per_block)
            .map(|digests| {
                let mut block = vec![0u8; HASH_BLOCK];
                for (index, value) in digests.iter().enumerate() {
                    block[index * 32..(index + 1) * 32].copy_from_slice(value);
                }
                block
            })
            .collect();
        if blocks.len() == 1 {
            return digest(&blocks[0])
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        }
        level = blocks.iter().map(|block| digest(block)).collect();
    }
}

fn data_file(root: &Path) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("data.bin"), data_bytes()).expect("data");
}

fn verity_transform(dest: &str, hash_dest: Option<&str>) -> AssemblyTransformSpec {
    AssemblyTransformSpec {
        kind: AssemblyTransformKindSpec::Verity,
        src: Some("@assets/data.bin".into()),
        dest: dest.into(),
        deterministic: true,
        level: None,
        hash_dest: hash_dest.map(Into::into),
        data_block_size: None,
        hash_block_size: None,
        salt: None,
        uuid: None,
    }
}

fn verity_assembly(dest: &str, hash_dest: Option<&str>) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        transforms: vec![verity_transform(dest, hash_dest)],
        ..ImageAssemblySpec::default()
    }
}

fn state_value(summary: &AssemblyStagingSummary, key: &str) -> String {
    summary
        .state
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_else(|| panic!("state has no {key}"))
}

#[test]
fn verity_tree_is_appended_after_the_padded_data_and_matches_the_reference() {
    if !veritysetup_present() {
        eprintln!("skipped: veritysetup is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-verity");
    let root = scratch.join("gaia-assembly-verity");
    data_file(&root);
    let mut spec = roomy_spec(&root);
    spec.image.assembly = Some(verity_assembly("$assembly.work/data.erofs", None));
    let operation = OperationId::image_assembly();

    let first = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("first verity assembly");
    let image = root.join("build/assembly/data.erofs");
    let bytes = fs::read(&image).expect("verity image");
    let padded = 4 * BLOCK;
    assert_eq!(&bytes[..3 * BLOCK + 100], &data_bytes()[..], "data is kept");
    assert!(bytes[3 * BLOCK + 100..padded].iter().all(|byte| *byte == 0));

    let root_hash = state_value(&first, "verity.1.root_hash");
    let salt_hex = state_value(&first, "verity.1.salt");
    let source_digest = Sha256::digest(data_bytes());
    let salt: Vec<u8> = source_digest.to_vec();
    assert_eq!(
        salt_hex,
        source_digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        "the default salt is the data's sha256"
    );
    let mut padded_data = data_bytes();
    padded_data.resize(padded, 0);
    assert_eq!(root_hash, reference_root_hash(&padded_data, &salt));
    assert_eq!(
        state_value(&first, "verity.1.hash_offset"),
        padded.to_string()
    );
    assert_eq!(state_value(&first, "verity.1.data_blocks"), "4");
    assert_eq!(
        state_value(&first, "verity.1.data_block_size"),
        BLOCK.to_string()
    );
    assert!(bytes.len() > padded, "the tree is appended");

    let check = Command::new("veritysetup")
        .arg("verify")
        .arg(&image)
        .arg(&image)
        .arg(&root_hash)
        .arg(format!("--hash-offset={padded}"))
        .output()
        .expect("veritysetup verify");
    assert!(check.status.success(), "{check:?}");

    // The same data gives the same bytes and root hash.
    let second = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("second verity assembly");
    assert_eq!(fs::read(&image).expect("second image"), bytes);
    assert_eq!(state_value(&second, "verity.1.root_hash"), root_hash);
}

#[test]
fn verity_tree_in_its_own_file_leaves_the_data_padded_and_alone() {
    if !veritysetup_present() {
        eprintln!("skipped: veritysetup is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-verity-separate");
    let root = scratch.join("gaia-assembly-verity-separate");
    data_file(&root);
    let mut spec = roomy_spec(&root);
    spec.image.assembly = Some(verity_assembly(
        "$assembly.work/data.erofs",
        Some("$assembly.work/data.hash"),
    ));
    let operation = OperationId::image_assembly();
    let summary = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("separate verity assembly");

    let data = fs::read(root.join("build/assembly/data.erofs")).expect("data");
    assert_eq!(data.len(), 4 * BLOCK, "only the padding is added");
    let hash = root.join("build/assembly/data.hash");
    assert!(fs::metadata(&hash).expect("hash file").len() > 0);
    assert_eq!(state_value(&summary, "verity.1.hash_offset"), "0");
    let mut padded_data = data_bytes();
    padded_data.resize(4 * BLOCK, 0);
    let salt = Sha256::digest(data_bytes()).to_vec();
    assert_eq!(
        state_value(&summary, "verity.1.root_hash"),
        reference_root_hash(&padded_data, &salt)
    );
    let check = Command::new("veritysetup")
        .arg("verify")
        .arg(root.join("build/assembly/data.erofs"))
        .arg(&hash)
        .arg(state_value(&summary, "verity.1.root_hash"))
        .output()
        .expect("veritysetup verify");
    assert!(check.status.success(), "{check:?}");
}

#[test]
fn archive_entries_read_the_verity_fields_they_name() {
    if !veritysetup_present() {
        eprintln!("skipped: veritysetup is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-verity-archive");
    let root = scratch.join("gaia-assembly-verity-archive");
    data_file(&root);
    let mut spec = roomy_spec(&root);
    let mut assembly = verity_assembly("$assembly.work/data.erofs", None);
    assembly.archives = vec![AssemblyArchiveSpec {
        id: "bundle".into(),
        output: "$assembly.work/bundle.tar".into(),
        members: vec![AssemblyArchiveMemberSpec {
            tree: None,
            src_dir: None,
            src_glob: None,
            prefix: String::new(),
            exclude: Vec::new(),
            name: "verity.env".into(),
            src: None,
            entries: Some(vec![
                (
                    "VERITY_ROOT_HASH".into(),
                    "${assembly.verity:$assembly.work/data.erofs:root_hash}".into(),
                ),
                (
                    "VERITY_HASH_OFFSET".into(),
                    "${assembly.verity:$assembly.work/data.erofs:hash_offset}".into(),
                ),
            ]),
            json: None,
        }],
    }];
    spec.image.assembly = Some(assembly);
    let operation = OperationId::image_assembly();
    let summary = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("verity archive assembly");
    let tar = fs::read(root.join("build/assembly/bundle.tar")).expect("bundle");
    let text = String::from_utf8_lossy(&tar);
    let root_hash = state_value(&summary, "verity.1.root_hash");
    assert!(
        text.contains(&format!("VERITY_ROOT_HASH={root_hash}")),
        "{text}"
    );
    assert!(text.contains(&format!("VERITY_HASH_OFFSET={}", 4 * BLOCK)));
}

#[test]
fn archive_entry_naming_no_verity_output_fails_without_running_a_tool() {
    let scratch = TempDir::new("gaia-exec-verity-missing");
    let root = scratch.join("gaia-assembly-verity-missing");
    fs::create_dir_all(&root).expect("root");
    let mut spec = roomy_spec(&root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        archives: vec![AssemblyArchiveSpec {
            id: "bundle".into(),
            output: "$assembly.work/bundle.tar".into(),
            members: vec![AssemblyArchiveMemberSpec {
                tree: None,
                src_dir: None,
                src_glob: None,
                prefix: String::new(),
                exclude: Vec::new(),
                name: "verity.env".into(),
                src: None,
                entries: Some(vec![(
                    "ROOT".into(),
                    "${assembly.verity:$assembly.work/nothing.erofs:root_hash}".into(),
                )]),
                json: None,
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let operation = OperationId::image_assembly();
    let error = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect_err("no verity output");
    assert!(
        error.message.contains("names no verity transform output"),
        "{}",
        error.message
    );
}

#[test]
fn reference_tree_root_hash_covers_one_and_many_hash_levels() {
    // One data block: the root is the digest of the single hash block.
    let one = vec![9u8; BLOCK];
    let salt = b"salt";
    let mut block = vec![0u8; HASH_BLOCK];
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(&one);
    block[..32].copy_from_slice(&hasher.finalize());
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(&block);
    let expected: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(reference_root_hash(&one, salt), expected);

    // A hash block holds 128 digests: 128 data blocks fit one hash block,
    // 129 need two level-0 blocks and so a second level.
    let full = vec![3u8; 128 * BLOCK];
    let deeper = vec![3u8; 129 * BLOCK];
    let root = reference_root_hash(&full, salt);
    assert_eq!(root.len(), 64);
    assert_ne!(root, reference_root_hash(&deeper, salt));
}
