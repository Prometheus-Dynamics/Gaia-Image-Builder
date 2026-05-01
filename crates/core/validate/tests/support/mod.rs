use gaia_image_providers::{ImageOutputContract, ImagePlan, ImageProvider, ImageProviderOperation};
use gaia_spec::{ImageProviderKind, ImageSpec};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn write_temp_config(contents: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gaia-unresolved-{nonce}.toml"));
    fs::write(&path, contents).expect("temp config should be written");
    path
}

pub fn create_temp_workspace(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    fs::create_dir_all(&root).expect("temp workspace should be created");
    root
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
