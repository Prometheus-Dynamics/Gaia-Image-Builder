/// Where a successful `gaia run` copies its primary image (`--export`
/// without the flag). Both directories are absolute once config resolves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportSpec {
    /// `image.output.export_dir`, the build's own setting.
    pub image_dir: Option<String>,
    /// `workspace.export_dir`, the default for every build in the workspace.
    pub workspace_dir: Option<String>,
}

impl ExportSpec {
    /// The configured export directory: the image's when set, otherwise the
    /// workspace's.
    pub fn configured_dir(&self) -> Option<&str> {
        self.image_dir.as_deref().or(self.workspace_dir.as_deref())
    }
}
