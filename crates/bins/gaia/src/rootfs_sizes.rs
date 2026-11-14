//! `gaia sizes --rootfs <erofs image>`: a root filesystem image's sizes.
//!
//! Read with a build's own erofs tools (`dump.erofs`, `fsck.erofs` of its
//! host tools), the ones that made this builder's images, so an image of
//! another builder is measured as this builder's are: its size, how it is
//! compressed and with which features, what it unpacks to, its kernel
//! modules, and its largest trees and files. The image is unpacked to a
//! scratch directory, which is removed again.
//!
//! Which component put what there is known only for a tree this builder
//! merged (`gaia sizes <run>`); here that is a row that says so.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One measured thing: what, bytes, and a detail.
pub type Row = (String, u64, String);

/// The trees and files shown.
const TOP: usize = 10;

/// The directory of `tools` that holds `dump.erofs` and `fsck.erofs`:
/// itself, or its `bin`, `host/bin`.
pub fn tools_dir(tools: &Path) -> Option<PathBuf> {
    [
        tools.to_path_buf(),
        tools.join("bin"),
        tools.join("host/bin"),
    ]
    .into_iter()
    .find(|dir| dir.join("dump.erofs").is_file() && dir.join("fsck.erofs").is_file())
}

fn said(program: &Path, args: &[&str], image: &Path) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .arg(image)
        .env_clear()
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} {}: {}",
            program.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The value of the superblock line that starts with `label`.
fn field<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(label))
        .map(str::trim)
        .unwrap_or_default()
}

/// How the image is compressed and with which features, from what
/// `dump.erofs -s` prints. Its lines differ by version: an image with a
/// compression table has `compr_algs`; an older lz4 one only its distance;
/// the features are on one line or on two.
fn compression(superblock: &str) -> String {
    let algorithms = match field(superblock, "Filesystem compr_algs:") {
        "" if superblock.contains("Filesystem lz4_max_distance:") => "lz4",
        "" => "none stated",
        stated => stated,
    };
    let features: Vec<&str> = superblock
        .lines()
        .filter(|line| line.starts_with("Filesystem ") && line.contains("features:"))
        .filter_map(|line| line.split_once("features:"))
        .flat_map(|(_, list)| list.split_whitespace())
        .collect();
    format!("{algorithms}; features {}", features.join(" "))
}

/// Every regular file under `dir`, as its path from it and its size, not
/// following links; and the counts of directories and symlinks.
fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, u64)>, others: &mut (u64, u64)) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            others.0 += 1;
            walk(root, &path, files, others);
        } else if meta.file_type().is_symlink() {
            others.1 += 1;
        } else if meta.is_file() {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            files.push((name, meta.len()));
        }
    }
}

fn is_module(name: &str) -> bool {
    [".ko", ".ko.xz", ".ko.gz", ".ko.zst"]
        .iter()
        .any(|end| name.ends_with(end))
}

/// Makes everything under `dir` removable, then removes it.
fn remove(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fn open(dir: &Path) {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                open(&path);
            }
        }
    }
    open(dir);
    let _ = std::fs::remove_dir_all(dir);
}

/// The rows of the erofs image at `image`, read with the tools in `tools`
/// (see [`tools_dir`]); unpacked under `scratch`.
pub fn measure(image: &Path, tools: &Path, scratch: &Path) -> Result<Vec<Row>, String> {
    let dir = tools_dir(tools).ok_or_else(|| {
        format!(
            "{} has no dump.erofs and fsck.erofs (in it, its bin or its host/bin)",
            tools.display()
        )
    })?;
    let stored = std::fs::metadata(image)
        .map_err(|e| format!("{}: {e}", image.display()))?
        .len();
    let version = said(&dir.join("dump.erofs"), &["--version"], Path::new(""))
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let superblock = said(&dir.join("dump.erofs"), &["-s"], image)?;
    let mut rows: Vec<Row> = vec![
        (
            "rootfs image".into(),
            stored,
            format!("read with {version}"),
        ),
        ("  compression".into(), 0, compression(&superblock)),
    ];
    let out = scratch.join(format!("gaia-sizes-rootfs-{}", std::process::id()));
    let extract = format!("--extract={}", out.display());
    let result = said(&dir.join("fsck.erofs"), &[&extract], image).map(|_| {
        let mut files = Vec::new();
        let mut others = (0, 0);
        walk(&out, &out, &mut files, &mut others);
        let total: u64 = files.iter().map(|(_, bytes)| bytes).sum();
        rows.push((
            "rootfs unpacked".into(),
            total,
            format!(
                "{} files, {} directories, {} symlinks",
                files.len(),
                others.0,
                others.1
            ),
        ));
        let modules: Vec<&(String, u64)> = files
            .iter()
            .filter(|(name, _)| name.contains("lib/modules/") && is_module(name))
            .collect();
        rows.push((
            "kernel modules".into(),
            modules.iter().map(|(_, bytes)| bytes).sum(),
            format!("{} modules", modules.len()),
        ));
        rows.push((
            "kernel modules tree".into(),
            files
                .iter()
                .filter(|(name, _)| name.contains("lib/modules/"))
                .map(|(_, bytes)| bytes)
                .sum(),
            String::new(),
        ));
        // The largest trees two levels down (`usr/bin`, `usr/lib`, `etc`).
        let mut trees: std::collections::BTreeMap<String, (u64, usize)> = Default::default();
        for (name, bytes) in &files {
            let mut parts = name.split('/');
            let tree = match (parts.next(), parts.next(), parts.next()) {
                (Some(a), Some(b), Some(_)) => format!("{a}/{b}"),
                (Some(a), Some(_), None) => a.to_string(),
                _ => "(top)".to_string(),
            };
            let at = trees.entry(tree).or_default();
            at.0 += bytes;
            at.1 += 1;
        }
        let mut ranked: Vec<(String, (u64, usize))> = trees.into_iter().collect();
        ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
        for (tree, (bytes, count)) in ranked.into_iter().take(TOP) {
            rows.push((format!("  tree {tree}"), bytes, format!("{count} files")));
        }
        files.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (name, bytes) in files.iter().take(TOP) {
            rows.push((format!("  file {name}"), *bytes, String::new()));
        }
    });
    remove(&out);
    result?;
    rows.push((
        "not measured".into(),
        0,
        "each component's bytes: known only for a tree this builder merged (gaia sizes <run>)"
            .into(),
    ));
    Ok(rows)
}
