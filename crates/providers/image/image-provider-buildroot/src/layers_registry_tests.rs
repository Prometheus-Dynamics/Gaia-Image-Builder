//! The registry cache of `[image.layers]`: a layer built here is pushed under
//! its fingerprint, and a layer missing locally is pulled and then reused by
//! the local reuse path. Runs against the fake registry only.
use super::*;
use crate::layers_tar::LayerTarWriter;
use gaia_process::fake::FakeRegistry;
use gaia_process::{RegistryTarget, SignatureKind};
use gaia_spec::{ImageLayerSpec, ImageLayersSpec, ResolvedBuildSpec};
use gaia_testkit::TempDir;
use std::io::{BufWriter, Write};
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

/// A rootfs with two layers' worth of paths: `base` takes `etc` and `usr`,
/// `app` takes `/opt/app` by its path selector.
fn write_rootfs(path: &Path) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    let file = fs::File::create(path).expect("create tar");
    let mut writer = LayerTarWriter::new(BufWriter::new(file), 0);
    writer.dir("etc", 0o755, 0, 0).expect("dir");
    writer
        .file("etc/motd", 0o644, 0, 0, 6, |out| {
            out.write_all(b"hello\n").map_err(|error| error.to_string())
        })
        .expect("file");
    writer.dir("opt", 0o755, 0, 0).expect("dir");
    writer.dir("opt/app", 0o755, 0, 0).expect("dir");
    writer
        .file("opt/app/run", 0o750, 1000, 1000, 9, |out| {
            out.write_all(b"run bytes")
                .map_err(|error| error.to_string())
        })
        .expect("file");
    let mut out = writer.finish().expect("finish");
    out.flush().expect("flush");
}

fn fake_tree(root: &Path) -> PathBuf {
    let tree = root.join("tree");
    write_rootfs(&tree.join("images/rootfs.tar"));
    fs::create_dir_all(tree.join("build")).expect("build dir");
    fs::write(tree.join("build/packages-file-list.txt"), "").expect("package list");
    tree
}

fn layers_spec() -> ImageLayersSpec {
    ImageLayersSpec {
        order: vec!["base".into(), "app".into()],
        default: "base".into(),
        overrides: Vec::new(),
        erofs: false,
        verity: false,
        layers: vec![ImageLayerSpec {
            name: "app".into(),
            packages: Vec::new(),
            paths: vec!["/opt/app".into()],
            installs: Vec::new(),
            stage: Vec::new(),
            services: Vec::new(),
        }],
    }
}

/// The layer entries of the fake registry: every tag of the layer image.
fn layer_tags(registry: &FakeRegistry) -> Vec<String> {
    registry
        .state()
        .manifests
        .get("owner/gaia-cache/layer")
        .map(|tags| tags.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn a_pushed_layer_is_pulled_into_an_empty_collect_dir_and_reused_there() {
    let scratch = TempDir::new("gaia-layer-registry");
    let tools = stub_tools(&scratch.join("tools"));
    let registry = FakeRegistry::start(None);
    let layers = layers_spec();
    let spec = ResolvedBuildSpec::new("layers-test");

    let tree = fake_tree(&scratch.join("root"));
    let first = scratch.join("first");
    let built = build_layers_with(
        &spec,
        &layers,
        &tree,
        &first,
        Some(&registry_use(&registry, &tools, true)),
    )
    .expect("build and push");
    assert!(
        built
            .iter()
            .any(|line| line.contains("pushed to the registry checkpoint 'registry'")),
        "{built:?}"
    );
    assert!(
        built.iter().any(|line| line.contains("layers: 2 pushed")),
        "{built:?}"
    );

    let second = scratch.join("second");
    let pulled = build_layers_with(
        &spec,
        &layers,
        &tree,
        &second,
        Some(&registry_use(&registry, &tools, false)),
    )
    .expect("pull");
    assert!(
        pulled
            .iter()
            .any(|line| line.contains("pulled from the registry checkpoint 'registry'")),
        "{pulled:?}"
    );
    assert!(
        pulled
            .iter()
            .any(|line| line.contains("2 of 2 reused, 0 rebuilt")),
        "{pulled:?}"
    );
    for file in ["base.tar", "app.tar", LAYERS_JSON] {
        assert_eq!(
            fs::read(first.join(LAYERS_DIR).join(file)).expect("built output"),
            fs::read(second.join(LAYERS_DIR).join(file)).expect("pulled output"),
            "{file} is the built bytes"
        );
    }

    // The local reuse accepts the pulled layers without the registry.
    let local = build_layers_with(&spec, &layers, &tree, &second, None).expect("local reuse");
    assert!(
        local.iter().any(|line| line.contains("2 of 2 reused")),
        "{local:?}"
    );
    assert!(
        !local.iter().any(|line| line.contains("pulled")),
        "{local:?}"
    );
}

#[test]
fn a_forged_layer_entry_is_refused_and_the_layer_is_built() {
    let scratch = TempDir::new("gaia-layer-registry-forged");
    let tools = stub_tools(&scratch.join("tools"));
    let registry = FakeRegistry::start(None);
    let tree = fake_tree(&scratch.join("root"));
    let layers = layers_spec();
    let spec = ResolvedBuildSpec::new("layers-test");
    let first = scratch.join("first");
    build_layers_with(
        &spec,
        &layers,
        &tree,
        &first,
        Some(&registry_use(&registry, &tools, true)),
    )
    .expect("build and push");
    // Every layer manifest changes to bytes no key signed (still valid JSON).
    {
        let mut state = registry.state();
        let tags = state
            .manifests
            .get_mut("owner/gaia-cache/layer")
            .expect("layer image");
        for key in tags.keys().cloned().collect::<Vec<_>>() {
            let mut manifest = tags.get(&key).expect("entry tag").clone();
            manifest.push(b' ');
            tags.insert(key, manifest);
        }
    }

    let second = scratch.join("second");
    let messages = build_layers_with(
        &spec,
        &layers,
        &tree,
        &second,
        Some(&registry_use(&registry, &tools, false)),
    )
    .expect("the build goes on");
    assert!(
        messages.iter().any(|line| line.contains("SIGNATURE")),
        "{messages:?}"
    );
    assert!(
        !messages.iter().any(|line| line.contains("pulled")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|line| line.contains("0 of 2 reused")),
        "{messages:?}"
    );
    // The built layer is the one a local build makes, and nothing was pulled.
    assert_eq!(
        fs::read(first.join(LAYERS_DIR).join("base.tar")).expect("first"),
        fs::read(second.join(LAYERS_DIR).join("base.tar")).expect("second"),
    );
}

#[test]
fn without_a_sign_key_no_layer_is_pushed() {
    let scratch = TempDir::new("gaia-layer-registry-nopush");
    let tools = stub_tools(&scratch.join("tools"));
    let registry = FakeRegistry::start(None);
    let tree = fake_tree(&scratch.join("root"));
    let layers = layers_spec();
    let spec = ResolvedBuildSpec::new("layers-test");
    let collect = scratch.join("collect");
    let messages = build_layers_with(
        &spec,
        &layers,
        &tree,
        &collect,
        Some(&registry_use(&registry, &tools, false)),
    )
    .expect("build");
    assert!(
        !messages.iter().any(|line| line.contains("pushed")),
        "{messages:?}"
    );
    let writes: Vec<String> = registry
        .state()
        .requests
        .iter()
        .filter(|request| !request.starts_with("GET ") && !request.starts_with("HEAD "))
        .cloned()
        .collect();
    assert!(
        writes.is_empty(),
        "no write reached the registry: {writes:?}"
    );
    assert!(layer_tags(&registry).is_empty());
}
