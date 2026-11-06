//! What `make show-info` reads, as one digest: the output's `.config` and
//! `local.mk`, and every `.mk`, `Config.in*` and `Makefile*` of the
//! Buildroot tree and its externals, by path and content. While it is
//! unchanged the recorded graph is the one show-info would print, and the
//! make (single-threaded, 15 to 45 s on the Raze's tree) is skipped.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) const PACKAGE_GRAPH_KEY: &str = ".gaia-buildroot-packages.key";

/// Directories show-info never reads: downloads, outputs, git metadata.
const SKIPPED: &[&str] = &[".git", "dl", "output", "per-package"];

pub(crate) fn package_graph_key(
    buildroot_dir: &Path,
    output_dir: &Path,
    br2_external: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    let mut add = |label: &str, path: &Path| {
        hasher.update(label.as_bytes());
        hasher.update([0]);
        hasher.update(path.as_os_str().as_encoded_bytes());
        hasher.update([0]);
        match std::fs::read(path) {
            Ok(bytes) => {
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            Err(_) => hasher.update(b"missing"),
        }
    };
    add("config", &output_dir.join(".config"));
    add("local", &output_dir.join("local.mk"));
    let mut trees = vec![buildroot_dir.to_path_buf()];
    trees.extend(
        br2_external
            .unwrap_or_default()
            .split([' ', ':'])
            .filter(|path| !path.is_empty())
            .map(PathBuf::from),
    );
    for tree in trees {
        let mut files = Vec::new();
        collect(&tree, output_dir, &mut files);
        files.sort();
        for file in files {
            add("tree", &file);
        }
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn collect(dir: &Path, output_dir: &Path, files: &mut Vec<PathBuf>) {
    if dir == output_dir {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if kind.is_dir() {
            if !SKIPPED.contains(&name.as_ref()) {
                collect(&path, output_dir, files);
            }
        } else if name.ends_with(".mk")
            || name.starts_with("Config.in")
            || name.starts_with("Makefile")
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_follows_the_config_and_the_package_files() {
        let scratch = gaia_testkit::TempDir::new("gaia-buildroot-graph-key");
        let root = scratch.path();
        let buildroot = root.join("buildroot");
        let output = root.join("output");
        let external = root.join("external");
        let write = |path: &Path, text: &str| {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, text).expect("write");
        };
        write(&buildroot.join("package/foo/foo.mk"), "FOO_VERSION = 1\n");
        write(&buildroot.join("package/foo/foo.hash"), "sha256 x\n");
        write(&buildroot.join("dl/foo/foo.mk"), "not read\n");
        write(
            &external.join("package/bar/Config.in"),
            "config BR2_PACKAGE_BAR\n",
        );
        write(&output.join(".config"), "BR2_PACKAGE_FOO=y\n");
        let external_arg = external.display().to_string();
        let key = || package_graph_key(&buildroot, &output, Some(&external_arg));
        let first = key();
        assert_eq!(first, key(), "the same inputs give the same key");

        write(&buildroot.join("package/foo/foo.hash"), "sha256 y\n");
        write(&buildroot.join("dl/foo/foo.mk"), "still not read\n");
        assert_eq!(first, key(), "hashes and downloads are not show-info's");

        write(&buildroot.join("package/foo/foo.mk"), "FOO_VERSION = 2\n");
        let second = key();
        assert_ne!(first, second, "a package version changes it");
        write(
            &external.join("package/bar/Config.in"),
            "config BR2_PACKAGE_BAR\n\tbool\n",
        );
        let third = key();
        assert_ne!(second, third, "an external's Config.in changes it");
        write(
            &output.join(".config"),
            "BR2_PACKAGE_FOO=y\nBR2_PACKAGE_BAR=y\n",
        );
        assert_ne!(third, key(), "the config changes it");
    }
}
