//! Generated content: text files and JSON documents in trees and archives,
//! with their `${assembly...}` tokens rendered at assembly time, the steps
//! they read run first, and RAM placement. Split from `tests.rs`.

use super::*;
use gaia_spec::{
    AssemblyArchiveMemberSpec, AssemblyArchiveSpec, AssemblyFileSpec, AssemblyJsonValue,
    AssemblyTreeSpec,
};
use gaia_testkit::TempDir;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// 1000 bytes of a pattern: the size and digest the tokens must render.
fn part_bytes() -> Vec<u8> {
    (0..1000).map(|index| (index * 13 % 251) as u8).collect()
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_assets(root: &Path) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("part.bin"), part_bytes()).expect("part");
}

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

fn state_value(summary: &AssemblyStagingSummary, key: &str) -> String {
    summary
        .state
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_else(|| panic!("state has no {key}"))
}

fn generated_file(
    dest: &str,
    content: Option<&str>,
    json: Option<AssemblyJsonValue>,
) -> AssemblyFileSpec {
    AssemblyFileSpec {
        include: Vec::new(),
        strip: false,
        exclude: Vec::new(),
        tree: "boot".into(),
        src: None,
        src_glob: None,
        content: content.map(str::to_string),
        json,
        dest: dest.into(),
        mode: None,
        optional: false,
        src_dir: None,
        preserve_symlink: false,
        allow_missing_libs: false,
    }
}

fn object(members: Vec<(&str, AssemblyJsonValue)>) -> AssemblyJsonValue {
    AssemblyJsonValue::Object(
        members
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

fn text(value: &str) -> AssemblyJsonValue {
    AssemblyJsonValue::String(value.to_string())
}

/// A boot tree, the 1000-byte part copied into the work dir, and the given
/// tree files and archives.
fn generated_assembly(
    work_dir: &str,
    files: Vec<AssemblyFileSpec>,
    archives: Vec<AssemblyArchiveSpec>,
) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some(work_dir.into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        // Declared before the part it reads: the step must still run after it.
        files,
        transforms: vec![copy_transform(
            "@assets/part.bin",
            "$assembly.work/part.bin",
        )],
        archives,
        ..ImageAssemblySpec::default()
    }
}

fn manifest_json() -> AssemblyJsonValue {
    object(vec![
        ("schema", AssemblyJsonValue::Integer(1)),
        ("id", text("slot-a")),
        (
            "image",
            object(vec![
                ("size", text("${assembly.size:$assembly.work/part.bin}")),
                ("sha256", text("${assembly.sha256:$assembly.work/part.bin}")),
            ]),
        ),
        ("trial", AssemblyJsonValue::Bool(false)),
        (
            "boot",
            object(vec![(
                "files",
                object(vec![(
                    "kernel8.img",
                    text("${assembly.sha256:$assembly.work/part.bin}"),
                )]),
            )]),
        ),
    ])
}

fn expected_manifest(sha: &str) -> String {
    format!(
        "{{\n  \"schema\": 1,\n  \"id\": \"slot-a\",\n  \"image\": {{\n    \"size\": 1000,\n    \"sha256\": \"{sha}\"\n  }},\n  \"trial\": false,\n  \"boot\": {{\n    \"files\": {{\n      \"kernel8.img\": \"{sha}\"\n    }}\n  }}\n}}\n"
    )
}

#[cfg(unix)]
#[test]
fn text_content_renders_tokens_after_the_step_that_produces_them() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = TempDir::new("gaia-exec-generated");
    let root = scratch.join("gaia-generated-text");
    write_assets(&root);
    let mut spec = test_spec(&root);
    let sha = hex_sha256(&part_bytes());
    let file = generated_file(
        "cmdline.txt",
        Some(
            "root=${assembly.size:$assembly.work/part.bin} digest=${assembly.sha256:$assembly.work/part.bin}\n",
        ),
        None,
    );
    spec.image.assembly = Some(generated_assembly("$assembly.work", vec![file], Vec::new()));

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("generated text assembly");

    let dest = PathBuf::from(state_value(&summary, "file.1.dest"));
    assert_eq!(
        fs::read_to_string(&dest).expect("cmdline"),
        format!("root=1000 digest={sha}\n")
    );
    assert_eq!(
        fs::metadata(&dest).expect("cmdline").permissions().mode() & 0o7777,
        0o644,
        "a generated file defaults to 0644 whatever the umask"
    );
    assert_eq!(state_value(&summary, "file.1.generated"), "true");
}

#[test]
fn json_documents_render_in_order_with_numbers_in_tree_and_archive() {
    let scratch = TempDir::new("gaia-exec-generated");
    let root = scratch.join("gaia-generated-json");
    write_assets(&root);
    let sha = hex_sha256(&part_bytes());
    let mut spec = test_spec(&root);
    let archive = AssemblyArchiveSpec {
        id: "bundle".into(),
        output: "$assembly.out/bundle.tar".into(),
        members: vec![AssemblyArchiveMemberSpec {
            tree: None,
            src_dir: None,
            src_glob: None,
            prefix: String::new(),
            exclude: Vec::new(),
            name: "manifest.json".into(),
            src: None,
            entries: None,
            json: Some(manifest_json()),
        }],
    };
    spec.image.assembly = Some(generated_assembly(
        "$assembly.work",
        vec![generated_file("manifest.json", None, Some(manifest_json()))],
        vec![archive],
    ));

    let first =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("json assembly");
    let tree_file = fs::read_to_string(state_value(&first, "file.1.dest")).expect("manifest");
    assert_eq!(tree_file, expected_manifest(&sha));

    let tar = fs::read(state_value(&first, "archives.1.output")).expect("bundle");
    let tar_text = String::from_utf8_lossy(&tar);
    assert!(
        tar_text.contains(&expected_manifest(&sha)),
        "the archive member holds the same document"
    );

    // Deterministic: a second run writes the same bytes.
    let second =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("second run");
    assert_eq!(
        fs::read_to_string(state_value(&second, "file.1.dest")).expect("manifest"),
        tree_file
    );
}

#[test]
fn a_token_that_names_a_missing_file_fails_the_assembly() {
    let scratch = TempDir::new("gaia-exec-generated");
    let root = scratch.join("gaia-generated-missing");
    write_assets(&root);
    let mut spec = test_spec(&root);
    let file = generated_file(
        "size.txt",
        Some("${assembly.size:$assembly.work/never-written.bin}"),
        None,
    );
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![file],
        ..ImageAssemblySpec::default()
    });
    let error =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("missing size");
    assert!(
        error.message.contains("${assembly.size:") && error.message.contains("does not exist"),
        "{}",
        error.message
    );
}

#[test]
fn generated_files_in_a_ram_work_dir_are_written_to_ram_and_render_the_same() {
    let scratch = TempDir::new("gaia-exec-generated");
    let root = scratch.join("gaia-generated-ram");
    write_assets(&root);
    let base = root.join("shm");
    fs::create_dir_all(&base).expect("ram base");
    let sha = hex_sha256(&part_bytes());
    let mut spec = test_spec(&root);
    // The archive publishes the RAM-built file, so the bytes are checked
    // after the RAM copies are gone.
    let archive = AssemblyArchiveSpec {
        id: "bundle".into(),
        output: "$assembly.out/bundle.tar".into(),
        members: vec![AssemblyArchiveMemberSpec {
            tree: None,
            src_dir: None,
            src_glob: None,
            prefix: String::new(),
            exclude: Vec::new(),
            name: "cmdline.txt".into(),
            src: Some("$assembly.tree.boot/cmdline.txt".into()),
            entries: None,
            json: None,
        }],
    };
    spec.image.assembly = Some(generated_assembly(
        "ram",
        vec![generated_file(
            "cmdline.txt",
            Some("digest=${assembly.sha256:$assembly.work/part.bin}"),
            None,
        )],
        vec![archive],
    ));
    let env = || PlacementEnv {
        ram_base: base.clone(),
        user: "tester".into(),
        memory_available: Some(16 * 1024 * 1024 * 1024),
        tmpfs_available: Some(64 * 1024 * 1024 * 1024),
    };
    let summary = stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, env)
        .expect("ram generated assembly");

    assert_eq!(state_value(&summary, "work_dir.placement"), "ram");
    let dest = PathBuf::from(state_value(&summary, "file.1.dest"));
    assert!(dest.starts_with(&base), "{}", dest.display());
    let tar = fs::read(state_value(&summary, "archives.1.output")).expect("bundle");
    assert!(
        String::from_utf8_lossy(&tar).contains(&format!("digest={sha}")),
        "the RAM-built file is published with its rendered token"
    );
}

fn tree_spec(id: &str) -> AssemblyTreeSpec {
    AssemblyTreeSpec {
        strip: false,
        id: id.into(),
        path: gaia_spec::AssemblyPathTemplate::new(format!("$assembly.work/{id}")),
    }
}

/// A file in `tree` with generated text content.
fn tree_text(tree: &str, dest: &str, content: &str) -> AssemblyFileSpec {
    let mut file = generated_file(dest, Some(content), None);
    file.tree = tree.into();
    file
}

/// `slot.json` in the boot tree, holding the digest map of `payload`.
fn slot_digest_json(payload: &str) -> AssemblyFileSpec {
    generated_file(
        "slot.json",
        None,
        Some(object(vec![(
            "boot",
            object(vec![(
                "files",
                text(&format!("${{assembly.sha256_tree:{payload}}}")),
            )]),
        )])),
    )
}

#[test]
fn a_tree_digest_map_lists_the_tree_after_its_fillers_write_it() {
    let scratch = TempDir::new("gaia-exec-generated");
    let root = scratch.join("gaia-generated-tree-digest");
    write_assets(&root);
    let mut spec = test_spec(&root);
    // The JSON is declared first; it must still run after both payload files.
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![tree_spec("boot"), tree_spec("payload")],
        files: vec![
            slot_digest_json("payload"),
            tree_text("payload", "a.txt", "alpha\n"),
            tree_text("payload", "nested/b.txt", "beta\n"),
        ],
        ..ImageAssemblySpec::default()
    });

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("tree digest assembly");

    // File steps run in dependency order: the two payload files, then the JSON.
    let dest = PathBuf::from(state_value(&summary, "file.3.dest"));
    assert!(dest.ends_with("slot.json"), "{}", dest.display());
    assert_eq!(
        fs::read_to_string(&dest).expect("slot.json"),
        format!(
            "{{\n  \"boot\": {{\n    \"files\": {{\n      \"a.txt\": \"{}\",\n      \"nested/b.txt\": \"{}\"\n    }}\n  }}\n}}\n",
            hex_sha256(b"alpha\n"),
            hex_sha256(b"beta\n")
        )
    );
}

#[test]
fn an_empty_tree_digest_renders_an_empty_object_and_text_cannot_use_one() {
    let scratch = TempDir::new("gaia-exec-generated");
    let root = scratch.join("gaia-generated-tree-digest-empty");
    write_assets(&root);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![tree_spec("boot"), tree_spec("empty")],
        dirs: vec![gaia_spec::AssemblyDirSpec {
            tree: "empty".into(),
            path: ".".into(),
            mode: None,
        }],
        files: vec![generated_file(
            "slot.json",
            None,
            Some(object(vec![(
                "none",
                text("${assembly.sha256_tree:empty}"),
            )])),
        )],
        ..ImageAssemblySpec::default()
    });
    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("empty tree digest");
    let dest = PathBuf::from(state_value(&summary, "file.1.dest"));
    assert_eq!(
        fs::read_to_string(&dest).expect("slot.json"),
        "{\n  \"none\": {}\n}\n"
    );

    // In text the token is an error, whatever the tree holds.
    let mut text_spec = test_spec(&root);
    text_spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![tree_spec("boot"), tree_spec("empty")],
        files: vec![generated_file(
            "cmdline.txt",
            Some("slots=${assembly.sha256_tree:empty}"),
            None,
        )],
        ..ImageAssemblySpec::default()
    });
    let error = stage_image_assembly_with(
        &text_spec,
        &OperationId::image_assembly(),
        None,
        disk_env_panic,
    )
    .expect_err("tree digest in text");
    assert!(
        error.message.contains("whole JSON value"),
        "{}",
        error.message
    );
}

/// Text written to a .json file must parse once its tokens are filled in:
/// a TOML '...' literal string keeps a backslash-n, which once shipped an
/// invalid state.json. Valid JSON text with tokens is written as is.
#[test]
fn text_content_for_a_json_file_must_be_json() {
    let scratch = TempDir::new("gaia-exec-generated-json-text");
    let root = scratch.join("gaia-generated-json-text");
    write_assets(&root);
    let mut spec = test_spec(&root);
    let broken = generated_file(
        "state.json",
        Some("{\"schema\":1,\"previous\":null}\\n"),
        None,
    );
    spec.image.assembly = Some(generated_assembly(
        "$assembly.work",
        vec![broken],
        Vec::new(),
    ));
    let error =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect_err("a literal backslash-n is not JSON");
    let message = format!("{error:?}");
    assert!(
        message.contains("state.json") && message.contains("not valid JSON"),
        "{message}"
    );

    let valid = generated_file(
        "state.json",
        Some("{\"size\":${assembly.size:$assembly.work/part.bin},\"previous\":null}\n"),
        None,
    );
    spec.image.assembly = Some(generated_assembly(
        "$assembly.work",
        vec![valid],
        Vec::new(),
    ));
    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("valid JSON text");
    let dest = PathBuf::from(state_value(&summary, "file.1.dest"));
    assert_eq!(
        fs::read_to_string(dest).expect("state.json"),
        "{\"size\":1000,\"previous\":null}\n"
    );
}
