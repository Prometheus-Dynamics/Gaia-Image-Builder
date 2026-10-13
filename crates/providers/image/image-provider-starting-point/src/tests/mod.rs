use super::*;
use gaia_spec::{
    ArtifactDefinition, ArtifactOutputSpec, ArtifactRef, ArtifactSpec, ArtifactVariantSpec,
    ImageDefinition, ImageOutputSpec, ImageSpec, InstallEntrySpec, RustArtifactSpec,
    SourceDefinition, SourceRef, SourceSpec, StageContentOriginSpec, StageEnvSetSpec,
    StageFileSpec, StageServiceSpec, StartingPointImageSpec,
};
use gaia_testkit::TempDir;
use std::time::{Duration, Instant};

mod command;
mod packages;
mod provider;
mod raw_image;
