//! The manifest stored with each package-cache entry.

pub(crate) struct Manifest {
    pub(crate) output_dir: String,
    /// Directories (with their modes) and files of the entry, relative to
    /// the output directory.
    pub(crate) dirs: Vec<(String, u32)>,
    pub(crate) files: Vec<String>,
    /// Bytes of its files.
    pub(crate) size: u64,
    pub(crate) stamps: Vec<String>,
    /// Archive paths of text files that hold `output_dir`.
    pub(crate) rewrite: Vec<String>,
    /// False when a binary holds `output_dir`: the package can only be
    /// restored into a tree at the same path.
    pub(crate) relocatable: bool,
}

impl Manifest {
    pub(crate) fn to_json(&self, name: &str, version: Option<&str>) -> String {
        serde_json::json!({
            "package": name,
            "version": version,
            "output_dir": self.output_dir,
            "dirs": self.dirs,
            "files": self.files,
            "size": self.size,
            "stamps": self.stamps,
            "rewrite": self.rewrite,
            "relocatable": self.relocatable,
        })
        .to_string()
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let strings = |field: &str| -> Option<Vec<String>> {
            value
                .get(field)?
                .as_array()?
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect()
        };
        let dirs = value
            .get("dirs")?
            .as_array()?
            .iter()
            .map(|pair| {
                let pair = pair.as_array()?;
                Some((
                    pair.first()?.as_str()?.to_string(),
                    u32::try_from(pair.get(1)?.as_u64()?).ok()?,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            output_dir: value.get("output_dir")?.as_str()?.to_string(),
            dirs,
            files: strings("files")?,
            size: value.get("size")?.as_u64()?,
            stamps: strings("stamps")?,
            rewrite: strings("rewrite")?,
            relocatable: value.get("relocatable")?.as_bool()?,
        })
    }
}
