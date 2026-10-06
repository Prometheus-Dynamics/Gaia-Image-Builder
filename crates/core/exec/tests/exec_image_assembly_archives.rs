pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyArchiveMemberSpec, AssemblyArchiveSpec, AssemblyTransformKindSpec, ImageAssemblySpec,
    ResolvedBuildSpec,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use support::{assembly_transform, provider_catalogs, test_spec};

fn run_assembly(spec: &ResolvedBuildSpec) {
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Puts a stand-in `zstd` (a plain copy) into the provider host tools when
/// the real one is not installed.
#[cfg(unix)]
fn ensure_zstd(build_dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    if command_available("zstd") {
        return true;
    }
    let bin = build_dir.join("image/buildroot-output/host/bin");
    fs::create_dir_all(&bin).expect("provider bin");
    let fake = bin.join("zstd");
    fs::write(
        &fake,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake zstd 1.0'; exit 0; fi\nfor last; do :; done\ncat \"$last\"\n",
    )
    .expect("fake zstd");
    let mut permissions = fs::metadata(&fake).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).expect("executable");
    false
}

struct TarEntry {
    name: String,
    mode: String,
    mtime: String,
    contents: Vec<u8>,
}

/// Parses ustar headers, checking the magic and checksum of each entry.
fn parse_tar(bytes: &[u8]) -> Vec<TarEntry> {
    assert_eq!(bytes.len() % 512, 0);
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            assert!(bytes[offset..].iter().all(|byte| *byte == 0));
            assert_eq!(bytes.len() - offset, 1024, "two zero end blocks");
            return entries;
        }
        let field = |range: std::ops::Range<usize>| {
            String::from_utf8(header[range].to_vec())
                .expect("utf-8")
                .trim_end_matches(['\0', ' '])
                .to_string()
        };
        assert_eq!(&header[257..263], b"ustar\0");
        assert_eq!(header[156], b'0');
        assert_eq!(field(108..116), "0000000");
        assert_eq!(field(116..124), "0000000");
        assert!(header[265..329].iter().all(|byte| *byte == 0));
        let mut summed = header.to_vec();
        summed[148..156].copy_from_slice(b"        ");
        let expected: u32 = summed.iter().map(|byte| *byte as u32).sum();
        assert_eq!(
            u32::from_str_radix(&field(148..156), 8).expect("checksum"),
            expected
        );
        let size = u64::from_str_radix(&field(124..136), 8).expect("size") as usize;
        let start = offset + 512;
        entries.push(TarEntry {
            name: field(0..100),
            mode: field(100..108),
            mtime: field(136..148),
            contents: bytes[start..start + size].to_vec(),
        });
        offset = start + size.div_ceil(512) * 512;
    }
    panic!("tar archive is missing its end blocks");
}

fn file_member(name: &str, src: &Path) -> AssemblyArchiveMemberSpec {
    AssemblyArchiveMemberSpec {
        name: name.into(),
        src: Some(src.display().to_string().into()),
        entries: None,
    }
}

#[cfg(unix)]
#[test]
fn archive_bundles_manifest_and_zstd_members_deterministically() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let collect_dir = PathBuf::from(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    let real_zstd = ensure_zstd(&build_dir);
    let sources = build_dir.join("bundle-sources");
    fs::create_dir_all(&sources).expect("sources");
    let boot = sources.join("boot.vfat");
    let rootfs = sources.join("rootfs.ext4");
    fs::write(&boot, b"boot filesystem ".repeat(400)).expect("boot");
    fs::write(&rootfs, b"root filesystem!".repeat(2000)).expect("rootfs");
    let work = build_dir.join("bundle-work");
    let boot_zst = work.join("boot.vfat.zst");
    let rootfs_zst = work.join("rootfs.ext4.zst");
    let mut rootfs_transform = assembly_transform(
        AssemblyTransformKindSpec::Zstd,
        rootfs.display(),
        rootfs_zst.display(),
    );
    rootfs_transform.level = Some(19);
    let bundle = collect_dir.join("helios-1.2.3.pdupdate");
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![
            assembly_transform(
                AssemblyTransformKindSpec::Zstd,
                boot.display(),
                boot_zst.display(),
            ),
            rootfs_transform,
        ],
        archives: vec![AssemblyArchiveSpec {
            id: "update".into(),
            output: bundle.display().to_string().into(),
            members: vec![
                AssemblyArchiveMemberSpec {
                    name: "manifest.env".into(),
                    src: None,
                    entries: Some(vec![
                        ("MODEL".into(), "cm5".into()),
                        ("VERSION".into(), "1.2.3".into()),
                        ("OS".into(), "Helios OS 'stable'".into()),
                        (
                            "BOOT_SHA256".into(),
                            format!("${{assembly.sha256:{}}}", boot_zst.display()),
                        ),
                        (
                            "ROOTFS_SHA256".into(),
                            format!("${{assembly.sha256:{}}}", rootfs_zst.display()),
                        ),
                    ]),
                },
                file_member("boot.vfat.zst", &boot_zst),
                file_member("rootfs.ext4.zst", &rootfs_zst),
            ],
        }],
        ..ImageAssemblySpec::default()
    });

    run_assembly(&spec);

    let first = fs::read(&bundle).expect("bundle");
    let entries = parse_tar(&first);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["manifest.env", "boot.vfat.zst", "rootfs.ext4.zst"]
    );
    for entry in &entries {
        assert_eq!(entry.mode, "0000644");
        assert_eq!(entry.mtime, "00000000000");
    }
    let boot_member = &entries[1].contents;
    let rootfs_member = &entries[2].contents;
    assert_eq!(boot_member, &fs::read(&boot_zst).expect("boot zst"));
    assert_eq!(rootfs_member, &fs::read(&rootfs_zst).expect("rootfs zst"));
    let manifest = String::from_utf8(entries[0].contents.clone()).expect("manifest");
    assert_eq!(
        manifest,
        format!(
            "MODEL=cm5\nVERSION=1.2.3\nOS='Helios OS '\\''stable'\\'''\nBOOT_SHA256={}\nROOTFS_SHA256={}\n",
            sha256_hex(boot_member),
            sha256_hex(rootfs_member)
        )
    );
    if real_zstd {
        let decompressed = Command::new("zstd")
            .arg("-dc")
            .arg(&rootfs_zst)
            .output()
            .expect("zstd -dc");
        assert!(decompressed.status.success());
        assert_eq!(decompressed.stdout, fs::read(&rootfs).expect("rootfs"));
        assert!(boot_member.len() < fs::read(&boot).expect("boot").len());
    }

    if command_available("tar") {
        let listing = Command::new("tar")
            .arg("-tvf")
            .arg(&bundle)
            .output()
            .expect("tar -tvf");
        assert!(
            listing.status.success(),
            "{}",
            String::from_utf8_lossy(&listing.stderr)
        );
        let listing = String::from_utf8_lossy(&listing.stdout);
        let names = listing
            .lines()
            .filter_map(|line| line.split_whitespace().last())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["manifest.env", "boot.vfat.zst", "rootfs.ext4.zst"],
            "{listing}"
        );
        assert!(
            listing.lines().all(|line| line.starts_with("-rw-r--r--")),
            "{listing}"
        );
    }

    let state = fs::read_to_string(
        PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains("transform.1.kind=zstd"), "{state}");
    assert!(state.contains("completed_archive_count=1"), "{state}");
    assert!(
        state.contains(&format!("archives.1.output={}", bundle.display())),
        "{state}"
    );
    assert!(
        state.contains(&format!("archives.1.sha256={}", sha256_hex(&first))),
        "{state}"
    );
    assert!(
        state.contains("archives.1.member.1.generated=true"),
        "{state}"
    );
    assert!(
        state.contains(&format!(
            "archives.1.member.3.sha256={}",
            sha256_hex(rootfs_member)
        )),
        "{state}"
    );

    run_assembly(&spec);
    assert_eq!(first, fs::read(&bundle).expect("second bundle"));
}

#[test]
fn archive_with_missing_digest_file_fails_without_publishing() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let bundle = build_dir.join("missing-digest/bundle.tar");
    spec.image.assembly = Some(ImageAssemblySpec {
        archives: vec![AssemblyArchiveSpec {
            id: "update".into(),
            output: bundle.display().to_string().into(),
            members: vec![AssemblyArchiveMemberSpec {
                name: "manifest.env".into(),
                src: None,
                entries: Some(vec![(
                    "ROOTFS_SHA256".into(),
                    format!(
                        "${{assembly.sha256:{}}}",
                        build_dir.join("absent.ext4").display()
                    ),
                )]),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
    let message = &outcome.errors[0].message;
    assert!(message.contains("member 'manifest.env'"), "{message}");
    assert!(message.contains("does not exist"), "{message}");
    assert!(!bundle.exists());
    let leftovers = fs::read_dir(bundle.parent().expect("parent"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "temporary archive left behind");
}
