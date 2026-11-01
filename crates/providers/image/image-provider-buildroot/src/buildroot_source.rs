//! Where the Buildroot tree a build uses is: its `source` (materialized
//! under `<build_dir>/sources/<id>`), else `GAIA_BUILDROOT_DIR` or
//! `BUILDROOT_DIR`.
use super::*;

pub(crate) fn resolve_buildroot_dir(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Option<PathBuf> {
    if let Some(source_id) = buildroot_source_id(image) {
        let candidate = buildroot_source_dir(spec, source_id);
        if candidate.join("Makefile").is_file() {
            return Some(candidate);
        }
    }
    for key in ["GAIA_BUILDROOT_DIR", "BUILDROOT_DIR"] {
        if let Some(candidate) = env::var_os(key).map(PathBuf::from)
            && candidate.join("Makefile").is_file()
        {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn buildroot_source_id(image: &ImageSpec) -> Option<&SourceId> {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.source.as_ref(),
        _ => None,
    }
}

pub(crate) fn buildroot_allow_fallback(image: &ImageSpec) -> bool {
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.allow_fallback,
        _ => false,
    }
}

/// The source's tree: a git, archive or download source is materialized in
/// `<build_dir>/sources/<id>` itself; a path source is a `content` link
/// there to the directory it names (a vendor SDK's prepared Buildroot tree,
/// say).
pub(crate) fn buildroot_source_dir(spec: &ResolvedBuildSpec, source_id: &SourceId) -> PathBuf {
    let dir = Path::new(&spec.workspace.root_dir)
        .join(&spec.workspace.build_dir)
        .join("sources")
        .join(source_id.as_str());
    let linked = dir.join("content");
    if linked.join("Makefile").is_file() {
        linked
    } else {
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::BuildrootImageSpec;
    use gaia_testkit::TempDir;

    #[test]
    fn a_path_source_resolves_through_its_content_link() {
        let scratch = TempDir::new("gaia-buildroot-path-source");
        let tree = scratch.join("sdk/output/buildroot-2024.02.1");
        fs::create_dir_all(&tree).expect("tree");
        fs::write(tree.join("Makefile"), "all:\n").expect("Makefile");
        let source = scratch.join("workspace/build/sources/buildroot");
        fs::create_dir_all(&source).expect("source dir");
        std::os::unix::fs::symlink(&tree, source.join("content")).expect("link");
        let mut spec = ResolvedBuildSpec::new("path-source");
        spec.workspace.root_dir = scratch.join("workspace").display().to_string();
        spec.workspace.build_dir = "build".into();
        let image = ImageSpec {
            signing: None,
            layers: None,
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                source: Some(SourceId::new("buildroot")),
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: gaia_spec::ImageOutputSpec::default(),
            assembly: None,
            checks: gaia_spec::ImageChecksSpec::default(),
            hardware: Vec::new(),
        };
        assert_eq!(
            resolve_buildroot_dir(&spec, &image),
            Some(source.join("content"))
        );
    }
}
