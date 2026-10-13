use gaia_image_providers::{ImageOutputContract, ImagePlan, ImageProvider, ImageProviderOperation};
use gaia_spec::{ImageProviderKind, ImageSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A scratch directory owned by one test; removed when the value is dropped.
pub struct Workspace {
    dir: TempDir,
}

impl Deref for Workspace {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.dir.path()
    }
}

impl AsRef<Path> for Workspace {
    fn as_ref(&self) -> &Path {
        self.dir.path()
    }
}

/// A config file written into its own scratch directory, removed when dropped.
pub struct ConfigFile {
    _dir: TempDir,
    path: PathBuf,
}

impl Deref for ConfigFile {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ConfigFile {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

pub fn write_temp_config(contents: &str) -> ConfigFile {
    let dir = TempDir::new("gaia-unresolved");
    let path = dir.join("config.toml");
    fs::write(&path, contents).expect("temp config should be written");
    ConfigFile { _dir: dir, path }
}

pub fn create_temp_workspace(prefix: &str) -> Workspace {
    Workspace {
        dir: TempDir::new(prefix),
    }
}

pub struct DuplicatePrepareImageProvider;

impl ImageProvider for DuplicatePrepareImageProvider {
    fn id(&self) -> &'static str {
        "prepare-image-test"
    }

    fn kind(&self) -> ImageProviderKind {
        ImageProviderKind::StartingPoint
    }

    fn plan_image(&self, image: &ImageSpec) -> ImagePlan {
        ImagePlan {
            operations: vec![
                ImageProviderOperation::Prepare,
                ImageProviderOperation::Prepare,
                ImageProviderOperation::Build,
            ],
            output: ImageOutputContract {
                collect_dir: image.output.collect_dir.clone(),
                archive_name: image.output.archive_name.clone(),
                emit_report: image.output.emit_report,
            },
        }
    }
}
