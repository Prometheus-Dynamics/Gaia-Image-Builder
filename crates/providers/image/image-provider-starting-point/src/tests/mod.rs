use super::*;
use gaia_spec::{
    ArtifactDefinition, ArtifactOutputSpec, ArtifactRef, ArtifactSpec, ArtifactVariantSpec,
    ImageDefinition, ImageOutputSpec, ImageSpec, InstallEntrySpec, RustArtifactSpec,
    SourceDefinition, SourceRef, SourceSpec, StageContentOriginSpec, StageEnvSetSpec,
    StageFileSpec, StageServiceSpec, StartingPointImageSpec,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn unique_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{label}-{nonce}"));
    fs::create_dir_all(&path).expect("unique dir");
    path
}

mod command;
mod packages;
mod provider;
mod raw_image;
