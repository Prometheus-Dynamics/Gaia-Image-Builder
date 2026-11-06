//! A `cdylib` with a declared surface, built for real: the check passes on
//! the exports the library has and names the ones that differ.

use super::*;
use gaia_testkit::TempDir;

/// A one-crate workspace whose `cdylib` exports `surface_open` and
/// `surface_tick`; `helper` is a Rust function, never exported.
fn surface_workspace(exports: &str) -> TempDir {
    let scratch = TempDir::new("gaia-rust-surface");
    let root = scratch.path().to_path_buf();
    let write = |path: &str, contents: &str| {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, contents).expect("write");
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"surface\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[workspace]\n",
    );
    write(
        "src/lib.rs",
        "pub fn helper() -> u32 { 7 }\n#[no_mangle]\npub extern \"C\" fn surface_open() -> u32 { helper() }\n#[no_mangle]\npub extern \"C\" fn surface_tick(frame: u32) -> u32 { frame + 1 }\n",
    );
    write("surface.exports", exports);
    scratch
}

fn surface_artifact(workspace: &Path) -> ArtifactSpec {
    let mut artifact = rust_artifact("surface", "surface", &workspace.join("out/libsurface.so"));
    if let ArtifactDefinition::Rust(rust) = &mut artifact.definition {
        rust.target_name = Some("libsurface.so".into());
        rust.surface = Some(Box::new(gaia_spec::RustSurfaceSpec::new("surface.exports")));
    }
    artifact
}

#[test]
fn a_built_cdylib_is_checked_against_its_declared_surface() {
    let scratch = surface_workspace("# the surface\nsurface_open\nsurface_tick\n");
    let workspace = scratch.path();
    let artifact = surface_artifact(workspace);
    let contract = nested_contract(&artifact, workspace);
    let messages = RustProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect("the library exports its surface");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("exports its declared surface: 2 symbol(s)")),
        "{messages:?}"
    );

    // The same library against a list that lost one name and gained one.
    fs::write(
        workspace.join("surface.exports"),
        "SURFACE_1 {\n  global: surface_open; surface_close;\n  local: *;\n};\n",
    )
    .expect("rewrite");
    let error = crate::surface::check(&artifact, &contract).expect_err("the surface differs");
    assert!(
        error
            .message
            .contains("1 undeclared export(s): surface_tick"),
        "{}",
        error.message
    );
    assert!(
        error
            .message
            .contains("1 declared but not exported: surface_close"),
        "{}",
        error.message
    );
}

#[test]
fn a_surface_needs_one_library_file() {
    let scratch = TempDir::new("gaia-rust-surface-validate");
    let mut artifact = surface_artifact(scratch.path());
    assert!(RustProvider.validate_artifact(&artifact).is_empty());
    if let ArtifactDefinition::Rust(rust) = &mut artifact.definition {
        rust.variant = ArtifactVariantSpec::Directory;
        let mut surface = gaia_spec::RustSurfaceSpec::new(" ");
        surface.shared_feature = Some(String::new());
        rust.surface = Some(Box::new(surface));
    }
    let codes = RustProvider
        .validate_artifact(&artifact)
        .into_iter()
        .map(|issue| issue.code)
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        [
            "rust_surface_not_a_library",
            "rust_surface_exports_empty",
            "rust_surface_name_empty"
        ]
    );
}

/// With a surface in the build, each cargo build records the crates it
/// linked and their features, for the image checks.
#[test]
fn a_build_records_its_crates_and_their_features() {
    let scratch = TempDir::new("gaia-rust-crates-record");
    let root = scratch.path().to_path_buf();
    let write = |path: &str, contents: &str| {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, contents).expect("write");
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nstate-crate = { path = \"state\", features = [\"shared\"] }\n\n[workspace]\n",
    );
    write(
        "src/main.rs",
        "fn main() { println!(\"{}\", state_crate::value()); }\n",
    );
    write(
        "state/Cargo.toml",
        "[package]\nname = \"state-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nshared = []\n",
    );
    write("state/src/lib.rs", "pub fn value() -> u32 { 7 }\n");
    let artifact = rust_artifact("app", "app", &root.join("out/app"));
    let mut contract = nested_contract(&artifact, &root);
    contract.rust_record_crates = true;
    RustProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect("the program builds");

    let record = fs::read_to_string(gaia_spec::rust_crates_record_path(&root.join("out/app")))
        .expect("the crates are recorded");
    let record: serde_json::Value = serde_json::from_str(&record).expect("JSON");
    assert_eq!(record["package"], "app");
    let crates = record["crates"].as_array().expect("crates");
    let state = crates
        .iter()
        .find(|entry| entry["name"] == "state-crate")
        .expect("the dependency is recorded");
    assert_eq!(state["features"], serde_json::json!(["shared"]));
}
