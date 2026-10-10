//! The Buildroot scripts a build runs after its packages or images, by
//! content: the script named by `BR2_ROOTFS_POST_BUILD_SCRIPT`,
//! `BR2_ROOTFS_POST_IMAGE_SCRIPT` or `BR2_ROOTFS_POST_FAKEROOT_SCRIPT`, and
//! the regular files beside it.
//!
//! Those settings are not in `.config`-based clean decisions (a script only
//! reruns image generation), but a changed script still changes what a
//! build produces, so image operations fingerprint it.

use crate::buildroot_external_files::{ExternalTree, resolve_tree_path};
use crate::sha256_hex;
use gaia_spec::{BuildrootImageSpec, WorkspaceSpec};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The script settings, with the label each component is named by.
pub const POST_SCRIPT_SETTINGS: &[(&str, &str)] = &[
    ("BR2_ROOTFS_POST_BUILD_SCRIPT", "post-build script"),
    ("BR2_ROOTFS_POST_IMAGE_SCRIPT", "post-image script"),
    ("BR2_ROOTFS_POST_FAKEROOT_SCRIPT", "post-fakeroot script"),
];

/// Sibling files larger than this are described by their size only.
const SIBLING_HASH_LIMIT: u64 = 1 << 20;

/// The settings a build resolves to. Precedence, lowest first: the defconfig,
/// the config fragments, the config overrides.
pub fn resolved_settings(
    workspace: &WorkspaceSpec,
    buildroot: &BuildrootImageSpec,
    trees: &[ExternalTree],
    buildroot_dir: Option<&Path>,
) -> BTreeMap<String, String> {
    let mut settings = BTreeMap::new();
    let defconfig = match (&buildroot.defconfig_path, &buildroot.defconfig) {
        (Some(path), _) => Some(resolve_tree_path(workspace, path)),
        (None, Some(name)) => trees
            .iter()
            .map(|tree| tree.dir.as_path())
            .chain(buildroot_dir)
            .map(|dir| dir.join("configs").join(format!("{name}_defconfig")))
            .find(|path| path.is_file()),
        (None, None) => None,
    };
    if let Some(text) = defconfig.and_then(|path| fs::read_to_string(path).ok()) {
        parse_settings(&text, &mut settings);
    }
    for fragment in &buildroot.config_fragments {
        if let Ok(text) = fs::read_to_string(resolve_tree_path(workspace, fragment)) {
            parse_settings(&text, &mut settings);
        }
    }
    for (key, value) in &buildroot.config_overrides {
        settings.insert(key.clone(), unquote(value).to_string());
    }
    settings
}

/// `KEY=value` lines; `# KEY is not set` and comments yield nothing.
fn parse_settings(text: &str, settings: &mut BTreeMap<String, String>) {
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            settings.insert(key.trim().to_string(), unquote(value.trim()).to_string());
        }
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
}

/// One `(name, digest)` per configured post script. The name says which
/// setting and which file; the digest covers the setting's value (as written,
/// so a tree's checkout path does not count), the script and its siblings.
pub fn post_script_components(
    workspace: &WorkspaceSpec,
    buildroot: &BuildrootImageSpec,
    trees: &[ExternalTree],
    buildroot_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let settings = resolved_settings(workspace, buildroot, trees, buildroot_dir);
    let mut components = Vec::new();
    for (setting, label) in POST_SCRIPT_SETTINGS {
        let Some(value) = settings
            .get(*setting)
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let expanded = expand_external_paths(value, trees);
        let script = expanded.split_whitespace().next().unwrap_or_default();
        let resolved = resolve_script(script, trees, buildroot_dir, &workspace.root_dir);
        let (display, content) = match &resolved {
            Some(path) => (
                display_script(path, trees, &workspace.root_dir),
                script_digest(path),
            ),
            None => (script.to_string(), "unresolved".to_string()),
        };
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        hasher.update([0]);
        hasher.update(content.as_bytes());
        components.push((format!("{label} {display}"), hex(&hasher.finalize())));
    }
    components
}

/// Replaces `$(BR2_EXTERNAL_<NAME>_PATH)` with the tree's directory.
pub fn expand_external_paths(value: &str, trees: &[ExternalTree]) -> String {
    trees.iter().fold(value.to_string(), |text, tree| {
        text.replace(
            &format!("$(BR2_EXTERNAL_{}_PATH)", tree.name.to_ascii_uppercase()),
            &tree.dir.display().to_string(),
        )
    })
}

fn resolve_script(
    script: &str,
    trees: &[ExternalTree],
    buildroot_dir: Option<&Path>,
    root: &str,
) -> Option<PathBuf> {
    if script.is_empty() || script.contains('$') {
        return None;
    }
    let path = Path::new(script);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    buildroot_dir
        .into_iter()
        .map(Path::to_path_buf)
        .chain(trees.iter().map(|tree| tree.dir.clone()))
        .chain(std::iter::once(PathBuf::from(root)))
        .map(|base| base.join(path))
        .find(|candidate| candidate.is_file())
}

fn display_script(path: &Path, trees: &[ExternalTree], root: &str) -> String {
    for tree in trees {
        if let Ok(relative) = path.strip_prefix(&tree.dir) {
            return format!("{}:{}", tree.name, relative.display());
        }
    }
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Digest of the script's content and of each regular file beside it
/// (non-recursive, sorted by name, large files by size).
fn script_digest(script: &Path) -> String {
    let mut text = format!("script={}\n", file_digest(script));
    if let Some(dir) = script.parent()
        && let Ok(entries) = fs::read_dir(dir)
    {
        let mut siblings = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        siblings.sort();
        for sibling in siblings {
            let name = sibling
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let digest = match fs::metadata(&sibling) {
                Ok(metadata) if metadata.len() <= SIBLING_HASH_LIMIT => file_digest(&sibling),
                Ok(metadata) => format!("size:{}", metadata.len()),
                Err(_) => "missing".to_string(),
            };
            text.push_str(&format!("sibling {name}={digest}\n"));
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex(&hasher.finalize())
}

fn file_digest(path: &Path) -> String {
    sha256_hex(path).unwrap_or_else(|error| format!("unreadable:{error}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildroot_external_files::external_trees;

    fn workspace(root: &Path) -> WorkspaceSpec {
        WorkspaceSpec {
            root_dir: root.display().to_string(),
            ..WorkspaceSpec::default()
        }
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, contents).expect("file");
    }

    #[test]
    fn overrides_win_over_fragments_and_defconfig() {
        let root = std::env::temp_dir().join(format!(
            "gaia-buildroot-scripts-settings-{}",
            std::process::id()
        ));
        write(
            &root.join("raze.defconfig"),
            "BR2_ROOTFS_POST_IMAGE_SCRIPT=\"board/a.sh\"\n# BR2_X is not set\n",
        );
        let buildroot = BuildrootImageSpec {
            defconfig_path: Some("raze.defconfig".into()),
            config_overrides: vec![(
                "BR2_ROOTFS_POST_IMAGE_SCRIPT".into(),
                "\"board/b.sh\"".into(),
            )],
            ..BuildrootImageSpec::default()
        };
        let settings = resolved_settings(&workspace(&root), &buildroot, &[], None);
        assert_eq!(settings["BR2_ROOTFS_POST_IMAGE_SCRIPT"], "board/b.sh");
        assert!(!settings.contains_key("BR2_X"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_post_image_script_change_changes_its_component() {
        let root = std::env::temp_dir().join(format!(
            "gaia-buildroot-scripts-change-{}",
            std::process::id()
        ));
        let script = root.join("board/raze/post-image.sh");
        write(&script, "#!/bin/sh\necho one\n");
        write(&root.join("board/raze/helper.sh"), "helper\n");
        let buildroot = BuildrootImageSpec {
            config_overrides: vec![(
                "BR2_ROOTFS_POST_IMAGE_SCRIPT".into(),
                "\"board/raze/post-image.sh\"".into(),
            )],
            ..BuildrootImageSpec::default()
        };
        let ws = workspace(&root);
        let before = post_script_components(&ws, &buildroot, &[], None);
        assert_eq!(before.len(), 1);
        assert!(before[0].0.starts_with("post-image script "), "{before:?}");
        write(&script, "#!/bin/sh\necho two\n");
        let after = post_script_components(&ws, &buildroot, &[], None);
        assert_ne!(before, after);
        // A sibling file beside the script counts too.
        write(&root.join("board/raze/helper.sh"), "helper v2\n");
        assert_ne!(after, post_script_components(&ws, &buildroot, &[], None));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn external_tree_scripts_are_found_through_br2_external_paths() {
        let root = std::env::temp_dir().join(format!(
            "gaia-buildroot-scripts-external-{}",
            std::process::id()
        ));
        write(&root.join("ext/external.desc"), "name: RAZE\n");
        write(&root.join("ext/board/post-fakeroot.sh"), "fakeroot\n");
        let ws = workspace(&root);
        let buildroot = BuildrootImageSpec {
            external_tree: Some("ext".into()),
            config_overrides: vec![(
                "BR2_ROOTFS_POST_FAKEROOT_SCRIPT".into(),
                "\"$(BR2_EXTERNAL_RAZE_PATH)/board/post-fakeroot.sh\"".into(),
            )],
            ..BuildrootImageSpec::default()
        };
        let trees = external_trees(&ws, buildroot.external_tree.as_deref());
        let components = post_script_components(&ws, &buildroot, &trees, None);
        assert_eq!(components.len(), 1);
        assert_eq!(
            components[0].0,
            "post-fakeroot script RAZE:board/post-fakeroot.sh"
        );
        let _ = fs::remove_dir_all(root);
    }
}
