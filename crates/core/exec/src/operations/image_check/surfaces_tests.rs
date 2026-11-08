//! Surface consumers in a root filesystem of generated ELF files.

use super::kernel_modules::{Outcome, Severity};
use super::surfaces::{InstalledSurface, check};
use gaia_plan::test_fixtures::{EM_X86_64, ElfFixture, build_elf};
use gaia_testkit::TempDir;
use std::path::Path;

/// `elf` (64-bit, little-endian, without a section table) with a `.dynsym`
/// appended: `exports` defined, `imports` undefined (`weak` ones marked).
fn with_dynsym(mut elf: Vec<u8>, exports: &[&str], imports: &[(&str, bool)]) -> Vec<u8> {
    let mut strings = vec![0u8];
    let mut entries = vec![0u8; 24];
    let mut symbol = |name: &str, info: u8, shndx: u16| {
        let offset = strings.len() as u32;
        strings.extend(name.as_bytes());
        strings.push(0);
        entries.extend(offset.to_le_bytes());
        entries.extend([info, 0]);
        entries.extend(shndx.to_le_bytes());
        entries.extend([0u8; 16]);
    };
    for name in exports {
        symbol(name, (1 << 4) | 2, 12);
    }
    for (name, weak) in imports {
        symbol(name, (if *weak { 2 } else { 1 } << 4) | 2, 0);
    }
    let entries_at = elf.len();
    elf.extend(&entries);
    let strings_at = elf.len();
    elf.extend(&strings);
    let table_at = elf.len();
    let section = |kind: u32, offset: usize, size: usize, link: u32| {
        let mut header = vec![0u8; 64];
        header[4..8].copy_from_slice(&kind.to_le_bytes());
        header[0x18..0x20].copy_from_slice(&(offset as u64).to_le_bytes());
        header[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
        header[0x28..0x2c].copy_from_slice(&link.to_le_bytes());
        header
    };
    let headers = [
        section(0, 0, 0, 0),
        section(11, entries_at, entries.len(), 2),
        section(3, strings_at, strings.len(), 0),
    ];
    for header in headers {
        elf.extend(header);
    }
    elf[0x28..0x30].copy_from_slice(&(table_at as u64).to_le_bytes());
    elf[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
    elf[0x3c..0x3e].copy_from_slice(&3u16.to_le_bytes());
    elf
}

pub(super) fn object(
    interpreter: Option<&str>,
    needed: &[&str],
    exports: &[&str],
    imports: &[(&str, bool)],
) -> Vec<u8> {
    let mut fixture = ElfFixture::dynamic(EM_X86_64, needed);
    fixture.interpreter = interpreter;
    with_dynsym(build_elf(&fixture), exports, imports)
}

pub(super) fn put(root: &Path, guest: &str, bytes: &[u8]) {
    let path = root.join(guest.trim_start_matches('/'));
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, bytes).expect("write");
}

fn findings(outcome: &Outcome, severity: Severity) -> Vec<(&'static str, Option<String>)> {
    outcome
        .findings
        .iter()
        .filter(|finding| finding.severity == severity)
        .map(|finding| (finding.code, finding.module.clone()))
        .collect()
}

#[test]
fn consumers_import_only_what_the_surface_and_their_libraries_export() {
    let scratch = TempDir::new("gaia-image-check-surfaces");
    let root = scratch.path();
    let interpreter = Some("/lib64/ld-linux-x86-64.so.2");
    put(
        root,
        "/usr/lib/libsurface.so",
        &object(
            None,
            &["libc.so.6"],
            &["surface_open", "surface_tick"],
            &[("malloc", false)],
        ),
    );
    put(
        root,
        "/usr/lib/libc.so.6",
        &object(None, &[], &["malloc", "printf"], &[]),
    );
    put(
        root,
        "/usr/bin/good",
        &object(
            interpreter,
            &["libsurface.so", "libc.so.6"],
            &[],
            &[
                ("surface_open", false),
                ("printf", false),
                ("__gmon_start__", true),
            ],
        ),
    );
    put(
        root,
        "/usr/bin/stale",
        &object(
            interpreter,
            &["libsurface.so", "libc.so.6"],
            &[],
            &[("surface_open", false), ("surface_close", false)],
        ),
    );
    put(
        root,
        "/usr/lib/libplugin.so",
        &object(None, &["libsurface.so"], &[], &[("host_callback", false)]),
    );
    put(
        root,
        "/usr/bin/orphan",
        &object(
            interpreter,
            &["libsurface.so", "libgone.so"],
            &[],
            &[("gone", false)],
        ),
    );
    put(
        root,
        "/usr/bin/unrelated",
        &object(interpreter, &["libc.so.6"], &[], &[("nowhere", false)]),
    );
    put(root, "/etc/motd", b"not ELF\n");

    let mut outcome = Outcome::default();
    let consumers = check(
        &[InstalledSurface {
            artifact: "libsurface".into(),
            guest: "/usr/lib/libsurface.so".into(),
            plugin_dirs: Vec::new(),
            plugin_exports: Vec::new(),
            plugin_version: None,
        }],
        root,
        &mut outcome,
    );
    assert_eq!(consumers, 4);
    assert_eq!(
        findings(&outcome, Severity::Error),
        [("surface_import_undeclared", Some("/usr/bin/stale".into()))]
    );
    let stale = &outcome.findings[outcome
        .findings
        .iter()
        .position(|finding| finding.severity == Severity::Error)
        .expect("the error")];
    assert!(
        stale.message.contains("imports 1 symbol(s)")
            && stale.message.ends_with(
                "export: surface_close; it was linked against another build of the surface"
            ),
        "{}",
        stale.message
    );
    assert_eq!(
        findings(&outcome, Severity::Warning),
        [
            ("surface_consumer_unchecked", Some("/usr/bin/orphan".into())),
            (
                "surface_import_undeclared",
                Some("/usr/lib/libplugin.so".into())
            ),
        ]
    );
}

/// Plugins under the surface's `plugin_dirs` are its consumers, whether or
/// not they need it by name, and their leftover imports are an error.
#[test]
fn plugins_are_checked_against_the_surface_that_opens_them() {
    let scratch = TempDir::new("gaia-image-check-surface-plugins");
    let root = scratch.path();
    put(
        root,
        "/usr/lib/libsurface.so",
        &object(None, &["libc.so.6"], &["surface_open"], &[]),
    );
    put(
        root,
        "/usr/lib/libc.so.6",
        &object(None, &[], &["malloc"], &[]),
    );
    put(
        root,
        "/usr/bin/host",
        &object(
            Some("/lib64/ld-linux-x86-64.so.2"),
            &["libsurface.so"],
            &[],
            &[("surface_open", false)],
        ),
    );
    // malloc comes from the surface's own libc.
    put(
        root,
        "/usr/lib/plugins/nested/good.so",
        &object(
            None,
            &[],
            &["plugin_descriptor"],
            &[("surface_open", false), ("malloc", false)],
        ),
    );
    put(
        root,
        "/usr/lib/plugins/stale.so",
        &object(None, &[], &[], &[("surface_gone", false)]),
    );
    // Not under the directory, only next to it by name.
    put(
        root,
        "/usr/lib/plugins-old/other.so",
        &object(None, &[], &[], &[("nowhere", false)]),
    );

    let mut outcome = Outcome::default();
    let consumers = check(
        &[InstalledSurface {
            artifact: "libsurface".into(),
            guest: "/usr/lib/libsurface.so".into(),
            plugin_dirs: vec!["/usr/lib/plugins/".into()],
            plugin_exports: vec!["plugin_descriptor".into()],
            plugin_version: None,
        }],
        root,
        &mut outcome,
    );
    assert_eq!(consumers, 3);
    assert_eq!(
        findings(&outcome, Severity::Error),
        [
            (
                "surface_plugin_export_missing",
                Some("/usr/lib/plugins/stale.so".into())
            ),
            (
                "surface_import_undeclared",
                Some("/usr/lib/plugins/stale.so".into())
            )
        ]
    );
    assert!(findings(&outcome, Severity::Warning).is_empty());
}

/// A range read from the surface: a surface without the constant is an
/// error once, and its plugins' versions are not checked.
#[test]
fn a_surface_without_its_range_constants_is_an_error() {
    let scratch = TempDir::new("gaia-image-check-surface-range");
    let root = scratch.path();
    put(
        root,
        "/usr/lib/libsurface.so",
        &object(None, &[], &["surface_open"], &[]),
    );
    for name in ["a.so", "b.so"] {
        put(
            root,
            &format!("/usr/lib/plugins/{name}"),
            &object(None, &[], &["plugin_descriptor"], &[]),
        );
    }
    let mut outcome = Outcome::default();
    check(
        &[InstalledSurface {
            artifact: "libsurface".into(),
            guest: "/usr/lib/libsurface.so".into(),
            plugin_dirs: vec!["/usr/lib/plugins".into()],
            plugin_exports: Vec::new(),
            plugin_version: Some(gaia_spec::SurfacePluginVersionSpec {
                symbol: "plugin_version".into(),
                min: Some(2),
                max: None,
                surface_min: Some("surface_abi_min".into()),
                surface_max: Some("surface_abi_max".into()),
            }),
        }],
        root,
        &mut outcome,
    );
    assert_eq!(
        findings(&outcome, Severity::Error),
        [(
            "surface_plugin_range_unreadable",
            Some("/usr/lib/libsurface.so".into())
        )]
    );
    // The fixed min wins: only the max is asked of the surface.
    assert!(
        outcome.findings[0].message.contains("surface_abi_max"),
        "{}",
        outcome.findings[0].message
    );
}

#[test]
fn an_image_without_surfaces_reads_nothing() {
    let scratch = TempDir::new("gaia-image-check-no-surfaces");
    let mut outcome = Outcome::default();
    assert_eq!(check(&[], scratch.path(), &mut outcome), 0);
    assert!(outcome.findings.is_empty());
}

/// A starting-point image: the operation checks the consumers in the
/// provider's `<collect_dir>/rootfs`, and fails on a stale one.
#[test]
fn a_starting_point_image_has_its_surface_consumers_checked() {
    use gaia_spec::{
        ArtifactDefinition, ArtifactOutputSpec, ArtifactSpec, ArtifactVariantSpec, ImageDefinition,
        InstallEntrySpec, ResolvedBuildSpec, RustArtifactSpec, RustSurfaceSpec,
    };
    let scratch = TempDir::new("gaia-image-check-starting-point");
    let mut spec = ResolvedBuildSpec::new("surface-starting-point");
    spec.workspace.root_dir = scratch.path().display().to_string();
    spec.workspace.build_dir = scratch.join("build").display().to_string();
    spec.workspace.out_dir = scratch.join("out").display().to_string();
    spec.image.definition = ImageDefinition::StartingPoint(Default::default());
    spec.image.output.collect_dir = Some(scratch.join("out/images").display().to_string());
    spec.artifacts.push(ArtifactSpec::new(
        "capi",
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: "capi".into(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
            cargo_git_sources: Vec::new(),
            build_env: Vec::new(),
            cpu: None,
            link: gaia_spec::RustLinkSpec::Static,
            lto: gaia_spec::RustLtoSpec::Profile,
            surface: Some(Box::new(RustSurfaceSpec::new("capi.exports"))),
            trim: None,
        }),
        None,
        ArtifactOutputSpec {
            path: "out/artifacts/libcapi.so".into(),
        },
    ));
    spec.install
        .entries
        .push(InstallEntrySpec::new("install-capi", "capi", "/usr/lib"));
    let root = scratch.join("out/images/rootfs");
    let interpreter = Some("/lib64/ld-linux-x86-64.so.2");
    put(
        &root,
        "/usr/lib/libcapi.so",
        &object(None, &[], &["capi_open"], &[]),
    );
    put(
        &root,
        "/usr/bin/client",
        &object(interpreter, &["libcapi.so"], &[], &[("capi_open", false)]),
    );
    let run = || super::execute(&spec, &gaia_plan::OperationId::image_check());
    let report = || -> serde_json::Value {
        let text = std::fs::read_to_string(gaia_spec::image_checks_report_path(&spec))
            .expect("the report is written");
        serde_json::from_str(&text).expect("the report is JSON")
    };

    let passed = run();
    assert!(passed.error.is_none(), "{:?}", passed.error);
    assert_eq!(report()["status"], "passed");
    assert_eq!(report()["surface_consumers"], 1);

    put(
        &root,
        "/usr/bin/stale",
        &object(interpreter, &["libcapi.so"], &[], &[("capi_reset", false)]),
    );
    let failed = run();
    let error = failed.error.expect("a stale consumer fails the check");
    assert!(
        error.message.contains("/usr/bin/stale"),
        "{}",
        error.message
    );
    assert_eq!(report()["status"], "failed");
    assert_eq!(report()["errors"][0]["code"], "surface_import_undeclared");
}

/// A real root filesystem: `GAIA_SURFACE_ROOTFS` and the surface's path in
/// it, `GAIA_SURFACE_GUEST`. Prints what the check finds.
#[test]
#[ignore = "reads a real root filesystem named by the environment"]
fn a_real_root_filesystem() {
    let root = std::env::var("GAIA_SURFACE_ROOTFS").expect("GAIA_SURFACE_ROOTFS");
    let guest = std::env::var("GAIA_SURFACE_GUEST").expect("GAIA_SURFACE_GUEST");
    let mut outcome = Outcome::default();
    let started = std::time::Instant::now();
    let consumers = check(
        &[InstalledSurface {
            artifact: "real".into(),
            guest,
            plugin_dirs: std::env::var("GAIA_SURFACE_PLUGIN_DIRS")
                .map(|dirs| dirs.split(':').map(str::to_string).collect())
                .unwrap_or_default(),
            plugin_exports: std::env::var("GAIA_SURFACE_PLUGIN_EXPORTS")
                .map(|names| names.split(',').map(str::to_string).collect())
                .unwrap_or_default(),
            plugin_version: None,
        }],
        Path::new(&root),
        &mut outcome,
    );
    println!("{consumers} consumer(s) in {:?}", started.elapsed());
    for finding in &outcome.findings {
        println!(
            "{:?} {} {}",
            finding.severity, finding.code, finding.message
        );
    }
}
