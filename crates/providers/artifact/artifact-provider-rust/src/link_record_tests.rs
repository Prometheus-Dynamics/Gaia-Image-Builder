use super::*;
use gaia_testkit::TempDir;

#[test]
fn a_response_file_unescapes_each_line() {
    assert_eq!(
        response_args("-o\n/a\\ b/libx.so\n-Wl,--as-needed\n"),
        ["-o", "/a b/libx.so", "-Wl,--as-needed"]
    );
}

#[test]
fn the_linker_comes_from_the_build_then_the_nearest_cargo_config() {
    let scratch = TempDir::new("gaia-link-record-config");
    let root = scratch.path();
    let source = root.join("backend/app");
    fs::create_dir_all(&source).expect("source");
    fs::create_dir_all(root.join("backend/.cargo")).expect("config dir");
    fs::write(
        root.join("backend/.cargo/config.toml"),
        "[target.aarch64-unknown-linux-gnu]\nlinker = \"tools/link.sh\"\n[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\n",
    )
    .expect("config");
    let variable = "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER";
    assert_eq!(
        original_linker(
            variable,
            "aarch64-unknown-linux-gnu",
            &source,
            &[(variable.into(), "/sdk/bin/gcc".into())],
            Some("aarch64-linux-gnu-gcc".into())
        ),
        "/sdk/bin/gcc"
    );
    assert_eq!(
        original_linker(
            variable,
            "aarch64-unknown-linux-gnu",
            &source,
            &[(variable.into(), "tools/link.sh".into())],
            None
        ),
        source.join("tools/link.sh").display().to_string(),
        "relative to where cargo runs"
    );
    assert_eq!(
        original_linker(
            variable,
            "aarch64-unknown-linux-gnu",
            &source,
            &[],
            Some("aarch64-linux-gnu-gcc".into())
        ),
        "aarch64-linux-gnu-gcc",
        "the image's variable wins over the cargo config"
    );
    assert_eq!(
        original_linker(variable, "aarch64-unknown-linux-gnu", &source, &[], None),
        root.join("backend/tools/link.sh").display().to_string()
    );
    assert_eq!(
        config_linker("aarch64-unknown-linux-gnu", &source),
        Some(root.join("backend/tools/link.sh").display().to_string())
    );
    assert_eq!(
        config_linker("x86_64-unknown-linux-gnu", &source),
        Some("clang".into())
    );
}

#[test]
fn a_record_reads_its_linker_arguments_and_rustc_dir() {
    let scratch = TempDir::new("gaia-link-record-read");
    let dir = scratch.path().join(LINK_DIR).join("libcore.so");
    fs::create_dir_all(&dir).expect("record");
    fs::write(dir.join("args"), b"-o\0/t/libcore.so\0").expect("args");
    fs::write(dir.join("linker"), "clang\n").expect("linker");
    fs::write(dir.join("rustc-dir"), "/t/rustcAB\n").expect("rustc dir");
    let record = read_record(scratch.path(), "libcore.so").expect("record");
    assert_eq!(record.args, ["-o", "/t/libcore.so"]);
    assert_eq!(record.linker, "clang");
    assert_eq!(record.rustc_dir, "/t/rustcAB");
    assert!(read_record(scratch.path(), "libother.so").is_err());
}
