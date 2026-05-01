use gaia_artifact_provider_go::GoProvider;
use gaia_artifact_provider_java::JavaProvider;
use gaia_artifact_provider_node::NodeProvider;
use gaia_artifact_provider_python::PythonProvider;
use gaia_artifact_provider_rust::RustProvider;
use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_image_provider_buildroot::BuildrootImageProvider;
use gaia_image_provider_starting_point::StartingPointImageProvider;
use gaia_image_providers::ImageProviderCatalog;
use gaia_source_providers::SourceProviderCatalog;

pub struct ProviderCatalogs {
    pub source: SourceProviderCatalog,
    pub artifact: ArtifactProviderCatalog,
    pub image: ImageProviderCatalog,
}

impl ProviderCatalogs {
    pub fn with_defaults() -> Self {
        Self {
            source: default_source_provider_catalog(),
            artifact: default_artifact_provider_catalog(),
            image: default_image_provider_catalog(),
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        SourceProviderCatalog,
        ArtifactProviderCatalog,
        ImageProviderCatalog,
    ) {
        (self.source, self.artifact, self.image)
    }
}

impl Default for ProviderCatalogs {
    fn default() -> Self {
        Self::with_defaults()
    }
}

pub fn default_source_provider_catalog() -> SourceProviderCatalog {
    SourceProviderCatalog::with_defaults()
}

pub fn default_artifact_provider_catalog() -> ArtifactProviderCatalog {
    let mut catalog = ArtifactProviderCatalog::new();
    catalog.register(Box::new(RustProvider));
    catalog.register(Box::new(JavaProvider));
    catalog.register(Box::new(NodeProvider));
    catalog.register(Box::new(PythonProvider));
    catalog.register(Box::new(GoProvider));
    catalog
}

pub fn default_image_provider_catalog() -> ImageProviderCatalog {
    let mut catalog = ImageProviderCatalog::new();
    catalog.register(Box::new(BuildrootImageProvider));
    catalog.register(Box::new(StartingPointImageProvider));
    catalog
}

pub fn provider_catalogs() -> (
    SourceProviderCatalog,
    ArtifactProviderCatalog,
    ImageProviderCatalog,
) {
    ProviderCatalogs::with_defaults().into_parts()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::{ArtifactProviderKind, ImageProviderKind, SourceProviderKind};

    #[test]
    fn default_catalogs_include_all_builtin_providers() {
        let catalogs = ProviderCatalogs::with_defaults();

        for kind in [
            SourceProviderKind::Git,
            SourceProviderKind::Path,
            SourceProviderKind::Archive,
            SourceProviderKind::Download,
        ] {
            assert!(catalogs.source.find_for_kind(kind).is_some());
        }

        for kind in [
            ArtifactProviderKind::Rust,
            ArtifactProviderKind::Java,
            ArtifactProviderKind::Node,
            ArtifactProviderKind::Python,
            ArtifactProviderKind::Go,
        ] {
            assert!(catalogs.artifact.find_for_kind(kind).is_some());
        }

        for kind in [
            ImageProviderKind::Buildroot,
            ImageProviderKind::StartingPoint,
        ] {
            assert!(catalogs.image.find_for_kind(kind).is_some());
        }
    }
}
