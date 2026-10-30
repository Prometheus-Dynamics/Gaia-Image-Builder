use crate::StageItemId;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct StageSpec {
    pub files: Vec<StageFileSpec>,
    pub env_sets: Vec<StageEnvSetSpec>,
    pub services: Vec<StageServiceSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StageFileSpec {
    pub id: StageItemId,
    /// Empty when `content` is set.
    pub src: String,
    /// Text written to `dest` in place of a copied `src`, with config tokens
    /// (`${build.version}`) already rendered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub dest: String,
    pub mode: Option<u32>,
    pub origin: StageContentOriginSpec,
}

impl StageFileSpec {
    pub fn new(
        id: impl Into<StageItemId>,
        src: impl Into<String>,
        dest: impl Into<String>,
        origin: StageContentOriginSpec,
    ) -> Self {
        Self {
            id: id.into(),
            src: src.into(),
            content: None,
            dest: dest.into(),
            mode: None,
            origin,
        }
    }

    pub fn static_asset(
        id: impl Into<StageItemId>,
        src: impl Into<String>,
        dest: impl Into<String>,
    ) -> Self {
        Self::new(id, src, dest, StageContentOriginSpec::StaticAsset)
    }

    pub fn generated(
        id: impl Into<StageItemId>,
        src: impl Into<String>,
        dest: impl Into<String>,
    ) -> Self {
        Self::new(id, src, dest, StageContentOriginSpec::Generated)
    }

    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.src.clear();
        self.content = Some(content.into());
        self
    }

    pub fn dest_path(&self) -> &Path {
        Path::new(&self.dest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StageEnvSetSpec {
    pub id: StageItemId,
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl StageEnvSetSpec {
    pub fn new(id: impl Into<StageItemId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StageServiceSpec {
    pub id: StageItemId,
    pub name: String,
    pub unit_path: String,
}

impl StageServiceSpec {
    pub fn new(
        id: impl Into<StageItemId>,
        name: impl Into<String>,
        unit_path: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            unit_path: unit_path.into(),
        }
    }

    pub fn unit_path(&self) -> &Path {
        Path::new(&self.unit_path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum StageContentOriginSpec {
    StaticAsset,
    Generated,
    ProviderEmitted,
}

impl StageContentOriginSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StaticAsset => "static-asset",
            Self::Generated => "generated",
            Self::ProviderEmitted => "provider-emitted",
        }
    }
}

impl std::fmt::Display for StageContentOriginSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_constructors_make_common_cases_explicit() {
        let file = StageFileSpec::static_asset("motd", "assets/motd", "/etc/motd");
        let generated = StageFileSpec::generated("hostname", "generated/hostname", "/etc/hostname");
        let env = StageEnvSetSpec::new("env", "demo");
        let service = StageServiceSpec::new("service", "demo.service", "assets/demo.service");

        assert_eq!(file.origin, StageContentOriginSpec::StaticAsset);
        assert_eq!(generated.origin, StageContentOriginSpec::Generated);
        assert!(env.entries.is_empty());
        assert_eq!(service.unit_path(), Path::new("assets/demo.service"));
    }
}
