//! Content inputs of operations that the specs only name by path: the script a
//! Java artifact's `build_command` runs (and the shell files it sources), and
//! a Buildroot image's post scripts and `BR2_EXTERNAL` trees.
//!
//! The spec and its argv do not change when the file does, so the operation
//! fingerprint and [`crate::operation_components`] both use these digests.
//! Each named component is one input that can explain a rebuild.

use gaia_image_providers::{
    external_tree_files, external_trees, post_script_components, sha256_hex,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ImageDefinition, ResolvedBuildSpec};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// Shell files larger than this are not scanned for `source` lines.
const SCRIPT_SCAN_LIMIT: u64 = 1 << 20;

/// `build command script <path>` for each `build_command` argument that names
/// a regular file inside the workspace. The argument is resolved against the
/// artifact's source directory, then the workspace root. A shell script also
/// contributes the files it sources by a relative path (one level).
pub(crate) fn build_command_components(
    spec: &ResolvedBuildSpec,
    artifact: &ArtifactSpec,
) -> Vec<(String, String)> {
    let ArtifactDefinition::Java(java) = &artifact.definition else {
        return Vec::new();
    };
    let root = PathBuf::from(&spec.workspace.root_dir);
    let mut bases = Vec::new();
    if let Some(source_ref) = &artifact.source
        && let Some(source) = spec
            .sources
            .iter()
            .find(|source| source.id == source_ref.id)
    {
        bases.push(gaia_spec::source_materialized_dir(&spec.workspace, source));
    }
    bases.push(root.clone());
    let canonical_root = fs::canonicalize(&root).unwrap_or(root);
    let mut components = Vec::new();
    for argument in &java.build_command {
        let Some(file) = bases
            .iter()
            .map(|base| base.join(argument))
            .find(|candidate| candidate.is_file())
        else {
            continue;
        };
        let Ok(canonical) = fs::canonicalize(&file) else {
            continue;
        };
        if !canonical.starts_with(&canonical_root) {
            continue;
        }
        let display = canonical
            .strip_prefix(&canonical_root)
            .unwrap_or(&canonical)
            .display()
            .to_string();
        components.push((
            format!("build command script {display}"),
            script_with_sourced_digest(&canonical, &canonical_root, &bases),
        ));
    }
    components
}

/// The digest of a script, with the digests of the relative files it sources.
fn script_with_sourced_digest(script: &Path, root: &Path, bases: &[PathBuf]) -> String {
    let mut text = format!("script={}\n", file_digest(script));
    if is_shell_script(script)
        && let Ok(metadata) = fs::metadata(script)
        && metadata.len() <= SCRIPT_SCAN_LIMIT
        && let Ok(contents) = fs::read_to_string(script)
    {
        let directory = script.parent().unwrap_or(root);
        for sourced in sourced_paths(&contents) {
            let candidate = [directory.to_path_buf()]
                .into_iter()
                .chain(bases.iter().cloned())
                .map(|base| base.join(&sourced))
                .find(|candidate| candidate.is_file());
            let Some(candidate) = candidate.and_then(|path| fs::canonicalize(path).ok()) else {
                continue;
            };
            if candidate.starts_with(root) {
                text.push_str(&format!("sources {sourced}={}\n", file_digest(&candidate)));
            }
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex(&hasher.finalize())
}

fn is_shell_script(script: &Path) -> bool {
    let by_name = script
        .extension()
        .is_some_and(|extension| extension == "sh" || extension == "bash");
    by_name
        || fs::read_to_string(script).is_ok_and(|contents| {
            contents
                .lines()
                .next()
                .is_some_and(|first| first.starts_with("#!") && first.contains("sh"))
        })
}

/// Relative paths named by `. file` and `source file` lines, without shell
/// variables.
fn sourced_paths(contents: &str) -> Vec<String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = line
                .strip_prefix(". ")
                .or_else(|| line.strip_prefix("source "))?;
            let token = rest.split_whitespace().next()?;
            let token = token.trim_matches(|c| c == '"' || c == '\'');
            (!token.is_empty() && !token.contains('$') && !token.starts_with('/'))
                .then(|| token.to_string())
        })
        .collect()
}

/// The Buildroot components of an image operation: each post script (see
/// [`post_script_components`]) and each file of each `BR2_EXTERNAL` tree. Empty
/// for a non-Buildroot image or a build with no external tree or scripts.
pub(crate) fn buildroot_image_components(spec: &ResolvedBuildSpec) -> Vec<(String, String)> {
    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        return Vec::new();
    };
    let buildroot_dir = std::env::var("GAIA_BUILDROOT_DIR")
        .ok()
        .or_else(|| std::env::var("BUILDROOT_DIR").ok())
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let trees = external_trees(&spec.workspace, buildroot.external_tree.as_deref());
    let mut components =
        post_script_components(&spec.workspace, buildroot, &trees, buildroot_dir.as_deref());
    components.extend(
        external_tree_files(&trees)
            .into_iter()
            .map(|(key, file)| (format!("external file {key}"), file.digest)),
    );
    components
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
    use crate::{OperationKind, operation_fingerprint};
    use gaia_spec::{
        ArtifactOutputSpec, ArtifactSpec, BuildrootImageSpec, ImageSpec, JavaArtifactSpec,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn workspace_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gaia-build-inputs-{label}-{nonce}"));
        fs::create_dir_all(&root).expect("workspace");
        root
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, contents).expect("file");
    }

    fn java_artifact(build_command: &[&str]) -> ArtifactSpec {
        ArtifactSpec::new(
            "photonvision-jar",
            ArtifactDefinition::Java(JavaArtifactSpec {
                build_target: "jar".into(),
                build_args: Vec::new(),
                build_command: build_command.iter().map(|arg| arg.to_string()).collect(),
                build_env: Vec::new(),
            }),
            None,
            ArtifactOutputSpec {
                path: "out/photonvision.jar".into(),
            },
        )
    }

    #[test]
    fn a_build_command_script_change_flips_the_artifact_fingerprint() {
        let root = workspace_dir("artifact");
        let script = root.join("raze/scripts/build-photonvision-jar.sh");
        write(&script, "#!/bin/sh\necho one\n");
        let mut spec = ResolvedBuildSpec::new("build-command");
        spec.workspace.root_dir = root.display().to_string();
        let artifact = java_artifact(&["raze/scripts/build-photonvision-jar.sh"]);
        let kind = OperationKind::BuildArtifact {
            artifact_id: artifact.id.clone(),
        };
        spec.artifacts.push(artifact);

        let components = build_command_components(&spec, &spec.artifacts[0]);
        assert_eq!(
            components
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["build command script raze/scripts/build-photonvision-jar.sh"]
        );
        let before = operation_fingerprint(&spec, &kind);
        write(&script, "#!/bin/sh\necho two\n");
        assert_ne!(before, operation_fingerprint(&spec, &kind));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_sourced_file_of_a_build_command_script_counts() {
        let root = workspace_dir("sourced");
        write(
            &root.join("scripts/build.sh"),
            ". ./lib/env.sh\necho build\n",
        );
        write(&root.join("scripts/lib/env.sh"), "export A=1\n");
        let mut spec = ResolvedBuildSpec::new("sourced");
        spec.workspace.root_dir = root.display().to_string();
        let artifact = java_artifact(&["scripts/build.sh"]);
        let before = build_command_components(&spec, &artifact);
        // `. ./lib/env.sh` resolves beside the script: scripts/lib/env.sh.
        write(&root.join("scripts/lib/env.sh"), "export A=2\n");
        let after = build_command_components(&spec, &artifact);
        assert_eq!(before.len(), 1);
        assert_ne!(before, after);
        let _ = fs::remove_dir_all(root);
    }

    fn buildroot_spec(root: &Path, external_tree: Option<&str>, script: &str) -> ResolvedBuildSpec {
        let mut spec = ResolvedBuildSpec::new("buildroot-inputs");
        spec.workspace.root_dir = root.display().to_string();
        spec.image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
            external_tree: external_tree.map(str::to_string),
            config_overrides: vec![(
                "BR2_ROOTFS_POST_IMAGE_SCRIPT".into(),
                format!("\"{script}\""),
            )],
            ..BuildrootImageSpec::default()
        }));
        spec
    }

    #[test]
    fn a_post_image_script_change_flips_the_image_build_fingerprint() {
        let root = workspace_dir("post-image");
        let script = root.join("board/raze/post-image.sh");
        write(&script, "#!/bin/sh\necho one\n");
        let spec = buildroot_spec(&root, None, "board/raze/post-image.sh");
        let before = operation_fingerprint(&spec, &OperationKind::BuildImage);
        write(&script, "#!/bin/sh\necho two\n");
        assert_ne!(
            before,
            operation_fingerprint(&spec, &OperationKind::BuildImage)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_external_mk_change_flips_the_image_fingerprints_and_names_its_file() {
        let root = workspace_dir("external-mk");
        let external = root.join("ext");
        write(&external.join("external.desc"), "name: RAZE\n");
        let mk = external.join("external.mk");
        write(
            &mk,
            "HOST_EROFS_UTILS_CONF_OPTS += --enable-multithreading\n",
        );
        let spec = buildroot_spec(&root, Some("ext"), "board/none.sh");
        let before_prepare = operation_fingerprint(&spec, &OperationKind::PrepareImage);
        let before_build = operation_fingerprint(&spec, &OperationKind::BuildImage);
        let components = buildroot_image_components(&spec);
        assert!(
            components
                .iter()
                .any(|(name, _)| name == "external file RAZE:external.mk"),
            "{components:?}"
        );
        write(
            &mk,
            "HOST_EROFS_UTILS_CONF_OPTS += --disable-multithreading\n",
        );
        assert_ne!(
            before_prepare,
            operation_fingerprint(&spec, &OperationKind::PrepareImage)
        );
        assert_ne!(
            before_build,
            operation_fingerprint(&spec, &OperationKind::BuildImage)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sourced_paths_skip_variables_and_absolute_paths() {
        let paths = sourced_paths(
            "#!/bin/sh\n. ./lib/common.sh\nsource \"helpers/net.sh\"\n. $HOME/x\n. /etc/profile\necho . x\n",
        );
        assert_eq!(paths, ["./lib/common.sh", "helpers/net.sh"]);
    }
}
