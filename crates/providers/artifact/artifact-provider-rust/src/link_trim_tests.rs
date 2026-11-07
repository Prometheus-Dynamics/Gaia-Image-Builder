use super::*;
use std::collections::BTreeSet;

#[test]
fn the_script_keeps_imported_names_and_the_crate_metadata() {
    let script =
        "{\n  global:\n    used;\n    unused;\n    rust_metadata_core_1;\n\n  local:\n    *;\n};\n";
    let keep = BTreeSet::from(["used".to_string()]);
    assert_eq!(
        trimmed_script(script, &keep),
        "{\n  global:\n    used;\n    rust_metadata_core_1;\n\n  local:\n    *;\n};\n"
    );
}

#[test]
fn the_replay_points_at_the_record_and_collects_garbage() {
    let record = LinkRecord {
        dir: PathBuf::from("/t/gaia-link/libcore.so"),
        linker: "cc".into(),
        args: [
            "-Wl,--version-script=/t/debug/deps/rustcAB/list",
            "/t/debug/deps/rustcAB/symbols.o",
            "/t/debug/deps/core.core.1-cgu.0.rcgu.o",
            "-L",
            "/t/debug/deps/rustcAB/raw-dylibs",
            "-o",
            "/t/debug/deps/libcore.so",
            "-shared",
        ]
        .map(String::from)
        .to_vec(),
        rustc_dir: "/t/debug/deps/rustcAB".into(),
    };
    assert_eq!(
        replay_args(
            &record,
            Path::new("/m/trim.map"),
            Path::new("/o/libcore.so")
        ),
        [
            "-Wl,--version-script=/m/trim.map",
            "/t/gaia-link/libcore.so/rustc/symbols.o",
            "/t/gaia-link/libcore.so/objects/core.core.1-cgu.0.rcgu.o",
            "-L",
            "/t/gaia-link/libcore.so/rustc/raw-dylibs",
            "-o",
            "/o/libcore.so",
            "-shared",
            "-Wl,--gc-sections",
        ]
    );
}
