use crate::{
    AssemblyPathTemplate, AssemblyPathTemplateContext, ImageAssemblySpec, ResolvedBuildSpec,
    resolve_workspace_path,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The keywords `[image.assembly] work_dir` accepts in place of a path:
/// `ram` keeps the assembly's intermediates in RAM when the build allows it,
/// `disk` keeps them in the build dir (the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyWorkDirKeyword {
    Ram,
    Disk,
}

impl AssemblyWorkDirKeyword {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ram => "ram",
            Self::Disk => "disk",
        }
    }
}

/// The keyword a `work_dir` value is, if it is one (not a path).
pub fn assembly_work_dir_keyword(raw: &str) -> Option<AssemblyWorkDirKeyword> {
    match raw {
        "ram" => Some(AssemblyWorkDirKeyword::Ram),
        "disk" => Some(AssemblyWorkDirKeyword::Disk),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyRoots {
    pub assembly_work: PathBuf,
    pub assembly_out: PathBuf,
    pub provider_images: PathBuf,
    pub provider_target: PathBuf,
    pub provider_host: Option<PathBuf>,
    pub provider_staging: Option<PathBuf>,
    pub trees: BTreeMap<crate::AssemblyTreeId, PathBuf>,
}

impl AssemblyRoots {
    pub fn new(spec: &ResolvedBuildSpec, assembly: &ImageAssemblySpec) -> Result<Self, String> {
        let default_collect_dir = crate::default_image_collect_dir(&spec.workspace);
        let provider_images = resolve_workspace_path_or_absolute(
            spec,
            spec.image
                .output
                .collect_dir
                .as_deref()
                .unwrap_or(&default_collect_dir),
        )?;
        let buildroot_output = buildroot_output_dir(spec)?;
        let provider_target = match spec.image.provider_kind() {
            crate::ImageProviderKind::Buildroot => buildroot_output.join("target"),
            crate::ImageProviderKind::StartingPoint => provider_images.join("rootfs"),
        };
        let provider_host = (spec.image.provider_kind() == crate::ImageProviderKind::Buildroot)
            .then(|| buildroot_output.join("host"));
        let provider_staging = (spec.image.provider_kind() == crate::ImageProviderKind::Buildroot)
            .then(|| buildroot_output.join("staging"));
        let default_assembly_work = resolve_workspace_path_or_absolute(
            spec,
            &format!("{}/assembly", spec.workspace.build_dir),
        )?;
        // `ram` and `disk` choose where the work dir is placed at run time
        // (see `assembly_work_dir_keyword`); the spec's own view of it is
        // the build dir's default.
        let assembly_work = match assembly.work_dir.as_deref() {
            Some(work_dir) if assembly_work_dir_keyword(work_dir).is_none() => {
                resolve_assembly_path_without_trees(
                    spec,
                    work_dir,
                    &provider_images,
                    &provider_target,
                    provider_host.as_deref(),
                    provider_staging.as_deref(),
                    &provider_images,
                )?
            }
            _ => default_assembly_work,
        };
        let assembly_out = match assembly.out_dir.as_deref() {
            Some(out_dir) => resolve_assembly_path_without_trees(
                spec,
                out_dir,
                &provider_images,
                &provider_target,
                provider_host.as_deref(),
                provider_staging.as_deref(),
                &provider_images,
            )?,
            None => provider_images.clone(),
        };
        let mut roots = Self {
            assembly_work,
            assembly_out,
            provider_images,
            provider_target,
            provider_host,
            provider_staging,
            trees: BTreeMap::new(),
        };
        for tree in &assembly.trees {
            let path = roots.resolve_path(spec, &tree.path)?;
            roots.trees.insert(tree.id.clone(), path);
        }
        Ok(roots)
    }

    pub fn tree_path(&self, id: impl AsRef<str>) -> Result<&Path, String> {
        let id = id.as_ref();
        self.trees
            .get(id)
            .map(PathBuf::as_path)
            .ok_or_else(|| format!("assembly tree '{id}' is not defined"))
    }

    pub fn resolve_path(
        &self,
        spec: &ResolvedBuildSpec,
        raw: impl AsRef<str>,
    ) -> Result<PathBuf, String> {
        let resolved = AssemblyPathTemplate::new(raw.as_ref())
            .resolve(&self.template_context(spec))
            .map_err(|error| error.to_string())?;
        resolve_workspace_path_or_absolute(spec, &resolved)
    }

    fn template_context(&self, spec: &ResolvedBuildSpec) -> AssemblyPathTemplateContext<'_> {
        AssemblyPathTemplateContext {
            provider_kind: spec.image.provider_kind(),
            provider_images: &self.provider_images,
            provider_buildroot_output: (spec.image.provider_kind()
                == crate::ImageProviderKind::Buildroot)
                .then_some(
                    self.provider_target
                        .parent()
                        .unwrap_or(&self.provider_target),
                ),
            provider_target: &self.provider_target,
            provider_host: self.provider_host.as_deref(),
            provider_staging: self.provider_staging.as_deref(),
            assembly_work: &self.assembly_work,
            assembly_out: &self.assembly_out,
            trees: &self.trees,
        }
    }
}

fn buildroot_output_dir(spec: &ResolvedBuildSpec) -> Result<PathBuf, String> {
    resolve_workspace_path_or_absolute(
        spec,
        &format!("{}/image/buildroot-output", spec.workspace.build_dir),
    )
}

pub fn expand_simple_glob(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    pattern: &str,
) -> Result<Vec<PathBuf>, String> {
    let resolved = AssemblyPathTemplate::new(pattern)
        .resolve(&roots.template_context(spec))
        .map_err(|error| error.to_string())?;
    let resolved_pattern = resolve_workspace_path_or_absolute(spec, &resolved)?;
    if !resolved_pattern.to_string_lossy().contains('*') {
        return Ok(vec![resolved_pattern]);
    }

    let components: Vec<_> = resolved_pattern.components().collect();
    let mut matches = expand_glob_components(&components, 0, PathBuf::new())?;
    matches.sort_by_key(|path| path.display().to_string());
    Ok(matches)
}

fn expand_glob_components(
    components: &[Component<'_>],
    index: usize,
    current: PathBuf,
) -> Result<Vec<PathBuf>, String> {
    if index >= components.len() {
        return Ok(vec![current]);
    }

    match components[index] {
        Component::Prefix(prefix) => {
            let mut next = current;
            next.push(prefix.as_os_str());
            expand_glob_components(components, index + 1, next)
        }
        Component::RootDir => {
            let mut next = current;
            next.push(Component::RootDir.as_os_str());
            expand_glob_components(components, index + 1, next)
        }
        Component::CurDir => expand_glob_components(components, index + 1, current),
        Component::ParentDir => {
            let mut next = current;
            next.push("..");
            expand_glob_components(components, index + 1, next)
        }
        Component::Normal(segment) => {
            let segment = segment.to_string_lossy();
            if !segment.contains('*') {
                let mut next = current;
                next.push(segment.as_ref());
                return expand_glob_components(components, index + 1, next);
            }
            let entries = read_glob_dir(&current)?;
            let mut matches = Vec::new();
            for entry in entries {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !wildcard_match(&segment, &name) {
                    continue;
                }
                matches.extend(expand_glob_components(components, index + 1, entry.path())?);
            }
            Ok(matches)
        }
    }
}

fn read_glob_dir(dir: &Path) -> Result<Vec<fs::DirEntry>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "failed to read assembly glob directory '{}': {error}",
                dir.display()
            ));
        }
    };
    entries
        .map(|entry| {
            entry.map_err(|error| {
                format!(
                    "failed to read assembly glob entry in '{}': {error}",
                    dir.display()
                )
            })
        })
        .collect()
}

pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == value;
    }

    let mut remainder = value;
    let mut parts = pattern.split('*').peekable();
    let first = parts.next().unwrap_or_default();
    if !first.is_empty() {
        let Some(stripped) = remainder.strip_prefix(first) else {
            return false;
        };
        remainder = stripped;
    }

    while let Some(part) = parts.next() {
        if part.is_empty() {
            continue;
        }
        if parts.peek().is_none() {
            return remainder.ends_with(part);
        }
        let Some(position) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[position + part.len()..];
    }

    true
}

fn resolve_assembly_path_without_trees(
    spec: &ResolvedBuildSpec,
    raw: &str,
    provider_images: &Path,
    provider_target: &Path,
    provider_host: Option<&Path>,
    provider_staging: Option<&Path>,
    assembly_out_default: &Path,
) -> Result<PathBuf, String> {
    let empty_trees = BTreeMap::new();
    let default_work = resolve_workspace_path_or_absolute(
        spec,
        &format!("{}/assembly", spec.workspace.build_dir),
    )?;
    let context = AssemblyPathTemplateContext {
        provider_kind: spec.image.provider_kind(),
        provider_images,
        provider_buildroot_output: (spec.image.provider_kind()
            == crate::ImageProviderKind::Buildroot)
            .then_some(provider_target.parent().unwrap_or(provider_target)),
        provider_target,
        provider_host,
        provider_staging,
        assembly_work: &default_work,
        assembly_out: assembly_out_default,
        trees: &empty_trees,
    };
    let resolved = AssemblyPathTemplate::new(raw)
        .resolve(&context)
        .map_err(|error| error.to_string())?;
    resolve_workspace_path_or_absolute(spec, &resolved)
}

fn resolve_workspace_path_or_absolute(
    spec: &ResolvedBuildSpec,
    raw: &str,
) -> Result<PathBuf, String> {
    resolve_workspace_path(&spec.workspace, raw).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssemblyTreeSpec, ImageAssemblySpec, ImageOutputSpec, ImageSpec, WorkspaceSpec};

    fn spec_with_image(kind: crate::ImageSpec) -> ResolvedBuildSpec {
        let mut spec = ResolvedBuildSpec::new("assembly-roots");
        spec.workspace = WorkspaceSpec {
            root_dir: "/workspace".into(),
            build_dir: "/workspace/build".into(),
            out_dir: "/workspace/out".into(),
            named_paths: Vec::new(),
            clean_policy: crate::CleanPolicy::None,
        };
        spec.image = kind;
        spec.image.output = ImageOutputSpec {
            collect_dir: Some("/workspace/out/images".into()),
            archive_name: None,
            emit_report: true,
        };
        spec
    }

    #[test]
    fn buildroot_roots_expose_provider_images_target_host_and_staging() {
        let spec = spec_with_image(ImageSpec::new(crate::ImageDefinition::Buildroot(
            crate::BuildrootImageSpec::default(),
        )));
        let assembly = ImageAssemblySpec {
            work_dir: Some("$provider.images/work".into()),
            out_dir: Some("$provider.images".into()),
            trees: vec![AssemblyTreeSpec {
                id: "boot".into(),
                path: "$assembly.work/boot".into(),
            }],
            ..ImageAssemblySpec::default()
        };

        let roots = AssemblyRoots::new(&spec, &assembly).expect("roots");

        assert_eq!(
            roots.provider_images,
            PathBuf::from("/workspace/out/images")
        );
        assert_eq!(
            roots.provider_target,
            PathBuf::from("/workspace/build/image/buildroot-output/target")
        );
        assert_eq!(
            roots.provider_host.as_deref(),
            Some(Path::new("/workspace/build/image/buildroot-output/host"))
        );
        assert_eq!(
            roots.provider_staging.as_deref(),
            Some(Path::new("/workspace/build/image/buildroot-output/staging"))
        );
        assert_eq!(
            roots.tree_path("boot").expect("tree"),
            Path::new("/workspace/out/images/work/boot")
        );
    }

    #[test]
    fn starting_point_roots_expose_only_supported_provider_roots() {
        let spec = spec_with_image(ImageSpec::new(crate::ImageDefinition::StartingPoint(
            crate::StartingPointImageSpec::default(),
        )));
        let assembly = ImageAssemblySpec::default();

        let roots = AssemblyRoots::new(&spec, &assembly).expect("roots");

        assert_eq!(
            roots.provider_target,
            PathBuf::from("/workspace/out/images/rootfs")
        );
        assert!(roots.provider_host.is_none());
        assert!(roots.provider_staging.is_none());
    }

    #[test]
    fn assembly_roots_reject_tree_self_reference() {
        let spec = spec_with_image(ImageSpec::new(crate::ImageDefinition::Buildroot(
            crate::BuildrootImageSpec::default(),
        )));
        let assembly = ImageAssemblySpec {
            trees: vec![AssemblyTreeSpec {
                id: "boot".into(),
                path: "$assembly.tree.boot/nested".into(),
            }],
            ..ImageAssemblySpec::default()
        };

        let error = AssemblyRoots::new(&spec, &assembly).expect_err("self reference");

        assert!(error.contains("unknown tree 'boot'"), "{error}");
    }

    #[test]
    fn work_dir_keywords_are_not_paths() {
        assert_eq!(
            assembly_work_dir_keyword("ram"),
            Some(AssemblyWorkDirKeyword::Ram)
        );
        assert_eq!(
            assembly_work_dir_keyword("disk"),
            Some(AssemblyWorkDirKeyword::Disk)
        );
        assert_eq!(assembly_work_dir_keyword("$provider.images/work"), None);

        // `ram` is not a relative directory: the spec's view of the work dir
        // is the build dir's default, as with no work_dir at all.
        let spec = spec_with_image(ImageSpec::new(crate::ImageDefinition::Buildroot(
            crate::BuildrootImageSpec::default(),
        )));
        let keyword = ImageAssemblySpec {
            work_dir: Some("ram".into()),
            ..ImageAssemblySpec::default()
        };
        let unset = ImageAssemblySpec::default();
        let with_keyword = AssemblyRoots::new(&spec, &keyword).expect("keyword roots");
        let without = AssemblyRoots::new(&spec, &unset).expect("default roots");
        assert_eq!(with_keyword.assembly_work, without.assembly_work);
        assert_eq!(
            with_keyword.assembly_work,
            PathBuf::from("/workspace/build/assembly")
        );
    }
}
