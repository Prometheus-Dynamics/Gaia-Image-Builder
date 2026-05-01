use crate::{ArtifactRef, InstallId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallSpec {
    pub entries: Vec<InstallEntrySpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallEntrySpec {
    pub id: InstallId,
    pub artifact: ArtifactRef,
    pub dest: String,
    pub replace: bool,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

impl InstallEntrySpec {
    pub fn new(
        id: impl Into<InstallId>,
        artifact: impl Into<ArtifactRef>,
        dest: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            artifact: artifact.into(),
            dest: dest.into(),
            replace: false,
            mode: None,
            owner: None,
            group: None,
        }
    }

    pub fn replacing(mut self, replace: bool) -> Self {
        self.replace = replace;
        self
    }

    pub fn with_mode(mut self, mode: u32) -> Self {
        self.mode = Some(mode);
        self
    }

    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_entry_constructor_sets_safe_defaults() {
        let entry = InstallEntrySpec::new("install-demo", "demo-artifact", "/usr/bin/demo")
            .replacing(true)
            .with_mode(0o755)
            .with_owner("root")
            .with_group("root");

        assert_eq!(entry.id.as_str(), "install-demo");
        assert_eq!(entry.artifact.id.as_str(), "demo-artifact");
        assert_eq!(entry.dest, "/usr/bin/demo");
        assert!(entry.replace);
        assert_eq!(entry.mode, Some(0o755));
        assert_eq!(entry.owner.as_deref(), Some("root"));
        assert_eq!(entry.group.as_deref(), Some("root"));
    }
}
