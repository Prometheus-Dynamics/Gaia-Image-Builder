//! The `[[image.assembly.packages]]` output: the ipk's member order, its
//! control and data, determinism, and the checks a written package must
//! pass. The host's `gzip` and `ar` read the output in the ordinary tests;
//! the ignored test also runs `dpkg-deb`. Split from `tests.rs`.

use super::super::package_archive::{list_tar, read_ar, read_tar_entry};
use super::super::package_control::parse_control;
use super::*;
use gaia_spec::{
    AssemblyFileSpec, AssemblyPackageControlSpec, AssemblyPackageScriptsSpec, AssemblyPackageSpec,
    AssemblyTreeSpec, ImageAssemblySpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs a host tool and returns its stdout; a failing tool fails the test.
fn tool(program: &str, args: &[&str]) -> Vec<u8> {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {program}: {error}"));
    assert!(
        output.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn state_value(summary: &AssemblyStagingSummary, key: &str) -> String {
    summary
        .state
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_else(|| panic!("state has no {key}"))
}

fn disk_env_panic() -> PlacementEnv {
    panic!("a package reads no RAM facts")
}

fn run(spec: &ResolvedBuildSpec) -> AssemblyStagingSummary {
    stage_image_assembly_with(spec, &OperationId::image_assembly(), None, disk_env_panic)
        .expect("package assembly")
}

/// The error a run fails with.
fn run_error(spec: &ResolvedBuildSpec) -> String {
    match stage_image_assembly_with(spec, &OperationId::image_assembly(), None, disk_env_panic) {
        Ok(_) => panic!("the package assembly was expected to fail"),
        Err(error) => error.message,
    }
}

/// The inputs of one test: the files of the `pkg-root` tree.
struct Fixture {
    root: PathBuf,
    script: PathBuf,
    unit: PathBuf,
    conffile: PathBuf,
}

const SCRIPT: &[u8] = b"#!/bin/sh\necho helios\n";
const UNIT: &[u8] = b"[Unit]\nDescription=Helios API\n\n[Service]\nExecStart=/usr/local/bin/helios-api\n\n[Install]\nWantedBy=multi-user.target\n";
const CONFFILE: &[u8] = b"port = 5801\n";

fn fixture(root: &Path) -> Fixture {
    fs::create_dir_all(root.join("assets")).expect("assets");
    let write = |name: &str, bytes: &[u8]| {
        let path = root.join("assets").join(name);
        fs::write(&path, bytes).expect("asset");
        path
    };
    Fixture {
        root: root.to_path_buf(),
        script: write("helios-api", SCRIPT),
        unit: write("helios-api.service", UNIT),
        conffile: write("startup.toml", CONFFILE),
    }
}

fn file_entry(src: &Path, dest: &str, mode: &str) -> AssemblyFileSpec {
    AssemblyFileSpec {
        tree: "pkg-root".into(),
        src: Some(src.display().to_string().into()),
        src_glob: None,
        exclude: Vec::new(),
        include: Vec::new(),
        strip: false,
        content: None,
        json: None,
        dest: dest.into(),
        mode: Some(mode.into()),
        dir_mode: None,
        optional: false,
        src_dir: None,
        preserve_symlink: false,
        allow_missing_libs: false,
    }
}

fn control() -> AssemblyPackageControlSpec {
    AssemblyPackageControlSpec {
        package: "helios".into(),
        version: "2.0.0".into(),
        architecture: "aarch64".into(),
        maintainer: "Gaia Tests <tests@example.invalid>".into(),
        description: "Helios SystemCore services\nThe API and the runtime.\n\nSecond paragraph."
            .into(),
        depends: vec!["libc6 (>= 2.38)".into()],
        section: Some("base".into()),
        priority: Some("optional".into()),
        source: None,
        homepage: None,
        extra: vec![("X-Port".into(), "5801".into())],
    }
}

/// A package of the `pkg-root` tree, written to `dest` (a directory ending
/// in `/`, so the file is named `helios_2.0.0_aarch64.ipk`).
fn package(
    dest: &str,
    debian_binary: bool,
    allowed_prefixes: &[&str],
    services: &[&str],
) -> AssemblyPackageSpec {
    let control = control();
    AssemblyPackageSpec {
        id: "helios".into(),
        format: "ipk".into(),
        tree: "pkg-root".into(),
        dest: dest.into(),
        output: AssemblyPackageSpec::output_for(dest, &control, "ipk").into(),
        debian_binary,
        allowed_prefixes: allowed_prefixes.iter().map(|p| p.to_string()).collect(),
        control,
        scripts: AssemblyPackageScriptsSpec {
            services: services.iter().map(|s| s.to_string()).collect(),
            postinst: None,
            preinst: None,
            prerm: None,
            postrm: None,
            conffiles: vec!["/etc/helios/startup.toml".into()],
        },
        device: None,
    }
}

/// A build whose `pkg-root` tree holds the script under /usr/local, the unit
/// under /etc/systemd/system and the conffile under /etc/helios.
fn package_spec(fixture: &Fixture, package: AssemblyPackageSpec) -> ResolvedBuildSpec {
    let mut spec = test_spec(&fixture.root);
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "pkg-root".into(),
            path: "$assembly.work/pkg-root".into(),
            strip: false,
        }],
        files: vec![
            file_entry(&fixture.script, "usr/local/bin/helios-api", "0755"),
            file_entry(
                &fixture.unit,
                "etc/systemd/system/helios-api.service",
                "0644",
            ),
            file_entry(&fixture.conffile, "etc/helios/startup.toml", "0644"),
        ],
        packages: vec![package],
        ..ImageAssemblySpec::default()
    });
    spec
}

/// The uncompressed tar held in member `name` of the ipk, under `scratch`.
fn member_tar(ipk: &Path, name: &str, scratch: &Path) -> PathBuf {
    fs::create_dir_all(scratch).expect("scratch dir");
    let ipk = ipk.to_str().expect("utf-8 path");
    let gz = scratch.join(name);
    fs::write(&gz, tool("ar", &["p", ipk, name])).expect("member");
    let tar = scratch.join(format!("{name}.tar"));
    fs::write(
        &tar,
        tool("gzip", &["-dc", gz.to_str().expect("utf-8 path")]),
    )
    .expect("gunzipped member");
    tar
}

fn entry_names(tar: &Path) -> Vec<String> {
    list_tar(tar)
        .expect("tar listing")
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

#[cfg(unix)]
#[test]
fn members_are_in_order_with_and_without_debian_binary() {
    let scratch = TempDir::new("gaia-exec-package-members");
    let root = scratch.join("members");
    let fixture = fixture(&root);

    let with = run(&package_spec(
        &fixture,
        package("$assembly.out/with/", true, &[], &["helios-api.service"]),
    ));
    let ipk = PathBuf::from(state_value(&with, "package.1.output"));
    let names: Vec<String> = read_ar(&ipk)
        .expect("ar")
        .into_iter()
        .map(|member| member.name)
        .collect();
    assert_eq!(names, ["debian-binary", "control.tar.gz", "data.tar.gz"]);
    assert_eq!(
        tool("ar", &["p", ipk.to_str().expect("utf-8"), "debian-binary"]),
        b"2.0\n"
    );

    let without = run(&package_spec(
        &fixture,
        package(
            "$assembly.out/without/",
            false,
            &[],
            &["helios-api.service"],
        ),
    ));
    let ipk = PathBuf::from(state_value(&without, "package.1.output"));
    let names: Vec<String> = read_ar(&ipk)
        .expect("ar")
        .into_iter()
        .map(|member| member.name)
        .collect();
    assert_eq!(names, ["control.tar.gz", "data.tar.gz"]);
}

#[cfg(unix)]
#[test]
fn control_parses_back_with_the_spec_fields_and_the_installed_size() {
    let scratch = TempDir::new("gaia-exec-package-control");
    let root = scratch.join("control");
    let fixture = fixture(&root);
    let summary = run(&package_spec(
        &fixture,
        package("$assembly.out/", true, &[], &["helios-api.service"]),
    ));
    let ipk = PathBuf::from(state_value(&summary, "package.1.output"));
    assert!(ipk.ends_with("helios_2.0.0_aarch64.ipk"), "{ipk:?}");

    let control_tar = member_tar(&ipk, "control.tar.gz", &scratch.join("parts"));
    let entries = list_tar(&control_tar).expect("control tar");
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "./",
            "./control",
            "./conffiles",
            "./postinst",
            "./prerm",
            "./postrm"
        ]
    );
    let entry = |name: &str| {
        entries
            .iter()
            .find(|entry| entry.name == name)
            .expect("entry")
    };
    let text = String::from_utf8(read_tar_entry(&control_tar, entry("./control")).expect("bytes"))
        .expect("utf-8");
    let fields = parse_control(&text).expect("control parses");
    let value = |name: &str| {
        fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("no {name}"))
    };
    assert_eq!(value("Package"), "helios");
    assert_eq!(value("Version"), "2.0.0");
    assert_eq!(value("Architecture"), "aarch64");
    assert_eq!(value("Depends"), "libc6 (>= 2.38)");
    assert_eq!(value("X-Port"), "5801");
    assert_eq!(value("Section"), "base");
    assert_eq!(
        value("Description"),
        "Helios SystemCore services\nThe API and the runtime.\n.\nSecond paragraph."
    );
    // The installed files' bytes, rounded up to KiB.
    let bytes = (SCRIPT.len() + UNIT.len() + CONFFILE.len()) as u64;
    assert_eq!(value("Installed-Size"), bytes.div_ceil(1024).to_string());
    assert_eq!(
        String::from_utf8(read_tar_entry(&control_tar, entry("./conffiles")).expect("bytes"))
            .expect("utf-8"),
        "/etc/helios/startup.toml\n"
    );
    let postinst = read_tar_entry(&control_tar, entry("./postinst")).expect("bytes");
    assert_eq!(entry("./postinst").mode, 0o755);
    let postinst = String::from_utf8(postinst).expect("utf-8");
    assert!(postinst.starts_with("#!/bin/sh\n"), "{postinst}");
    assert!(
        postinst.contains("systemctl enable 'helios-api.service'"),
        "{postinst}"
    );
    assert!(postinst.ends_with("exit 0\n"), "{postinst}");
    // A unit left failed (start limit hit) by the old version is reset
    // before it is started again.
    let reset = postinst
        .find("systemctl reset-failed 'helios-api.service'")
        .expect("reset-failed in postinst");
    let start = postinst
        .find("systemctl start 'helios-api.service'")
        .expect("start in postinst");
    assert!(reset < start, "{postinst}");
}

#[cfg(unix)]
#[test]
fn data_holds_the_tree_as_installed_under_dot_slash_paths() {
    let scratch = TempDir::new("gaia-exec-package-data");
    let root = scratch.join("data");
    let fixture = fixture(&root);
    let summary = run(&package_spec(
        &fixture,
        package("$assembly.out/", true, &[], &["helios-api.service"]),
    ));
    let ipk = PathBuf::from(state_value(&summary, "package.1.output"));
    let data_tar = member_tar(&ipk, "data.tar.gz", &scratch.join("parts"));
    // No `./` root: opkg would record it as an obsolete file named `.`.
    assert_eq!(
        entry_names(&data_tar),
        [
            "./etc/",
            "./etc/helios/",
            "./etc/helios/startup.toml",
            "./etc/systemd/",
            "./etc/systemd/system/",
            "./etc/systemd/system/helios-api.service",
            "./usr/",
            "./usr/local/",
            "./usr/local/bin/",
            "./usr/local/bin/helios-api",
        ]
    );
    let entries = list_tar(&data_tar).expect("listing");
    let mode = |name: &str| {
        entries
            .iter()
            .find(|entry| entry.name == name)
            .expect("entry")
            .mode
    };
    assert_eq!(mode("./usr/local/bin/helios-api"), 0o755);
    assert_eq!(mode("./etc/systemd/system/helios-api.service"), 0o644);
    assert_eq!(summary_files(&summary), 3);
}

fn summary_files(summary: &AssemblyStagingSummary) -> usize {
    state_value(summary, "package.1.files")
        .parse()
        .expect("files count")
}

#[cfg(unix)]
#[test]
fn two_builds_of_the_same_tree_are_byte_identical() {
    let scratch = TempDir::new("gaia-exec-package-determinism");
    let root = scratch.join("determinism");
    let fixture = fixture(&root);
    let first = run(&package_spec(
        &fixture,
        package("$assembly.out/first/", true, &[], &["helios-api.service"]),
    ));
    let second = run(&package_spec(
        &fixture,
        package("$assembly.out/second/", true, &[], &["helios-api.service"]),
    ));
    let first = fs::read(state_value(&first, "package.1.output")).expect("first ipk");
    let second = fs::read(state_value(&second, "package.1.output")).expect("second ipk");
    assert!(!first.is_empty());
    assert_eq!(first, second);
}

#[cfg(unix)]
#[test]
fn a_file_outside_the_allowed_prefixes_fails_the_build() {
    let scratch = TempDir::new("gaia-exec-package-prefix");
    let root = scratch.join("prefix");
    let fixture = fixture(&root);
    // The conffile under /etc/helios is not allowed.
    let spec = package_spec(
        &fixture,
        package(
            "$assembly.out/",
            true,
            &["/usr/local", "/etc/systemd/system"],
            &["helios-api.service"],
        ),
    );
    let message = run_error(&spec);
    assert!(
        message.contains("/etc/helios/startup.toml is outside allowed_prefixes"),
        "{message}"
    );
    assert!(
        !any_ipk_under(&root),
        "a package that fails its check is not published"
    );
}

/// Whether an `.ipk` file exists anywhere under `dir`.
fn any_ipk_under(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                any_ipk_under(&path)
            } else {
                path.extension().is_some_and(|ext| ext == "ipk")
            }
        })
    })
}

#[cfg(unix)]
#[test]
fn allowed_prefixes_that_hold_every_file_let_the_build_pass() {
    let scratch = TempDir::new("gaia-exec-package-prefix-ok");
    let root = scratch.join("prefix-ok");
    let fixture = fixture(&root);
    let spec = package_spec(
        &fixture,
        package(
            "$assembly.out/",
            true,
            &["/usr/local", "/etc/systemd/system", "/etc/helios"],
            &["helios-api.service"],
        ),
    );
    run(&spec);
}

#[cfg(unix)]
#[test]
fn a_service_unit_missing_from_the_tree_fails_the_build() {
    let scratch = TempDir::new("gaia-exec-package-unit");
    let root = scratch.join("unit");
    let fixture = fixture(&root);
    let spec = package_spec(
        &fixture,
        package("$assembly.out/", true, &[], &["missing.service"]),
    );
    let message = run_error(&spec);
    assert!(
        message.contains("service unit 'missing.service' is not in the tree"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn a_conffile_missing_from_the_tree_fails_the_build() {
    let scratch = TempDir::new("gaia-exec-package-conffile");
    let root = scratch.join("conffile");
    let fixture = fixture(&root);
    let mut entry = package("$assembly.out/", true, &[], &["helios-api.service"]);
    entry.scripts.conffiles = vec!["/etc/helios/absent.toml".into()];
    let message = run_error(&package_spec(&fixture, entry));
    assert!(
        message.contains("conffile '/etc/helios/absent.toml' is not a file of the tree"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "needs dpkg-deb and ar on the host; run with --ignored"]
fn dpkg_deb_and_ar_read_the_ipk() {
    if Command::new("dpkg-deb").arg("--version").output().is_err() {
        eprintln!("skipped: dpkg-deb is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-exec-package-dpkg");
    let root = scratch.join("dpkg");
    let fixture = fixture(&root);
    let summary = run(&package_spec(
        &fixture,
        package("$assembly.out/", true, &[], &["helios-api.service"]),
    ));
    let ipk = state_value(&summary, "package.1.output");
    let info = String::from_utf8(tool("dpkg-deb", &["-I", ipk.as_str()])).expect("utf-8");
    assert!(info.contains("Package: helios"), "{info}");
    assert!(info.contains("Architecture: aarch64"), "{info}");
    let contents = String::from_utf8(tool("dpkg-deb", &["-c", ipk.as_str()])).expect("utf-8");
    assert!(
        contents.contains("./usr/local/bin/helios-api"),
        "{contents}"
    );
    let members = String::from_utf8(tool("ar", &["t", ipk.as_str()])).expect("utf-8");
    assert_eq!(members, "debian-binary\ncontrol.tar.gz\ndata.tar.gz\n");
}

#[test]
fn a_deb_is_named_dot_deb_and_reads_back_with_dpkg_deb() {
    let scratch = TempDir::new("gaia-package-deb");
    let fixture = fixture(&scratch.join("deb"));
    let mut deb = package("$assembly.out/", true, &[], &["helios-api.service"]);
    deb.format = "deb".into();
    deb.control.architecture = "arm64".into();
    deb.output = AssemblyPackageSpec::output_for("$assembly.out/", &deb.control, "deb").into();
    assert!(deb.output.as_str().ends_with("helios_2.0.0_arm64.deb"));
    let summary = run(&package_spec(&fixture, deb));
    let path = PathBuf::from(state_value(&summary, "package.1.output"));
    assert!(path.is_file(), "{}", path.display());
    let members: Vec<String> = read_ar(&path)
        .expect("ar")
        .into_iter()
        .map(|member| member.name)
        .collect();
    assert_eq!(members, ["debian-binary", "control.tar.gz", "data.tar.gz"]);
    // A deb keeps the `./` root entry dpkg-deb writes.
    let data_tar = member_tar(&path, "data.tar.gz", &scratch.join("deb-parts"));
    assert_eq!(
        entry_names(&data_tar).first().map(String::as_str),
        Some("./")
    );
    // dpkg-deb, where it is installed, parses the control and lists the data.
    if Command::new("dpkg-deb").arg("--version").output().is_ok() {
        let info = String::from_utf8(tool("dpkg-deb", &["-I", path.to_str().expect("utf-8")]))
            .expect("utf-8");
        assert!(info.contains("Package: helios"), "{info}");
        assert!(info.contains("Architecture: arm64"), "{info}");
        let listing = String::from_utf8(tool("dpkg-deb", &["-c", path.to_str().expect("utf-8")]))
            .expect("utf-8");
        assert!(
            listing.contains("./etc/systemd/system/helios-api.service"),
            "{listing}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_src_dir_copied_into_the_package_tree_lists_its_nested_paths_in_the_data() {
    let scratch = TempDir::new("gaia-exec-package-src-dir");
    let root = scratch.join("nested");
    let fixture = fixture(&root);
    let site = root.join("assets/site");
    fs::create_dir_all(site.join("_app/immutable/chunks")).expect("site dirs");
    fs::write(site.join("index.html"), "<html></html>\n").expect("index");
    fs::write(site.join("_app/immutable/chunks/abc123.js"), "export {};\n").expect("chunk");
    let mut spec = package_spec(
        &fixture,
        package("$assembly.out/", true, &[], &["helios-api.service"]),
    );
    spec.image
        .assembly
        .as_mut()
        .expect("assembly")
        .files
        .push(AssemblyFileSpec {
            include: Vec::new(),
            strip: false,
            tree: "pkg-root".into(),
            src: None,
            src_glob: None,
            src_dir: Some(site.display().to_string().into()),
            exclude: Vec::new(),
            content: None,
            json: None,
            dest: "var/www/helios/".into(),
            mode: None,
            dir_mode: None,
            optional: false,
            preserve_symlink: false,
            allow_missing_libs: false,
        });
    let summary = run(&spec);
    let ipk = PathBuf::from(state_value(&summary, "package.1.output"));
    let data_tar = member_tar(&ipk, "data.tar.gz", &scratch.join("parts"));
    let names = entry_names(&data_tar);
    for expected in [
        "./var/www/helios/",
        "./var/www/helios/index.html",
        "./var/www/helios/_app/",
        "./var/www/helios/_app/immutable/",
        "./var/www/helios/_app/immutable/chunks/",
        "./var/www/helios/_app/immutable/chunks/abc123.js",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} is missing from the data: {names:?}"
        );
    }
}
