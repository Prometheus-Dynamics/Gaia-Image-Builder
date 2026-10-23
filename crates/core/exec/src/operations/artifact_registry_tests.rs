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
