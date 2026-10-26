//! The `[[image.assembly.packages]]` output: an opkg `ipk` package, written
//! from an assembly tree. Its data is the tree as installed (`./`-prefixed,
//! sorted, dirs included, the same walk the OCI layers use); its control is
//! the control file, the generated or given maintainer scripts, and the
//! conffiles. Both tars are gzipped with the external `gzip -n -9`, then
//! written into an ar archive (`debian-binary` first unless the spec drops
//! it). The result is read back and checked before it is published.
//!
//! The data and control tars are built in a staging dir beside the output
//! and removed afterwards. Only the published `.ipk` is kept.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::oci::append_tree;
use super::package_archive::{TarEntryKind, list_tar, write_ar};
use super::package_control::{control_text, generated_script};
use super::package_verify::{verify_device, verify_ipk};
use super::tar::TarWriter;
use super::transforms::compress_to_dest;
use super::*;
use gaia_spec::{AssemblyPackageSpec, KeyValueState};

/// What one package build produced.
pub(super) struct PackageSummary {
    pub(super) output: PathBuf,
    /// The size of the `.ipk`.
    pub(super) bytes: u64,
    pub(super) sha256: String,
    /// The non-directory entries of the data.
    pub(super) files: usize,
    /// The `Installed-Size` written, in KiB.
    pub(super) installed_kib: u64,
}

/// The `debian-binary` member's content: the format version opkg and dpkg
/// read.
const DEBIAN_BINARY: &[u8] = b"2.0\n";

/// Writes an `[[image.assembly.packages]]` entry and publishes it atomically.
pub(super) fn execute_assembly_package(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    package: &AssemblyPackageSpec,
    cancel: Option<gaia_process::ProcessCancelCheck>,
) -> Result<PackageSummary, AssemblyError> {
    let known = [gaia_spec::PACKAGE_FORMAT_IPK, gaia_spec::PACKAGE_FORMAT_DEB];
    if !known.contains(&package.format.as_str())
        || (package.format == gaia_spec::PACKAGE_FORMAT_DEB && !package.debian_binary)
    {
        return Err(format!(
            "assembly package '{}' has an unsupported format '{}'",
            package.id, package.format
        )
        .into());
    }
    let output = roots.resolve_path(spec, package.output.as_str())?;
    let root = roots.tree_path(package.tree.as_str())?.to_path_buf();
    let parent = output.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "assembly package '{}' output '{}' has no parent dir",
            package.id,
            output.display()
        )
    })?;
    std_fs::create_dir_all(&parent).map_err(|error| {
        format!(
            "failed to create assembly package output dir '{}': {error}",
            parent.display()
        )
    })?;
    let name = output.file_name().map_or_else(
        || "package".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let stage = parent.join(format!(".{name}.gaia-pkg"));
    remove_stage(&stage);
    std_fs::create_dir_all(&stage).map_err(|error| {
        format!(
            "failed to create assembly package staging dir '{}': {error}",
            stage.display()
        )
    })?;
    let built = build_package(spec, roots, package, &root, &stage, &output, cancel);
    remove_stage(&stage);
    built
}

fn build_package(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    package: &AssemblyPackageSpec,
    root: &Path,
    stage: &Path,
    output: &Path,
    cancel: Option<gaia_process::ProcessCancelCheck>,
) -> Result<PackageSummary, AssemblyError> {
    // The device checks read the tree as installed, before anything is packed:
    // a package for the wrong device never reaches the archive.
    if let Some(device) = &package.device {
        let check = verify_device(spec, package, device, root)?;
        tracing::info!(
            package = package.id.as_str(),
            elf_files = check.elf_files,
            "package device checks passed"
        );
    }
    let mtime = entry_mtime();
    let data_tar = stage.join("data.tar");
    write_data_tar(root, &data_tar, mtime).map_err(|error| {
        format!(
            "assembly package '{}' (tree '{}'): {error}",
            package.id, package.tree
        )
    })?;
    // Installed-Size: the regular files' bytes, rounded up to KiB.
    let installed_bytes: u64 = list_tar(&data_tar)?
        .iter()
        .filter(|entry| entry.kind == TarEntryKind::File)
        .map(|entry| entry.size)
        .sum();
    let installed_kib = installed_bytes.div_ceil(1024);

    let control_tar = stage.join("control.tar");
    write_control_tar(spec, roots, package, installed_kib, mtime, &control_tar)?;

    let data_gz = stage.join("data.tar.gz");
    let control_gz = stage.join("control.tar.gz");
    // Fixed level and no name or time in the header: the same tars give the
    // same bytes.
    let gzip_args = ["-n", "-c", "-9"];
    compress_to_dest(
        spec,
        roots,
        "gzip",
        &gzip_args,
        data_tar,
        data_gz.clone(),
        cancel.clone(),
    )?;
    compress_to_dest(
        spec,
        roots,
        "gzip",
        &gzip_args,
        control_tar,
        control_gz.clone(),
        cancel,
    )?;

    let binary = stage.join("debian-binary");
    let mut members: Vec<(&str, &Path)> = Vec::new();
    if package.debian_binary {
        std_fs::write(&binary, DEBIAN_BINARY)
            .map_err(|error| format!("failed to write '{}': {error}", binary.display()))?;
        members.push(("debian-binary", &binary));
    }
    members.push(("control.tar.gz", &control_gz));
    members.push(("data.tar.gz", &data_gz));

    // The archive is checked where it was written, and published only when
    // it passes: a package that fails the check never reaches the output dir.
    let temp = temporary_assembly_output_path(output);
    if let Err(error) = write_ar(&temp, &members) {
        let _ = std_fs::remove_file(&temp);
        return Err(format!("assembly package '{}': {error}", package.id).into());
    }
    let gzip = resolve_assembly_tool(roots, "gzip")?.program;
    let check = match verify_ipk(package, &temp, stage, &gzip) {
        Ok(check) => check,
        Err(error) => {
            let _ = std_fs::remove_file(&temp);
            return Err(error.into());
        }
    };
    let sha256 = publish_assembly_output(&temp, output)?;
    Ok(PackageSummary {
        output: output.to_path_buf(),
        bytes: file_len(output)?,
        sha256,
        files: check.files,
        installed_kib,
    })
}

/// The mtime of every tar entry: `SOURCE_DATE_EPOCH` when the environment sets
/// it, else 0 (the same rule the OCI config's `created` time follows).
fn entry_mtime() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// The data tar: a `./` root, then the tree's entries in the OCI layer order.
fn write_data_tar(root: &Path, tar: &Path, mtime: u64) -> Result<(), String> {
    let file = File::create(tar)
        .map_err(|error| format!("failed to create '{}': {error}", tar.display()))?;
    let mut writer = TarWriter::new(BufWriter::new(file)).with_mtime(mtime);
    let mut first_names = HashMap::new();
    writer.append_dir_mode("./", 0o755)?;
    append_tree(&mut writer, root, "./", &mut first_names)?;
    finish_tar(writer, tar)
}

/// The control tar: `./control`, `./conffiles` when there are conffiles, and
/// the maintainer scripts (mode 0755), each given or generated.
fn write_control_tar(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    package: &AssemblyPackageSpec,
    installed_kib: u64,
    mtime: u64,
    tar: &Path,
) -> Result<(), AssemblyError> {
    let file = File::create(tar)
        .map_err(|error| format!("failed to create '{}': {error}", tar.display()))?;
    let mut writer = TarWriter::new(BufWriter::new(file)).with_mtime(mtime);
    writer.append_dir_mode("./", 0o755)?;
    let control = control_text(&package.control, installed_kib);
    append_bytes(&mut writer, "./control", 0o644, control.as_bytes())?;
    let scripts = &package.scripts;
    if !scripts.conffiles.is_empty() {
        let conffiles: String = scripts
            .conffiles
            .iter()
            .map(|path| format!("{path}\n"))
            .collect();
        append_bytes(&mut writer, "./conffiles", 0o644, conffiles.as_bytes())?;
    }
    let overrides = [
        ("postinst", &scripts.postinst),
        ("preinst", &scripts.preinst),
        ("prerm", &scripts.prerm),
        ("postrm", &scripts.postrm),
    ];
    for (name, given) in overrides {
        let bytes = match given {
            Some(path) => {
                let path = roots.resolve_path(spec, path.as_str())?;
                std_fs::read(&path).map_err(|error| {
                    format!(
                        "assembly package '{}': failed to read script '{}': {error}",
                        package.id,
                        path.display()
                    )
                })?
            }
            None => match generated_script(name, &scripts.services) {
                Some(text) => text.into_bytes(),
                None => continue,
            },
        };
        append_bytes(&mut writer, &format!("./{name}"), 0o755, &bytes)?;
    }
    finish_tar(writer, tar).map_err(AssemblyError::from)
}

fn append_bytes(
    writer: &mut TarWriter<BufWriter<File>>,
    name: &str,
    mode: u32,
    bytes: &[u8],
) -> Result<(), String> {
    writer.append_file(name, mode, bytes.len() as u64, &mut &bytes[..], |_| {})
}

fn finish_tar(writer: TarWriter<BufWriter<File>>, tar: &Path) -> Result<(), String> {
    let mut out = writer.finish()?;
    out.flush()
        .and_then(|()| out.get_ref().sync_all())
        .map_err(|error| format!("failed to write '{}': {error}", tar.display()))
}

fn remove_stage(stage: &Path) {
    let _ = std_fs::remove_dir_all(stage);
}

/// The state keys of a package, under `package.<index>.`.
pub(super) fn record_package_state(
    state: &mut KeyValueState,
    index: usize,
    package: &AssemblyPackageSpec,
    summary: &PackageSummary,
) {
    let key = AssemblyStateKey::new("package", index);
    state.insert(key.field("id"), package.id.as_str());
    state.insert(key.field("format"), package.format.as_str());
    state.insert(key.field("output"), summary.output.display().to_string());
    state.insert(key.field("bytes"), summary.bytes);
    state.insert(key.field("sha256"), &summary.sha256);
    state.insert(key.field("files"), summary.files);
    state.insert(key.field("installed_kib"), summary.installed_kib);
}
