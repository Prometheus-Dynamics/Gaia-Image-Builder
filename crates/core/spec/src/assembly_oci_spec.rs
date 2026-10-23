use crate::{AssemblyPathTemplate, AssemblyTreeId};

/// An OCI image built from layers, written as an OCI image layout directory
/// or, with `format = "docker-archive"`, as a `docker load` tarball.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyOciSpec {
    pub id: String,
    pub dest: AssemblyPathTemplate,
    /// `oci-layout` or `docker-archive`; checked by the validator.
    pub format: String,
    /// Lowest layer first.
    pub layers: Vec<AssemblyOciLayerSpec>,
    /// `gzip`, `zstd` or `none`; checked by the validator.
    pub compression: String,
    /// `org.opencontainers.image.ref.name` (OCI) or the `RepoTags` entry.
    pub reference: Option<String>,
    pub config: AssemblyOciConfigSpec,
}

impl AssemblyOciSpec {
    /// The parsed `format`, or `None` for an unknown value.
    pub fn parsed_format(&self) -> Option<AssemblyOciFormatSpec> {
        AssemblyOciFormatSpec::parse(&self.format)
    }

    /// The parsed `compression`, or `None` for an unknown value.
    pub fn parsed_compression(&self) -> Option<AssemblyOciCompressionSpec> {
        AssemblyOciCompressionSpec::parse(&self.compression)
    }
}

/// One layer. Exactly one of `tar` (an uncompressed layer tar, used as is)
/// and `tree` (an assembly tree, tarred deterministically) must be set.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyOciLayerSpec {
    pub tar: Option<AssemblyPathTemplate>,
    pub tree: Option<AssemblyTreeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyOciLayerSource<'a> {
    Tar(&'a AssemblyPathTemplate),
    Tree(&'a AssemblyTreeId),
}

impl AssemblyOciLayerSpec {
    /// The layer's source, or `None` unless exactly one is set.
    pub fn source(&self) -> Option<AssemblyOciLayerSource<'_>> {
        match (&self.tar, &self.tree) {
            (Some(tar), None) => Some(AssemblyOciLayerSource::Tar(tar)),
            (None, Some(tree)) => Some(AssemblyOciLayerSource::Tree(tree)),
            _ => None,
        }
    }
}

/// The image config: the run settings and labels of the image.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyOciConfigSpec {
    /// `amd64` or `arm64`; required (empty is reported by the validator).
    pub architecture: String,
    /// `linux` (the default); checked by the validator.
    pub os: String,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    /// `KEY=value` strings.
    pub env: Vec<String>,
    pub working_dir: Option<String>,
    pub stop_signal: Option<String>,
    /// Sorted by key.
    pub labels: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AssemblyOciFormatSpec {
    OciLayout,
    DockerArchive,
}

impl AssemblyOciFormatSpec {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "oci-layout" => Some(Self::OciLayout),
            "docker-archive" => Some(Self::DockerArchive),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OciLayout => "oci-layout",
            Self::DockerArchive => "docker-archive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AssemblyOciCompressionSpec {
    Gzip,
    Zstd,
    None,
}

impl AssemblyOciCompressionSpec {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "gzip" => Some(Self::Gzip),
            "zstd" => Some(Self::Zstd),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::None => "none",
        }
    }

    /// The layer media type for this compression.
    pub fn layer_media_type(self) -> &'static str {
        match self {
            Self::Gzip => "application/vnd.oci.image.layer.v1.tar+gzip",
            Self::Zstd => "application/vnd.oci.image.layer.v1.tar+zstd",
            Self::None => "application/vnd.oci.image.layer.v1.tar",
        }
    }
}
