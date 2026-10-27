//! The file a `[[stage.files]]` entry copies: its `src`, or for a `content`
//! entry the text written to `<build_dir>/stage-content/<id>`.
use std::fs;
use std::path::PathBuf;

use gaia_spec::{ResolvedBuildSpec, StageFileSpec};

use crate::ImageProviderError;

/// Resolves the source of `file`, writing a `content` entry's text first.
/// The text file is only rewritten when it changed, so its mtime stays put
/// between runs that render the same text.
pub fn stage_file_source(
    spec: &ResolvedBuildSpec,
    file: &StageFileSpec,
) -> Result<PathBuf, ImageProviderError> {
    let resolve = |raw: &str| {
        gaia_spec::resolve_workspace_path(&spec.workspace, raw)
            .map_err(|error| ImageProviderError::runtime_state(error.to_string()))
    };
    let Some(content) = &file.content else {
        return resolve(&file.src);
    };
    let dir = resolve(&spec.workspace.build_dir)?.join("stage-content");
    let path = dir.join(file.id.as_str());
    if fs::read(&path).ok().as_deref() == Some(content.as_bytes()) {
        return Ok(path);
    }
    fs::create_dir_all(&dir).map_err(|error| {
        ImageProviderError::runtime_state(format!("failed to create '{}': {error}", dir.display()))
    })?;
    fs::write(&path, content).map_err(|error| {
        ImageProviderError::runtime_state(format!(
            "failed to write stage file content '{}': {error}",
            path.display()
        ))
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    #[test]
    fn content_is_written_under_the_build_dir_and_src_resolves_as_before() {
        let root = TempDir::new("gaia-stage-content");
        let mut spec = ResolvedBuildSpec::new("stage-content");
        spec.workspace.root_dir = root.path().display().to_string();
        let file = StageFileSpec::generated("os-release", "", "/etc/os-release")
            .with_content("VERSION_ID=\"2026.2.0-dev.7\"\n");
        let path = stage_file_source(&spec, &file).expect("content");
        assert_eq!(path, root.path().join("build/stage-content/os-release"));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "VERSION_ID=\"2026.2.0-dev.7\"\n"
        );
        let copied = StageFileSpec::static_asset("motd", "assets/motd", "/etc/motd");
        assert_eq!(
            stage_file_source(&spec, &copied).expect("src"),
            root.path().join("assets/motd")
        );
    }
}
