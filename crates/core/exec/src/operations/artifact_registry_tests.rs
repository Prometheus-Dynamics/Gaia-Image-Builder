//! The artifact registry cache: an output pushed under its operation
//! fingerprint is restored into an empty workspace, byte for byte, and the
//! plan's content signature of the restored output is the built one. Runs
//! against the fake registry only.
use super::*;
use gaia_image_providers::RegistryCacheUse;
use gaia_plan::operation_content_signature;
use gaia_process::fake::FakeRegistry;
use gaia_process::{RegistryTarget, SignatureKind};
use gaia_spec::{ArtifactDefinition, ArtifactOutputSpec, GoArtifactSpec, ResolvedBuildSpec};
use gaia_testkit::TempDir;
use std::os::unix::fs::PermissionsExt;

/// A `minisign` stand-in (none is installed here): signing writes the file's
/// sha256 to the `-x` path, and verifying compares it.
fn stub_tools(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("tools dir");
    let script = dir.join("minisign");
    let body = concat!(
        "#!/bin/sh\n",
        "mode=\"\"; file=\"\"; sig=\"\"\n",
        "while [ $# -gt 0 ]; do\n",
        "  case \"$1\" in\n",
        "    -S) mode=sign ;;\n",
        "    -V) mode=verify ;;\n",
        "    -m) file=\"$2\"; shift ;;\n",
        "    -x) sig=\"$2\"; shift ;;\n",
        "  esac\n",
        "  shift\n",
        "done\n",
        "sum=$(sha256sum \"$file\" | cut -d' ' -f1)\n",
        "if [ \"$mode\" = sign ]; then echo \"$sum\" > \"$sig\"; exit 0; fi\n",
        "[ \"$sum\" = \"$(cat \"$sig\")\" ]\n",
    );
    fs::write(&script, body).expect("stub");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("mode");
    dir.to_path_buf()
}

/// The registry cache of these tests: pulls on, pushes when `sign` is set.
fn registry_use(registry: &FakeRegistry, tools: &Path, sign: bool) -> RegistryCacheUse {
    RegistryCacheUse {
        checkpoint_id: "registry".into(),
        target: RegistryTarget {
            registry: registry.addr().to_string(),
            repository: "owner/gaia-cache".into(),
            kind: SignatureKind::Minisign,
            sign_key: sign.then(|| "/keys/gaia.sec".to_string()),
            verify_key: Some("/keys/gaia.pub".to_string()),
            trust_unsigned: false,
            token: None,
            username: None,
            tool_dir: Some(tools.to_path_buf()),
        },
        pull: true,
        push: sign,
        warnings: Vec::new(),
    }
}

/// A workspace under `root` with one file artifact, `helios`, at `out/helios.bin`.
fn spec_in(root: &Path) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("artifact-registry-test");
    spec.workspace.root_dir = root.display().to_string();
    spec.artifacts.push(ArtifactSpec::new(
        "helios",
        ArtifactDefinition::Go(GoArtifactSpec {
            package: "example.com/helios".into(),
        }),
        None,
        ArtifactOutputSpec {
            path: "out/helios.bin".into(),
        },
    ));
    spec
}

/// The output and its provider state, as a local build leaves them.
fn write_built_output(root: &Path) {
    fs::create_dir_all(root.join("out/.gaia")).expect("dirs");
    fs::write(root.join("out/helios.bin"), b"helios bytes").expect("output");
    fs::write(
        root.join("out/.gaia/helios.bin.gaia-state.txt"),
        "digest=abc\n",
    )
    .expect("state");
}

fn built_kind(spec: &ResolvedBuildSpec) -> OperationKind {
    OperationKind::BuildArtifact {
        artifact_id: spec.artifacts[0].id.clone(),
    }
}

#[test]
fn a_pushed_artifact_is_restored_into_an_empty_workspace() {
    let scratch = TempDir::new("gaia-artifact-registry");
    let tools = stub_tools(&scratch.join("tools"));
    let registry = FakeRegistry::start(None);
    let spec = spec_in(scratch.path());
    let artifact = &spec.artifacts[0];
    write_built_output(scratch.path());
    let built = operation_content_signature(&spec, &built_kind(&spec));

    let pushed = publish_with(
        Some(&registry_use(&registry, &tools, true)),
        &spec,
        artifact,
    );
    assert!(
        pushed
            .iter()
            .any(|line| line.contains("pushed to the registry checkpoint 'registry'")),
        "{pushed:?}"
    );

    // A fresh workspace: the output and its state are gone.
    fs::remove_dir_all(scratch.join("out")).expect("removed");
    assert_ne!(
        operation_content_signature(&spec, &built_kind(&spec)),
        built,
        "the output is gone"
    );
    let restored = restore_with(
        Some(&registry_use(&registry, &tools, false)),
        &spec,
        artifact,
    );
    assert!(restored.restored, "{:?}", restored.messages);
    assert_eq!(
        fs::read(scratch.join("out/helios.bin")).expect("restored output"),
        b"helios bytes"
    );
    assert_eq!(
        fs::read_to_string(scratch.join("out/.gaia/helios.bin.gaia-state.txt"))
            .expect("restored state"),
        "digest=abc\n"
    );
    // The plan's reuse reads the same state signature as after the local build.
    assert_eq!(
        operation_content_signature(&spec, &built_kind(&spec)),
        built,
        "the restored artifact is the built one"
    );
}

#[test]
fn a_forged_artifact_entry_is_refused_and_nothing_is_restored() {
    let scratch = TempDir::new("gaia-artifact-registry-forged");
    let tools = stub_tools(&scratch.join("tools"));
    let registry = FakeRegistry::start(None);
    let spec = spec_in(scratch.path());
    let artifact = &spec.artifacts[0];
    write_built_output(scratch.path());
    publish_with(
        Some(&registry_use(&registry, &tools, true)),
        &spec,
        artifact,
    );
    // Every entry manifest changes to bytes no key signed (still valid JSON).
    {
        let mut state = registry.state();
        let tags = state
            .manifests
            .get_mut("owner/gaia-cache/artifact")
            .expect("artifact image");
        for key in tags.keys().cloned().collect::<Vec<_>>() {
            let mut manifest = tags.get(&key).expect("entry tag").clone();
            manifest.push(b' ');
            tags.insert(key, manifest);
        }
    }
    fs::remove_dir_all(scratch.join("out")).expect("removed");

    let restored = restore_with(
        Some(&registry_use(&registry, &tools, false)),
        &spec,
        artifact,
    );
    assert!(!restored.restored);
    assert!(
        restored
            .messages
            .iter()
            .any(|line| line.contains("SIGNATURE")),
        "{:?}",
        restored.messages
    );
    assert!(!scratch.join("out/helios.bin").exists());
    assert!(!scratch.join("out/.gaia").exists());
}

#[test]
fn without_a_sign_key_nothing_is_pushed() {
    let scratch = TempDir::new("gaia-artifact-registry-nopush");
    let tools = stub_tools(&scratch.join("tools"));
    let registry = FakeRegistry::start(None);
    let spec = spec_in(scratch.path());
    write_built_output(scratch.path());
    let messages = publish_with(
        Some(&registry_use(&registry, &tools, false)),
        &spec,
        &spec.artifacts[0],
    );
    assert!(messages.is_empty(), "{messages:?}");
    assert!(
        registry.state().requests.is_empty(),
        "no request reached the registry"
    );
}

#[test]
fn the_key_follows_the_content_of_the_artifacts_source() {
    let dir = TempDir::new("artifact-registry-key-source");
    let root = dir.path().join("ws");
    let source_dir = root.join("src-tree");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("main.rs"), "fn main() {}\n").expect("source file");
    let mut spec = spec_in(&root);
    spec.sources.push(gaia_spec::SourceSpec::new(
        "app-src",
        gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
            path: source_dir.display().to_string(),
            identity_ignore: Vec::new(),
            refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
            pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
        }),
    ));
    spec.artifacts[0].source = Some(gaia_spec::SourceRef::new("app-src"));
    let artifact = spec.artifacts[0].clone();

    let first = registry_key(&spec, &artifact);
    assert_eq!(first.len(), 64);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        registry_key(&spec, &artifact),
        first,
        "stable for the same content"
    );

    // The build's config is unchanged; only the source's bytes are not.
    fs::write(
        source_dir.join("main.rs"),
        "fn main() { println!(\"changed\"); }\n",
    )
    .expect("edit source");
    let second = registry_key(&spec, &artifact);
    assert_ne!(second, first, "a source edit must change the key");

    fs::write(source_dir.join("main.rs"), "fn main() {}\n").expect("restore source");
    assert_eq!(
        registry_key(&spec, &artifact),
        first,
        "the same content gives the same key"
    );
}

#[test]
fn the_same_source_content_in_another_checkout_gives_the_same_key() {
    let dir = TempDir::new("artifact-registry-key-checkouts");
    let key_in = |root: &Path| {
        let source_dir = root.join("src-tree");
        fs::create_dir_all(&source_dir).expect("source dir");
        fs::write(source_dir.join("main.rs"), "fn main() {}\n").expect("source file");
        let mut spec = spec_in(root);
        spec.sources.push(gaia_spec::SourceSpec::new(
            "app-src",
            gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
                path: source_dir.display().to_string(),
                identity_ignore: Vec::new(),
                refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
                pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
            }),
        ));
        spec.artifacts[0].source = Some(gaia_spec::SourceRef::new("app-src"));
        let artifact = spec.artifacts[0].clone();
        registry_key(&spec, &artifact)
    };
    assert_eq!(
        key_in(&dir.path().join("checkout-a")),
        key_in(&dir.path().join("checkout-b"))
    );
}

#[test]
fn the_key_follows_the_toolchain_an_artifact_builds_with() {
    let dir = TempDir::new("artifact-registry-key-toolchain");
    let root = dir.path().join("ws");
    let mut spec = spec_in(&root);
    let artifact_key = |spec: &ResolvedBuildSpec| {
        let artifact = spec.artifacts[0].clone();
        registry_key(spec, &artifact)
    };
    let without = artifact_key(&spec);

    spec.toolchains.push(gaia_spec::ToolchainSpec {
        id: "sdk".into(),
        kind: gaia_spec::ToolchainKindSpec::BuildrootSdk,
        root_source: None,
        root_path: Some(root.join("sdk").display().to_string()),
        relocate: false,
        prefix: Some("aarch64-linux-".into()),
        triple: "aarch64-buildroot-linux-gnu".into(),
        sysroot_path: Some("sysroot".into()),
        sysroot_source: None,
        rust_target: Some("aarch64-unknown-linux-gnu".into()),
        page_size: None,
    });
    spec.artifacts[0].toolchain = Some("sdk".into());
    let with_sdk = artifact_key(&spec);
    assert_ne!(with_sdk, without, "a toolchain changes the entry");

    spec.toolchains[0].prefix = Some("aarch64-other-".into());
    assert_ne!(
        artifact_key(&spec),
        with_sdk,
        "another compiler prefix is another toolchain"
    );
    spec.toolchains[0].prefix = Some("aarch64-linux-".into());
    assert_eq!(
        artifact_key(&spec),
        with_sdk,
        "the same toolchain, the same key"
    );
}
