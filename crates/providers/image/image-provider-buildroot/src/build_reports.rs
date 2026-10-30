//! Reports a Buildroot build leaves beside its images:
//! - the installed size of every package, written as
//!   `<build>.package-sizes.json` in the collect dir, with the largest packages
//!   and their change since the previous build in the run notes;
//! - with `[image] legal_info = true`, the license and manifest files of every
//!   package (`make legal-info`), published as `legal-info.tar.zst` and reused
//!   while the set of packages is unchanged.
//!
//! Sizes are the bytes of the regular files each package installs into
//! `target/` (symbolic links count as none). Files a package puts in `images/`
//! are not counted.

use super::*;
pub(crate) use gaia_process::format_bytes;
use sha2::{Digest, Sha256};

/// The `build/` file listing each package's installed files (`package,path`).
const PACKAGE_FILE_LIST: &str = "build/packages-file-list.txt";
/// The published archive of the license and manifest files.
pub(crate) const LEGAL_INFO_ARCHIVE: &str = "legal-info.tar.zst";
/// The package set the archive was made from, kept beside the tree.
const LEGAL_INFO_STATE: &str = ".gaia-legal-info-state";
/// How many packages the run notes name.
const TOP_PACKAGES: usize = 10;

/// Installed bytes per package, largest first (ties by name).
pub(crate) fn package_sizes(output_dir: &Path) -> Vec<(String, u64)> {
    let Ok(list) = fs::read_to_string(output_dir.join(PACKAGE_FILE_LIST)) else {
        return Vec::new();
    };
    let target = output_dir.join("target");
    let mut sizes = BTreeMap::<String, u64>::new();
    for line in list.lines() {
        let Some((package, path)) = line.split_once(',') else {
            continue;
        };
        let relative = path.trim_start_matches("./");
        // Only paths inside target/: an absolute or `..` path is not a file
        // the package installed there.
        let inside = !relative.is_empty()
            && Path::new(relative)
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        let Some(metadata) = inside
            .then(|| fs::symlink_metadata(target.join(relative)).ok())
            .flatten()
        else {
            continue;
        };
        let bytes = if metadata.is_file() {
            metadata.len()
        } else {
            0
        };
        *sizes.entry(package.to_string()).or_default() += bytes;
    }
    let mut ranked = sizes.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked
}

/// The change from `previous` to `current`, for a note.
fn size_change(current: u64, previous: u64) -> String {
    match current.cmp(&previous) {
        std::cmp::Ordering::Equal => "unchanged".to_string(),
        std::cmp::Ordering::Greater => format!("+{}", format_bytes(current - previous)),
        std::cmp::Ordering::Less => format!("-{}", format_bytes(previous - current)),
    }
}

/// The sizes the previous report of `build_name` recorded, by package.
fn previous_sizes(path: &Path) -> Option<BTreeMap<String, u64>> {
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    Some(
        value
            .get("packages")?
            .as_array()?
            .iter()
            .filter_map(|entry| {
                Some((
                    entry.get("name")?.as_str()?.to_string(),
                    entry.get("bytes")?.as_u64()?,
                ))
            })
            .collect(),
    )
}

/// Writes `<build>.package-sizes.json` into `collect_dir` and returns the run
/// notes: the largest packages with their change since the previous build.
pub(crate) fn package_size_report(
    build_name: &str,
    output_dir: &Path,
    collect_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let sizes = package_sizes(output_dir);
    if sizes.is_empty() {
        return Ok(Vec::new());
    }
    let report = collect_dir.join(format!("{build_name}.package-sizes.json"));
    let previous = previous_sizes(&report);
    let total = sizes.iter().map(|(_, bytes)| bytes).sum::<u64>();
    let document = serde_json::json!({
        "build": build_name,
        "total_bytes": total,
        "packages": sizes
            .iter()
            .map(|(name, bytes)| serde_json::json!({ "name": name, "bytes": bytes }))
            .collect::<Vec<_>>(),
    });
    fs::create_dir_all(collect_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create '{}': {error}",
            collect_dir.display()
        ))
    })?;
    let text = serde_json::to_string_pretty(&document).unwrap_or_default();
    fs::write(&report, format!("{text}\n")).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write '{}': {error}",
            report.display()
        ))
    })?;

    let mut notes = vec![format!(
        "summary: package sizes: {} installed in target/; largest {} (change since the last build)",
        format_bytes(total),
        TOP_PACKAGES.min(sizes.len())
    )];
    for (rank, (name, bytes)) in sizes.iter().take(TOP_PACKAGES).enumerate() {
        let change = match previous.as_ref().map(|previous| previous.get(name)) {
            Some(Some(before)) => size_change(*bytes, *before),
            Some(None) => "new".to_string(),
            None => "no previous build".to_string(),
        };
        notes.push(format!(
            "summary:   {}. {name} {} ({change})",
            rank + 1,
            format_bytes(*bytes)
        ));
    }
    Ok(notes)
}

/// What a legal-info archive is made from: the package list of the tree.
fn package_set_fingerprint(tree_dir: &Path) -> String {
    let list = fs::read(tree_dir.join(PACKAGE_FILE_LIST)).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(b"gaia-legal-info-v1\0");
    hasher.update(&list);
    hex(&hasher.finalize())
}

/// The inputs of [`legal_info_archive`].
pub(crate) struct LegalInfoRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) buildroot_dir: &'a Path,
    pub(crate) tree_dir: &'a Path,
    pub(crate) collect_dir: &'a Path,
    pub(crate) command: ImageCommandContext<'a>,
}

/// Makes `legal-info.tar.zst` in the collect dir from `make legal-info`, or
/// keeps the archive already there when the package set is unchanged. Needs
/// `tar` with zstd support (`tar --zstd`).
pub(crate) fn legal_info_archive(
    request: LegalInfoRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let LegalInfoRequest {
        spec,
        buildroot_dir,
        tree_dir,
        collect_dir,
        command,
    } = request;
    let archive = collect_dir.join(LEGAL_INFO_ARCHIVE);
    let fingerprint = package_set_fingerprint(tree_dir);
    let state = tree_dir.join(LEGAL_INFO_STATE);
    if archive.is_file() && fs::read_to_string(&state).ok().as_deref() == Some(fingerprint.as_str())
    {
        return Ok(vec![
            "summary: legal-info: kept the archive (the package set is unchanged)".to_string(),
        ]);
    }

    let mut make = Command::new("make");
    make.arg(format!("O={}", tree_dir.display()))
        .arg("legal-info")
        .current_dir(buildroot_dir);
    apply_buildroot_policy_env(&mut make, spec, command.policy)?;
    let mut messages = run_command(
        make,
        "buildroot legal-info",
        command.execution,
        command.policy,
        command.log_sink.clone(),
        command.cancel_check.clone(),
    )?;
    if !tree_dir.join("legal-info").is_dir() {
        return Err(ImageProviderError::backend_command(
            "make legal-info did not create legal-info/ in the output tree",
        ));
    }

    fs::create_dir_all(collect_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create '{}': {error}",
            collect_dir.display()
        ))
    })?;
    let temporary = collect_dir.join(format!(".{LEGAL_INFO_ARCHIVE}.{}.tmp", std::process::id()));
    let mut tar = Command::new("tar");
    tar.arg("--zstd")
        .arg("-C")
        .arg(tree_dir)
        .arg("-cf")
        .arg(&temporary)
        .arg("legal-info");
    let archived = run_command(
        tar,
        "legal-info archive",
        command.execution,
        command.policy,
        command.log_sink.clone(),
        command.cancel_check.clone(),
    );
    let archived = archived.inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    messages.extend(archived);
    fs::rename(&temporary, &archive).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ImageProviderError::backend_command(format!(
            "failed to publish '{}': {error}",
            archive.display()
        ))
    })?;
    fs::write(&state, &fingerprint).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to record legal-info state in '{}': {error}",
            tree_dir.display()
        ))
    })?;
    let bytes = fs::metadata(&archive)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    messages.push(format!(
        "summary: legal-info: wrote {LEGAL_INFO_ARCHIVE} ({})",
        format_bytes(bytes)
    ));
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    fn installed(output: &Path, files: &[(&str, &str)]) {
        let target = output.join("target");
        for (path, contents) in files {
            let file = target.join(path);
            fs::create_dir_all(file.parent().expect("parent")).expect("dir");
            fs::write(file, contents).expect("file");
        }
    }

    #[test]
    fn package_sizes_sum_each_packages_installed_files_largest_first() {
        let scratch = TempDir::new("gaia-package-sizes");
        let output = scratch.join("output");
        installed(
            &output,
            &[
                ("bin/busybox", &"b".repeat(3000)),
                ("etc/busybox.conf", &"c".repeat(100)),
                ("lib/libfoo.so", &"f".repeat(5000)),
            ],
        );
        fs::create_dir_all(output.join("build")).expect("build");
        fs::write(
            output.join(PACKAGE_FILE_LIST),
            "busybox,./bin/busybox\nbusybox,./etc/busybox.conf\nlibfoo,./lib/libfoo.so\n\
             ghost,./usr/missing\nescape,../../etc/passwd\n",
        )
        .expect("list");
        assert_eq!(
            package_sizes(&output),
            vec![("libfoo".to_string(), 5000), ("busybox".to_string(), 3100)]
        );
    }

    #[test]
    fn the_report_names_the_top_packages_and_their_change() {
        let scratch = TempDir::new("gaia-package-report");
        let output = scratch.join("output");
        let collect = scratch.join("collect");
        installed(&output, &[("bin/a", &"a".repeat(2048))]);
        fs::create_dir_all(output.join("build")).expect("build");
        fs::write(output.join(PACKAGE_FILE_LIST), "alpha,./bin/a\n").expect("list");

        let first = package_size_report("acme", &output, &collect).expect("first");
        assert!(
            first
                .iter()
                .any(|note| note.contains("alpha 2.0 KiB (no previous build)"))
        );
        assert!(collect.join("acme.package-sizes.json").is_file());

        installed(&output, &[("bin/a", &"a".repeat(4096))]);
        let second = package_size_report("acme", &output, &collect).expect("second");
        assert!(
            second
                .iter()
                .any(|note| note.contains("alpha 4.0 KiB (+2.0 KiB)")),
            "{second:?}"
        );
        assert!(second.iter().all(|note| note.starts_with("summary: ")));
    }

    #[test]
    fn the_package_set_fingerprint_follows_the_package_list() {
        let scratch = TempDir::new("gaia-legal-info-fingerprint");
        let tree = scratch.join("tree");
        fs::create_dir_all(tree.join("build")).expect("build");
        fs::write(tree.join(PACKAGE_FILE_LIST), "alpha,./bin/a\n").expect("list");
        let before = package_set_fingerprint(&tree);
        assert_eq!(before, package_set_fingerprint(&tree));
        fs::write(
            tree.join(PACKAGE_FILE_LIST),
            "alpha,./bin/a\nbeta,./bin/b\n",
        )
        .expect("list");
        assert_ne!(before, package_set_fingerprint(&tree));
    }
}
