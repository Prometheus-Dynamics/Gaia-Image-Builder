use super::*;

fn artifact(target: &str) -> gaia_spec::ArtifactSpec {
    let mut artifact = gaia_spec::ArtifactSpec::new(
        "lemnosd",
        gaia_spec::ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "lemnosd".into(),
            target_name: None,
            variant: gaia_spec::ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: "lemnosd".into(),
        },
    );
    artifact.target = Some(target.to_string());
    artifact
}

/// A 64-bit little-endian ELF header for `machine` (e_machine).
fn elf(path: &Path, machine: u16) {
    let mut header = vec![0u8; 64];
    header[..4].copy_from_slice(b"\x7FELF");
    header[4] = 2;
    header[5] = 1;
    header[18..20].copy_from_slice(&machine.to_le_bytes());
    fs::write(path, header).expect("elf");
}

#[test]
fn static_musl_and_other_libc_targets_are_verified_by_architecture() {
    let dir = temp_path("gaia-artifact-target");
    fs::create_dir_all(&dir).expect("dir");
    let aarch64 = dir.join("lemnosd");
    elf(&aarch64, 0xB7);
    for target in [
        "aarch64-unknown-linux-musl",
        "aarch64-unknown-linux-gnu",
        "linux/arm64",
    ] {
        verify_install_artifact_target(&artifact(target), &aarch64)
            .unwrap_or_else(|error| panic!("{target}: {}", error.message));
    }
    let armv7 = dir.join("board-agent");
    elf(&armv7, 0x28);
    verify_install_artifact_target(&artifact("armv7-unknown-linux-musleabihf"), &armv7)
        .expect("armv7 musl");

    // A binary of another machine is still refused.
    let error = verify_install_artifact_target(&artifact("aarch64-unknown-linux-musl"), &armv7)
        .expect_err("mismatch");
    assert!(
        error.message.contains("target mismatch"),
        "{}",
        error.message
    );
    // And a target Gaia cannot check still is too.
    let error = verify_install_artifact_target(&artifact("mips-unknown-linux-gnu"), &armv7)
        .expect_err("unknown");
    assert!(error.message.contains("cannot verify"), "{}", error.message);
    let _ = fs::remove_dir_all(dir);
}
