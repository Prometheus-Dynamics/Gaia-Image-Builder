//! The check a written package passes before the build goes on: the archive
//! holds its members in order, both tars gunzip and list, the control file
//! parses and names this package, the scripts are executable, the conffiles
//! and the service units are in the data, and every file of the data is
//! under the allowed prefixes. A failure fails the build.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gaia_spec::AssemblyPackageSpec;

use super::package_archive::{
    ArMember, TarEntry, TarEntryKind, extract_member, list_tar, read_ar, read_tar_entry,
};
use super::package_control::{check_control_fields, control_value, parse_control};

/// What the check counted.
pub(super) struct PackageCheck {
    /// The non-directory entries of the data.
    pub(super) files: usize,
}

/// Checks the written package at `output`. `stage` is scratch space for the
/// members, and `gzip` the tool that reads them.
pub(super) fn verify_ipk(
    package: &AssemblyPackageSpec,
    output: &Path,
    stage: &Path,
    gzip: &Path,
) -> Result<PackageCheck, String> {
    let id = package.id.as_str();
    let members = read_ar(output)?;
    let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
    let expected: Vec<&str> = if package.debian_binary {
        vec!["debian-binary", "control.tar.gz", "data.tar.gz"]
    } else {
        vec!["control.tar.gz", "data.tar.gz"]
    };
    if names != expected {
        return Err(format!(
            "package '{id}': the archive holds members {names:?}, expected {expected:?}"
        ));
    }
    if package.debian_binary {
        let path = member_file(output, &members[0], stage, "debian-binary")?;
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        if bytes != b"2.0\n" {
            return Err(format!("package '{id}': debian-binary is not '2.0'"));
        }
    }
    let control_tar = gunzip_member(output, &members[members.len() - 2], stage, "control", gzip)?;
    let data_tar = gunzip_member(output, &members[members.len() - 1], stage, "data", gzip)?;
    let control_entries = list_tar(&control_tar)?;
    let data_entries = list_tar(&data_tar)?;

    check_control(package, &control_tar, &control_entries, &data_entries)?;
    check_data(package, &data_entries)?;
    Ok(PackageCheck {
        files: data_entries
            .iter()
            .filter(|entry| entry.kind != TarEntryKind::Dir)
            .count(),
    })
}

fn check_control(
    package: &AssemblyPackageSpec,
    control_tar: &Path,
    entries: &[TarEntry],
    data: &[TarEntry],
) -> Result<(), String> {
    let id = package.id.as_str();
    let control_entry = find(entries, "./control")
        .ok_or_else(|| format!("package '{id}': control.tar has no ./control"))?;
    let text = String::from_utf8(read_tar_entry(control_tar, control_entry)?)
        .map_err(|_| format!("package '{id}': ./control is not UTF-8"))?;
    let fields = parse_control(&text).map_err(|error| format!("package '{id}': {error}"))?;
    check_control_fields(&fields).map_err(|error| format!("package '{id}': {error}"))?;
    let wanted = [
        ("Package", package.control.package.as_str()),
        ("Version", package.control.version.as_str()),
        ("Architecture", package.control.architecture.as_str()),
    ];
    for (field, want) in wanted {
        let got = control_value(&fields, field).unwrap_or_default();
        if got != want {
            return Err(format!(
                "package '{id}': control {field} is '{got}', the spec says '{want}'"
            ));
        }
    }
    let installed: u64 = data
        .iter()
        .filter(|entry| entry.kind == TarEntryKind::File)
        .map(|entry| entry.size)
        .sum();
    let recorded = control_value(&fields, "Installed-Size").unwrap_or_default();
    if recorded.parse::<u64>().ok() != Some(installed.div_ceil(1024)) {
        return Err(format!(
            "package '{id}': control Installed-Size is {recorded}, the data is {} KiB",
            installed.div_ceil(1024)
        ));
    }

    let scripts = &package.scripts;
    for name in ["postinst", "preinst", "prerm", "postrm"] {
        let required = name != "preinst" && !scripts.services.is_empty();
        let script = find(entries, &format!("./{name}"));
        match script {
            Some(entry) if entry.kind != TarEntryKind::File || entry.mode & 0o111 == 0 => {
                return Err(format!(
                    "package '{id}': control {name} is not an executable file"
                ));
            }
            None if required => {
                return Err(format!("package '{id}': control has no {name} script"));
            }
            _ => {}
        }
    }
    match find(entries, "./conffiles") {
        None if !scripts.conffiles.is_empty() => {
            return Err(format!("package '{id}': control has no conffiles"));
        }
        None => {}
        Some(entry) => {
            let listed = String::from_utf8(read_tar_entry(control_tar, entry)?)
                .map_err(|_| format!("package '{id}': conffiles is not UTF-8"))?;
            let listed = listed.lines();
            let wanted = scripts.conffiles.iter().map(String::as_str);
            if !listed.eq(wanted) {
                return Err(format!(
                    "package '{id}': conffiles does not list the spec's conffiles"
                ));
            }
        }
    }
    Ok(())
}

fn check_data(package: &AssemblyPackageSpec, data: &[TarEntry]) -> Result<(), String> {
    let id = package.id.as_str();
    let prefixes = &package.allowed_prefixes;
    if !prefixes.is_empty() {
        for entry in data.iter().filter(|entry| entry.kind != TarEntryKind::Dir) {
            let absolute = format!("/{}", entry.name.trim_start_matches("./"));
            let inside = prefixes.iter().any(|prefix| {
                let prefix = prefix.trim_end_matches('/');
                absolute == prefix || absolute.starts_with(&format!("{prefix}/"))
            });
            if !inside {
                return Err(format!(
                    "package '{id}': {absolute} is outside allowed_prefixes ({})",
                    prefixes.join(", ")
                ));
            }
        }
    }
    for unit in &package.scripts.services {
        let etc = format!("./etc/systemd/system/{unit}");
        let lib = format!("./usr/lib/systemd/system/{unit}");
        let present = data.iter().any(|entry| {
            (entry.name == etc || entry.name == lib)
                && matches!(entry.kind, TarEntryKind::File | TarEntryKind::Symlink)
        });
        if !present {
            return Err(format!(
                "package '{id}': service unit '{unit}' is not in the tree (looked for etc/systemd/system/{unit} and usr/lib/systemd/system/{unit})"
            ));
        }
    }
    for conffile in &package.scripts.conffiles {
        let name = format!(".{conffile}");
        let present = data
            .iter()
            .any(|entry| entry.name == name && entry.kind == TarEntryKind::File);
        if !present {
            return Err(format!(
                "package '{id}': conffile '{conffile}' is not a file of the tree"
            ));
        }
    }
    Ok(())
}

fn find<'a>(entries: &'a [TarEntry], name: &str) -> Option<&'a TarEntry> {
    entries.iter().find(|entry| entry.name == name)
}

/// Extracts `member` to a file named after `label` in `stage`.
fn member_file(
    archive: &Path,
    member: &ArMember,
    stage: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    let path = stage.join(format!("verify-{label}"));
    extract_member(archive, member, &path)?;
    Ok(path)
}

/// Extracts a gzipped `member` and gunzips it to a tar in `stage`.
fn gunzip_member(
    archive: &Path,
    member: &ArMember,
    stage: &Path,
    label: &str,
    gzip: &Path,
) -> Result<PathBuf, String> {
    let compressed = member_file(archive, member, stage, &format!("{label}.tar.gz"))?;
    let tar = stage.join(format!("verify-{label}.tar"));
    let out = File::create(&tar)
        .map_err(|error| format!("failed to create '{}': {error}", tar.display()))?;
    let child = Command::new(gzip)
        .arg("-dc")
        .arg(&compressed)
        .stdout(Stdio::from(out))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run gzip: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for gzip: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "the {label} member does not gunzip: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(tar)
}
