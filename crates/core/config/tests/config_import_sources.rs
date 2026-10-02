//! `imports = [{ source = "<id>", path = ".." }]`, `@self` and `@source:`.
//! Every repository is a local `file://` repo in a temp dir.

use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_spec::{ResolvedBuildSpec, SourceDefinition};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEVICE_LAYER: &str = r#"
imports = ["units.toml"]

[[stage.files]]
id = "raze-overlay"
src = "@self/overlays/raze.dtbo"
dest = "/boot/overlays/raze.dtbo"
"#;

const UNITS_LAYER: &str = r#"
[[stage.services]]
id = "raze-camera"
name = "raze-camera.service"
unit_path = "@self/units/raze-camera.service"
"#;

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    repo: PathBuf,
    commits: Vec<String>,
}

impl Fixture {
    /// A workspace (with `Cargo.toml`) and a git repo laid out like Atlas,
    /// with two commits; the second only touches `README.md`.
    fn new(name: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gaia-import-src-{name}-{nonce}"));
        let workspace = root.join("workspace");
        let repo = root.join("atlas");
        fs::create_dir_all(workspace.join("configs/builds")).expect("workspace");
        fs::write(workspace.join("Cargo.toml"), "[workspace]\n").expect("cargo");
        write_device_tree(&repo.join("devices/raze/gaia"));
        fs::write(repo.join("README.md"), "one\n").expect("readme");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "one"]);
        let first = head(&repo);
        fs::write(repo.join("README.md"), "two\n").expect("readme");
        git(&repo, &["commit", "--quiet", "-am", "two"]);
        let second = head(&repo);
        Self {
            root: fs::canonicalize(&root).expect("canonical root"),
            workspace: fs::canonicalize(&workspace).expect("canonical workspace"),
            repo,
            commits: vec![first, second],
        }
    }

    fn repo_url(&self) -> String {
        format!("file://{}", self.repo.display())
    }

    fn build_file(&self) -> PathBuf {
        self.workspace.join("configs/builds/cm5.toml")
    }

    /// Writes the entrypoint: `imports` + `rest` + the atlas source.
    fn write_build(&self, imports: &str, source_extra: &str, rest: &str) {
        fs::write(
            self.build_file(),
            format!(
                "build_name = \"cm5\"\ntarget = \"cm5\"\nimports = [{imports}]\n{rest}\n\
                 [[sources]]\nid = \"atlas\"\nkind = \"git\"\nrepo = \"{}\"\n{source_extra}\n",
                self.repo_url()
            ),
        )
        .expect("build file");
    }

    fn checkout(&self, rev: &str) -> PathBuf {
        self.workspace
            .join(".gaia/cache/import-sources")
            .join(format!("atlas-{rev}"))
    }

    fn resolve(&self, overrides: &[(&str, &str)]) -> Result<ResolvedBuildSpec, String> {
        try_resolve_config_with_options(
            &self.build_file().display().to_string(),
            &ResolveOptions {
                explicit_overrides: overrides
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_device_tree(dir: &Path) {
    fs::create_dir_all(dir.join("overlays")).expect("overlays");
    fs::create_dir_all(dir.join("units")).expect("units");
    fs::write(dir.join("device.toml"), DEVICE_LAYER).expect("device");
    fs::write(dir.join("units.toml"), UNITS_LAYER).expect("units layer");
    fs::write(dir.join("overlays/raze.dtbo"), "dtbo").expect("dtbo");
    fs::write(dir.join("units/raze-camera.service"), "[Unit]\n").expect("unit");
}

fn git(repo: &Path, args: &[&str]) {
    fs::create_dir_all(repo).expect("repo dir");
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=gaia",
            "-c",
            "user.email=gaia@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn head(repo: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("git");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stage_file_src(spec: &ResolvedBuildSpec, id: &str) -> Option<String> {
    spec.stage
        .files
        .iter()
        .find(|file| file.id.as_str() == id)
        .map(|file| file.src.clone())
}

fn service_unit(spec: &ResolvedBuildSpec, id: &str) -> Option<String> {
    spec.stage
        .services
        .iter()
        .find(|service| service.id.as_str() == id)
        .map(|service| service.unit_path.clone())
}

const ATLAS_IMPORT: &str = r#"{ source = "atlas", path = "devices/raze/gaia/device.toml" }"#;

#[test]
fn imports_from_a_git_source_at_rev_with_nested_import_and_tokens() {
    let fixture = Fixture::new("rev");
    let rev = fixture.commits[0].clone();
    fixture.write_build(
        ATLAS_IMPORT,
        &format!("rev = \"{rev}\""),
        r#"
[[stage.files]]
id = "consumer-ref"
src = "@source:atlas/devices/raze/gaia/overlays/raze.dtbo"
dest = "/opt/raze.dtbo"
"#,
    );
    let spec = fixture.resolve(&[]).expect("resolve");
    let layer_dir = fixture.checkout(&rev).join("devices/raze/gaia");
    assert_eq!(
        stage_file_src(&spec, "raze-overlay").as_deref(),
        Some(layer_dir.join("overlays/raze.dtbo").to_str().unwrap()),
        "@self resolves to the directory of the imported file"
    );
    assert_eq!(
        service_unit(&spec, "raze-camera").as_deref(),
        Some(
            layer_dir
                .join("units/raze-camera.service")
                .to_str()
                .unwrap()
        ),
        "nested import inside the checkout resolves relative to its importer"
    );
    assert_eq!(
        stage_file_src(&spec, "consumer-ref").as_deref(),
        Some(layer_dir.join("overlays/raze.dtbo").to_str().unwrap()),
        "@source:<id> resolves to the checkout"
    );
    let import = &spec.selection.import_sources[0];
    assert_eq!(import.id, "atlas");
    assert_eq!(import.identity, format!("git:{}@{rev}", fixture.repo_url()));
    assert!(import.contributes_to("stage-file:raze-overlay"));
    assert!(import.contributes_to("stage-service:raze-camera"));
    assert!(!import.contributes_to("stage-file:consumer-ref"));

    // The checkout is reused: resolving again works without the repo.
    fs::rename(&fixture.repo, fixture.root.join("atlas-moved")).expect("move repo");
    let again = fixture
        .resolve(&[])
        .expect("resolve from existing checkout");
    assert_eq!(
        stage_file_src(&again, "raze-overlay"),
        stage_file_src(&spec, "raze-overlay")
    );
}

#[test]
fn when_conditions_filter_source_imports_before_fetching() {
    let fixture = Fixture::new("when");
    let rev = fixture.commits[0].clone();
    fixture.write_build(
        r#"{ source = "atlas", path = "devices/raze/gaia/device.toml", when = { target = "rpi5" } }"#,
        &format!("rev = \"{rev}\""),
        "",
    );
    let spec = fixture.resolve(&[]).expect("resolve");
    assert!(stage_file_src(&spec, "raze-overlay").is_none());
    assert!(spec.selection.import_sources.is_empty());
    assert!(
        !fixture.checkout(&rev).exists(),
        "skipped import must not fetch"
    );

    let spec = fixture
        .resolve(&[("build.target", "rpi5")])
        .expect("resolve rpi5");
    assert!(stage_file_src(&spec, "raze-overlay").is_some());
}

#[test]
fn vendored_copy_resolves_self_like_the_checkout() {
    let fixture = Fixture::new("vendored");
    let vendored = fixture.workspace.join("vendor/atlas/devices/raze/gaia");
    write_device_tree(&vendored);
    fixture.write_build(
        r#""../../vendor/atlas/devices/raze/gaia/device.toml""#,
        "",
        "",
    );
    let spec = fixture.resolve(&[]).expect("resolve");
    assert_eq!(
        stage_file_src(&spec, "raze-overlay").as_deref(),
        Some(vendored.join("overlays/raze.dtbo").to_str().unwrap())
    );
    assert_eq!(
        service_unit(&spec, "raze-camera").as_deref(),
        Some(vendored.join("units/raze-camera.service").to_str().unwrap())
    );
    assert!(spec.selection.import_sources.is_empty());
}

#[test]
fn rejects_imports_that_escape_the_checkout() {
    let fixture = Fixture::new("escape");
    fs::write(
        fixture.repo.join("escape.toml"),
        "imports = [\"../../outside.toml\"]\n",
    )
    .expect("escape layer");
    git(&fixture.repo, &["add", "."]);
    git(&fixture.repo, &["commit", "--quiet", "-m", "escape"]);
    let rev = head(&fixture.repo);
    fixture.write_build(
        r#"{ source = "atlas", path = "escape.toml" }"#,
        &format!("rev = \"{rev}\""),
        "",
    );
    let error = fixture.resolve(&[]).expect_err("nested escape");
    assert!(error.contains("outside the checkout"), "{error}");
    assert!(error.contains("'atlas'"), "{error}");
    assert!(error.contains("escape.toml"), "{error}");

    fixture.write_build(
        r#"{ source = "atlas", path = "../atlas-other/device.toml" }"#,
        &format!("rev = \"{rev}\""),
        "",
    );
    let error = fixture.resolve(&[]).expect_err("direct escape");
    assert!(error.contains("outside the checkout"), "{error}");
    assert!(error.contains("cm5.toml"), "{error}");
}

#[test]
fn unpinned_import_source_names_the_import_and_suggests_rev_or_lock() {
    let fixture = Fixture::new("unpinned");
    fixture.write_build(ATLAS_IMPORT, "", "");
    let error = fixture.resolve(&[]).expect_err("unpinned");
    assert!(error.contains("import source 'atlas'"), "{error}");
    assert!(
        error.contains(fixture.build_file().to_str().unwrap()),
        "{error}"
    );
    assert!(error.contains("rev = "), "{error}");
    assert!(error.contains("gaia lock"), "{error}");
}

#[test]
fn undeclared_import_source_is_an_error() {
    let fixture = Fixture::new("undeclared");
    fixture.write_build(r#"{ source = "nope", path = "device.toml" }"#, "", "");
    let error = fixture.resolve(&[]).expect_err("undeclared");
    assert!(error.contains("import source 'nope'"), "{error}");
    assert!(
        error.contains("not declared") || error.contains("no git source"),
        "{error}"
    );
}

#[test]
fn lockfile_supplies_the_revision() {
    let fixture = Fixture::new("lock");
    let locked = fixture.commits[0].clone();
    fixture.write_build(ATLAS_IMPORT, "", "");
    fs::write(
        fixture.workspace.join("configs/builds/cm5.gaia.lock"),
        format!(
            "version = 1\n\n[[git]]\nsource = \"atlas\"\nrepo = \"{}\"\nref = \"head:HEAD\"\ncommit = \"{locked}\"\n",
            fixture.repo_url()
        ),
    )
    .expect("lockfile");
    let spec = fixture.resolve(&[]).expect("resolve");
    assert!(
        stage_file_src(&spec, "raze-overlay")
            .unwrap()
            .starts_with(fixture.checkout(&locked).to_str().unwrap())
    );
    let atlas = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "atlas")
        .expect("atlas source");
    let SourceDefinition::Git(git) = &atlas.definition else {
        panic!("atlas stays a git source");
    };
    assert_eq!(git.locked_commit.as_deref(), Some(locked.as_str()));
}

#[test]
fn local_path_override_reads_the_directory_and_materializes_as_path() {
    let fixture = Fixture::new("override");
    let dev = fixture.workspace.join("atlas-dev");
    write_device_tree(&dev.join("devices/raze/gaia"));
    // No rev: the override needs no pin.
    fixture.write_build(ATLAS_IMPORT, "", "");
    let spec = fixture
        .resolve(&[("sources.atlas.path", "atlas-dev")])
        .expect("resolve with override");
    assert_eq!(
        stage_file_src(&spec, "raze-overlay").as_deref(),
        Some(
            dev.join("devices/raze/gaia/overlays/raze.dtbo")
                .to_str()
                .unwrap()
        )
    );
    let atlas = spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == "atlas")
        .expect("atlas source");
    assert!(
        matches!(&atlas.definition, SourceDefinition::Path(path) if path.path == "atlas-dev"),
        "{:?}",
        atlas.definition
    );
    let identity = &spec.selection.import_sources[0].identity;
    assert!(
        identity.starts_with(&format!("path:{}#", dev.display())),
        "{identity}"
    );
    assert!(
        !fixture
            .workspace
            .join(".gaia/cache/import-sources")
            .exists()
    );

    // Editing a layer file changes the override's identity.
    fs::write(
        dev.join("devices/raze/gaia/units.toml"),
        format!("{UNITS_LAYER}\n# edited\n"),
    )
    .expect("edit");
    let edited = fixture
        .resolve(&[("sources.atlas.path", "atlas-dev")])
        .expect("resolve edited");
    assert_ne!(&edited.selection.import_sources[0].identity, identity);
}

#[test]
fn source_imported_files_cannot_declare_import_sources() {
    let fixture = Fixture::new("remote-decl");
    fs::write(
        fixture.repo.join("chain.toml"),
        "imports = [{ source = \"inner\", path = \"x.toml\" }]\n\n[[sources]]\nid = \"inner\"\nkind = \"git\"\nrepo = \"file:///nowhere\"\nrev = \"abc\"\n",
    )
    .expect("chain");
    git(&fixture.repo, &["add", "."]);
    git(&fixture.repo, &["commit", "--quiet", "-m", "chain"]);
    let rev = head(&fixture.repo);
    fixture.write_build(
        r#"{ source = "atlas", path = "chain.toml" }"#,
        &format!("rev = \"{rev}\""),
        "",
    );
    let error = fixture.resolve(&[]).expect_err("remote declaration");
    assert!(error.contains("import source 'inner'"), "{error}");
    assert!(error.contains("chain.toml"), "{error}");
}

/// A local layer selected for one target only references the source with
/// `@source:` (inside a `:`-separated list). Other targets must not fetch the
/// source, resolve its tokens, or plan it, even when its rev is unreachable.
#[test]
fn non_selected_local_layers_neither_fetch_nor_plan_their_import_source() {
    let fixture = Fixture::new("local-when");
    fs::create_dir_all(fixture.workspace.join("configs/layers")).expect("layers dir");
    fs::write(
        fixture.workspace.join("configs/layers/raze.toml"),
        r#"
[[stage.files]]
id = "raze-overlay"
src = "@source:atlas/devices/raze/gaia/overlays/raze.dtbo:raze/assets/extra"
dest = "/boot/overlays/raze.dtbo"
"#,
    )
    .expect("raze layer");
    let imports = r#""../layers/raze.toml""#;
    let raze_only = format!("{{ path = {imports}, when = {{ target = \"raze\" }} }}");

    // An unreachable rev, like an unpushed commit: non-raze targets must not care.
    fixture.write_build(
        &raze_only,
        "rev = \"0000000000000000000000000000000000000000\"",
        "",
    );
    let spec = fixture.resolve(&[]).expect("non-raze target resolves");
    assert!(stage_file_src(&spec, "raze-overlay").is_none());
    assert!(spec.selection.import_sources.is_empty());
    assert!(
        spec.sources
            .iter()
            .all(|source| source.id.as_str() != "atlas"),
        "an import-only source of a non-selected layer must not be planned"
    );

    let rev = fixture.commits[0].clone();
    fixture.write_build(&raze_only, &format!("rev = \"{rev}\""), "");
    let spec = fixture
        .resolve(&[("build.target", "raze")])
        .expect("raze target resolves");
    let src = stage_file_src(&spec, "raze-overlay").expect("raze overlay staged");
    assert_eq!(
        src,
        format!(
            "{}/devices/raze/gaia/overlays/raze.dtbo:raze/assets/extra",
            fixture.checkout(&rev).display()
        )
    );
    assert!(
        spec.sources
            .iter()
            .any(|source| source.id.as_str() == "atlas")
    );
}
