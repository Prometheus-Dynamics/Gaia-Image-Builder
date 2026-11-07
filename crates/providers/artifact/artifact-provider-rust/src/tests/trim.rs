//! A trimmed library: the group's cargo build links it through Gaia's
//! wrapper, and the library is relinked exporting only what its users
//! import.

use super::*;
use gaia_testkit::TempDir;

/// A Rust `dylib` exporting two C functions and a binary calling one.
fn library_and_user_workspace(prefix: &str) -> TempDir {
    let scratch = TempDir::new(prefix);
    let root = scratch.path().to_path_buf();
    let write = |path: &str, contents: &str| {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, contents).expect("write");
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"core\", \"app\"]\nresolver = \"2\"\n",
    );
    write(
        "core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"dylib\"]\n",
    );
    write(
        "core/src/lib.rs",
        "#[no_mangle]\npub extern \"C\" fn core_used(x: u32) -> u32 { x.wrapping_mul(7) }\n#[no_mangle]\npub extern \"C\" fn core_unused(n: u64) -> u64 { (0..n).map(|x| x * x).sum() }\n",
    );
    write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    write(
        "app/src/main.rs",
        "fn main() { println!(\"{}\", core::core_used(std::env::args().count() as u32)); }\n",
    );
    scratch
}

fn member(
    id: &str,
    target_name: &str,
    output: &Path,
    trim: gaia_spec::RustTrimSpec,
) -> ArtifactSpec {
    let mut artifact = rust_artifact(id, id, output);
    if let ArtifactDefinition::Rust(rust) = &mut artifact.definition {
        rust.target_name = Some(target_name.into());
        rust.build_group = Some("stack".into());
        rust.group_packages = vec!["app".into(), "core".into()];
        rust.link = gaia_spec::RustLinkSpec::Shared;
        rust.trim = Some(Box::new(trim));
    }
    artifact
}

#[test]
fn a_trimmed_library_exports_only_what_its_users_import() {
    let scratch = library_and_user_workspace("gaia-rust-trim");
    let workspace = scratch.path();
    let recorded = vec!["libcore.so".to_string()];
    let core = member(
        "core",
        "libcore.so",
        &workspace.join("out/libcore.so"),
        gaia_spec::RustTrimSpec {
            users: vec!["app".into()],
            user_files: vec!["app".into()],
            recorded: recorded.clone(),
        },
    );
    let app = member(
        "app",
        "app",
        &workspace.join("out/app"),
        gaia_spec::RustTrimSpec {
            recorded,
            ..Default::default()
        },
    );
    let core_contract = nested_contract(&core, workspace);
    let app_contract = nested_contract(&app, workspace);
    let results = RustProvider.execute_artifact_batch(
        &[
            ArtifactBatchItem {
                artifact: &core,
                contract: &core_contract,
                log_sink: None,
            },
            ArtifactBatchItem {
                artifact: &app,
                contract: &app_contract,
                log_sink: None,
            },
        ],
        None,
    );
    let messages = results
        .into_iter()
        .map(|result| result.expect("the group builds"))
        .collect::<Vec<_>>();
    assert!(
        messages[0]
            .iter()
            .any(|message| message.contains("trimmed for 1 user(s)")),
        "{messages:?}"
    );

    let exports = |path: &Path| {
        gaia_artifact_providers::dynamic_symbols(&fs::read(path).expect("read"))
            .expect("symbols")
            .exports
    };
    let target = workspace.join(".gaia/cargo-target-linked");
    assert!(target.join("gaia-link/libcore.so/args").is_file());
    assert!(
        target.join("gaia-link/bin/cc").is_file(),
        "named after the linker"
    );
    let full = exports(&target.join("debug/libcore.so"));
    assert!(full.contains("core_unused"), "{full:?}");
    let trimmed = exports(&workspace.join("out/libcore.so"));
    assert!(trimmed.contains("core_used"), "{trimmed:?}");
    assert!(!trimmed.contains("core_unused"), "{trimmed:?}");
    assert!(trimmed.len() < full.len());
}
