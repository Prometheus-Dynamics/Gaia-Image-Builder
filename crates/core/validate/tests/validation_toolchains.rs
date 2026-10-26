//! Validation of the archive `url` form, `[[toolchains]]`, the artifacts that
//! name a toolchain, and `[image.assembly.packages.device]`.

pub mod support;

use gaia_config::try_resolve_config;
use gaia_validate::validate_spec;
use std::fs;

use support::create_temp_workspace;

const SHA: &str = "3c9f1ab2f8c7ce1d6e4e4f8a0b5c9d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6c";

/// The diagnostic codes a config raises, resolved and validated.
fn codes(name: &str, body: &str) -> Vec<String> {
    let root = create_temp_workspace(name);
    let build = root.join("build.toml");
    fs::write(
        &build,
        format!(
            r#"
build_name = "{name}"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

{body}
"#
        ),
    )
    .expect("build file");
    let spec = try_resolve_config(&build.display().to_string()).expect("resolve");
    validate_spec(&spec)
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// A kind-none body with one package; `device` is the package's device table
/// (empty for none).
fn package_body(device: &str) -> String {
    format!(
        r#"
[[sources]]
id = "files"
kind = "path"
path = "files"

[image]
kind = "none"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"
out_dir = "${{workspace.out_dir}}"

[[image.assembly.trees]]
id = "pkg-root"
path = "$assembly.work/pkg-root"

[[image.assembly.packages]]
id = "tool"
tree = "pkg-root"
dest = "$assembly.out/"

[image.assembly.packages.control]
Package = "tool"
Version = "1.0.0"
Architecture = "aarch64"
Maintainer = "Test <test@example.invalid>"
Description = "A tool."

{device}
"#
    )
}

fn sdk_source() -> String {
    r#"
[[sources]]
id = "sdk"
kind = "path"
path = "sdk"
"#
    .to_string()
}

fn toolchain(extra: &str) -> String {
    format!(
        r#"
[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
triple = "aarch64-buildroot-linux-gnu"
sysroot = "aarch64-buildroot-linux-gnu/sysroot"
{extra}
"#
    )
}

#[test]
fn a_valid_url_archive_and_toolchain_raise_no_toolchain_diagnostics() {
    let codes = codes(
        "valid-toolchain",
        &format!(
            r#"
[[sources]]
id = "sdk-archive"
kind = "archive"
url = "https://example.invalid/sdk.tar.gz"
sha256 = "{SHA}"
strip_components = 1
{}
[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
source = "sdk-archive"
triple = "aarch64-buildroot-linux-gnu"
sysroot = "aarch64-buildroot-linux-gnu/sysroot"
rust_target = "aarch64-unknown-linux-gnu"

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
toolchain = "systemcore"
output_path = "out/app"
"#,
            ""
        ),
    );
    let ours: Vec<&String> = codes
        .iter()
        .filter(|code| code.starts_with("archive_") || code.contains("toolchain"))
        .collect();
    assert!(ours.is_empty(), "{ours:?}");
}

#[test]
fn an_archive_url_needs_a_checksum_and_no_path() {
    let missing = codes(
        "archive-missing-sha",
        r#"
[[sources]]
id = "bundle"
kind = "archive"
url = "https://example.invalid/bundle.tar.gz"
strip_components = 0
"#,
    );
    assert!(
        missing.contains(&"archive_sha256_missing".into()),
        "{missing:?}"
    );

    let both = codes(
        "archive-path-and-url",
        &format!(
            r#"
[[sources]]
id = "bundle"
kind = "archive"
path = "bundle.tar.gz"
url = "https://example.invalid/bundle.tar.gz"
sha256 = "{SHA}"
strip_components = 0
"#
        ),
    );
    assert!(both.contains(&"archive_path_and_url".into()), "{both:?}");

    let short = codes(
        "archive-short-sha",
        r#"
[[sources]]
id = "bundle"
kind = "archive"
url = "https://example.invalid/bundle.tar.gz"
sha256 = "abc123"
strip_components = 0
"#,
    );
    assert!(
        short.contains(&"archive_sha256_invalid".into()),
        "{short:?}"
    );

    let scheme = codes(
        "archive-ftp",
        &format!(
            r#"
[[sources]]
id = "bundle"
kind = "archive"
url = "ftp://example.invalid/bundle.tar.gz"
sha256 = "{SHA}"
strip_components = 0
"#
        ),
    );
    assert!(scheme.contains(&"archive_url_invalid".into()), "{scheme:?}");

    let sha_on_path = codes(
        "archive-sha-on-path",
        &format!(
            r#"
[[sources]]
id = "bundle"
kind = "archive"
path = "bundle.tar.gz"
sha256 = "{SHA}"
strip_components = 0
"#
        ),
    );
    assert!(
        sha_on_path.contains(&"archive_sha256_without_url".into()),
        "{sha_on_path:?}"
    );
}

#[test]
fn a_toolchain_needs_one_root_one_sysroot_and_a_triple() {
    let neither_root = codes("toolchain-no-root", &toolchain(""));
    assert!(
        neither_root.contains(&"toolchain_root_missing".into()),
        "{neither_root:?}"
    );

    let both_roots = codes(
        "toolchain-both-roots",
        &format!(
            "{}\n{}",
            sdk_source(),
            toolchain("source = \"sdk\"\npath = \"sdk\"")
        ),
    );
    assert!(
        both_roots.contains(&"toolchain_root_conflict".into()),
        "{both_roots:?}"
    );

    let undefined = codes(
        "toolchain-undefined-source",
        &toolchain("source = \"nope\""),
    );
    assert!(
        undefined.contains(&"toolchain_source_undefined".into()),
        "{undefined:?}"
    );

    let no_triple = codes(
        "toolchain-no-triple",
        r#"
[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
path = "sdk"
sysroot = "sysroot"
"#,
    );
    assert!(
        no_triple.contains(&"toolchain_triple_missing".into()),
        "{no_triple:?}"
    );

    let no_sysroot = codes(
        "toolchain-no-sysroot",
        r#"
[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
path = "sdk"
triple = "aarch64-buildroot-linux-gnu"
"#,
    );
    assert!(
        no_sysroot.contains(&"toolchain_sysroot_missing".into()),
        "{no_sysroot:?}"
    );
}

#[test]
fn toolchains_belong_to_rust_artifacts_and_each_needs_a_target() {
    let non_rust = codes(
        "toolchain-on-node",
        r#"
[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
path = "sdk"
triple = "aarch64-buildroot-linux-gnu"
sysroot = "sysroot"

[[artifacts]]
id = "web"
kind = "node"
package_dir = "frontend"
toolchain = "systemcore"
output_path = "out/web.tgz"
"#,
    );
    assert!(
        non_rust.contains(&"artifact_toolchain_non_rust".into()),
        "{non_rust:?}"
    );

    let unknown = codes(
        "toolchain-unknown-id",
        r#"
[[artifacts]]
id = "app"
kind = "rust"
package = "app"
toolchain = "ghost"
output_path = "out/app"
"#,
    );
    assert!(
        unknown.contains(&"artifact_toolchain_unknown".into()),
        "{unknown:?}"
    );

    let no_target = codes(
        "toolchain-no-target",
        r#"
[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
path = "sdk"
triple = "aarch64-buildroot-linux-gnu"
sysroot = "sysroot"

[[artifacts]]
id = "app"
kind = "rust"
package = "app"
toolchain = "systemcore"
output_path = "out/app"
"#,
    );
    assert!(
        no_target.contains(&"artifact_toolchain_target_missing".into()),
        "{no_target:?}"
    );
}

#[test]
fn a_device_table_names_one_sysroot_and_well_formed_limits() {
    let conflict = codes(
        "device-conflict",
        &package_body(
            r#"[image.assembly.packages.device]
toolchain = "systemcore"
sysroot = "sysroot"
"#,
        ),
    );
    assert!(
        conflict.contains(&"package_device_sysroot_conflict".into()),
        "{conflict:?}"
    );

    let none = codes(
        "device-none",
        &package_body("[image.assembly.packages.device]\n"),
    );
    assert!(
        none.contains(&"package_device_sysroot_missing".into()),
        "{none:?}"
    );

    let unknown = codes(
        "device-unknown-toolchain",
        &package_body("[image.assembly.packages.device]\ntoolchain = \"ghost\"\n"),
    );
    assert!(
        unknown.contains(&"package_device_toolchain_unknown".into()),
        "{unknown:?}"
    );

    let glibc = codes(
        "device-bad-glibc",
        &package_body(
            "[image.assembly.packages.device]\nsysroot = \"sysroot\"\nmax_glibc = \"two\"\n",
        ),
    );
    assert!(
        glibc.contains(&"package_device_max_glibc_invalid".into()),
        "{glibc:?}"
    );

    let page = codes(
        "device-bad-page",
        &package_body(
            "[image.assembly.packages.device]\nsysroot = \"sysroot\"\nmin_page_size = 3000\n",
        ),
    );
    assert!(
        page.contains(&"package_device_min_page_size_invalid".into()),
        "{page:?}"
    );

    let fine = codes(
        "device-fine",
        &package_body(
            "[image.assembly.packages.device]\nsysroot = \"sysroot\"\nmax_glibc = \"2.17\"\n",
        ),
    );
    assert!(
        !fine.iter().any(|code| code.starts_with("package_device_")),
        "{fine:?}"
    );
}

#[test]
fn a_toolchain_page_size_is_a_power_of_two_of_at_least_4_kib() {
    let bad = codes(
        "toolchain-bad-page",
        &toolchain("path = \"sdk\"\npage_size = 3000"),
    );
    assert!(
        bad.contains(&"toolchain_page_size_invalid".into()),
        "{bad:?}"
    );

    let small = codes(
        "toolchain-small-page",
        &toolchain("path = \"sdk\"\npage_size = 2048"),
    );
    assert!(
        small.contains(&"toolchain_page_size_invalid".into()),
        "{small:?}"
    );

    let fine = codes(
        "toolchain-good-page",
        &toolchain("path = \"sdk\"\npage_size = 16384"),
    );
    assert!(
        !fine.contains(&"toolchain_page_size_invalid".into()),
        "{fine:?}"
    );
}
