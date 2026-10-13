use gaia_testkit::TempDir;
use std::ops::Deref;
use std::path::{Path, PathBuf};

pub fn default_config_path() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/default-workspace/configs/default.toml")
        .display()
        .to_string()
}

/// A config file written into its own scratch directory. The directory is removed
/// when this value is dropped, so keep it alive for as long as the path is used.
pub struct TempConfig {
    path: PathBuf,
    _scratch: TempDir,
}

impl Deref for TempConfig {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.path
    }
}

impl AsRef<Path> for TempConfig {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

pub fn write_temp_config(contents: &str) -> TempConfig {
    let scratch = TempDir::new("gaia-config-pipeline");
    let path = scratch.join("config.toml");
    std::fs::write(&path, contents).expect("temp config should be written");
    TempConfig {
        path,
        _scratch: scratch,
    }
}

pub fn write_temp_config_at(path: &PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("temp config parent should be created");
    }
    std::fs::write(path, contents).expect("temp config should be written");
}
