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
        rust.surface = Some(gaia_spec::RustSurfaceSpec {
            exports: "surface.exports".into(),
        });
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
        rust.surface = Some(gaia_spec::RustSurfaceSpec {
            exports: " ".into(),
        });
    }
    let codes = RustProvider
        .validate_artifact(&artifact)
        .into_iter()
        .map(|issue| issue.code)
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        ["rust_surface_not_a_library", "rust_surface_exports_empty"]
    );
}
