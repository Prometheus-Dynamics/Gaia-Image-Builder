use super::*;
use gaia_spec::{CargoGitSourceSpec, SourceDefinition, SourceId, SourceSpec};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn compile_artifact(raw: RawArtifactConfig, sources: &[SourceSpec]) -> ArtifactSpec {
    let mut extra_sources = compile_extra_sources(&raw.sources, raw.source.as_deref());
    let definition = match raw.definition {
        RawArtifactDefinition::Rust {
            package,
            target_name,
            emit_directory,
            features,
            no_default_features,
            all_features,
        } => {
            let cargo_git_sources =
                compile_cargo_git_sources(raw.cargo_git_sources.clone(), sources);
            // The build reads the git sources its cargo git dependencies come
            // from, so the artifact depends on each of them.
            extra_sources = merge_extra_sources(
                extra_sources,
                cargo_git_sources.iter().map(|source| source.source.clone()),
                raw.source.as_deref(),
            );
            ArtifactDefinition::Rust(RustArtifactSpec {
                package,
                target_name,
                variant: if emit_directory {
                    ArtifactVariantSpec::Directory
                } else {
                    ArtifactVariantSpec::File
                },
                features,
                no_default_features,
                all_features,
                build_group: raw.build_group,
                group_packages: Vec::new(),
                cargo_git_sources,
            })
        }
        RawArtifactDefinition::Java {
            build_target,
            build_args,
            build_command,
            build_env,
        } => ArtifactDefinition::Java(JavaArtifactSpec {
            build_target,
            build_args,
            build_command,
            build_env,
        }),
        RawArtifactDefinition::Node { package_dir } => {
            ArtifactDefinition::Node(NodeArtifactSpec { package_dir })
        }
        RawArtifactDefinition::Python { package_dir } => {
            ArtifactDefinition::Python(PythonArtifactSpec { package_dir })
        }
        RawArtifactDefinition::Go { package } => ArtifactDefinition::Go(GoArtifactSpec { package }),
    };
    ArtifactSpec {
        id: raw.id.into(),
        definition,
        source: raw.source.map(SourceRef::new),
        extra_sources,
        execution: compile_artifact_execution(raw.execution),
        target: raw.target,
        build_mode: raw.profile.map(compile_build_mode),
        dependencies: raw.dependencies.into_iter().map(ArtifactRef::new).collect(),
        after_image_prepare: raw.after_image_prepare,
        install_identity: raw
            .install_name
            .map(|install_name| ArtifactInstallIdentitySpec {
                install_name,
                install_class: compile_artifact_install_class(raw.install_class),
                destination_hint: raw.install_dest_hint,
            }),
        output: ArtifactOutputSpec {
            path: raw.output_path,
        },
    }
}

/// The `sources` an artifact names, sorted and without duplicates, and
/// without its own `source` (which it already depends on).
pub(crate) fn compile_extra_sources(sources: &[String], own: Option<&str>) -> Vec<SourceId> {
    let mut extra = sources
        .iter()
        .filter(|id| Some(id.as_str()) != own)
        .map(|id| SourceId::new(id.as_str()))
        .collect::<Vec<_>>();
    extra.sort();
    extra.dedup();
    extra
}

/// Adds `more` to an artifact's extra sources: sorted, without duplicates,
/// and without its own `own` source.
fn merge_extra_sources(
    extra: Vec<SourceId>,
    more: impl Iterator<Item = SourceId>,
    own: Option<&str>,
) -> Vec<SourceId> {
    let mut merged = extra
        .into_iter()
        .chain(more)
        .filter(|id| Some(id.as_str()) != own)
        .collect::<Vec<_>>();
    merged.sort();
    merged.dedup();
    merged
}

/// `cargo_git_sources` as compiled: sorted by URL, each with the repository
/// of the git source it names. A source that is not a declared git source
/// keeps an empty repository; validation reports it.
fn compile_cargo_git_sources(
    mapping: BTreeMap<String, String>,
    sources: &[SourceSpec],
) -> Vec<CargoGitSourceSpec> {
    mapping
        .into_iter()
        .map(|(url, source)| {
            let repo = sources
                .iter()
                .find(|candidate| candidate.id.as_str() == source)
                .and_then(|candidate| match &candidate.definition {
                    SourceDefinition::Git(git) => Some(git.repo.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            CargoGitSourceSpec {
                url,
                source: SourceId::new(source),
                repo,
            }
        })
        .collect()
}

/// Gives every member of a rust build group the same cargo selection: the
/// sorted packages of all members and the sorted union of their features.
/// Each member keeps its own `package` and `target_name` for collecting its
/// output.
pub(crate) fn complete_build_groups(artifacts: &mut [ArtifactSpec]) {
    let mut groups = BTreeMap::<String, (BTreeSet<String>, BTreeSet<String>)>::new();
    for artifact in artifacts.iter() {
        if let ArtifactDefinition::Rust(rust) = &artifact.definition
            && let Some(group) = &rust.build_group
        {
            let (packages, features) = groups.entry(group.clone()).or_default();
            packages.insert(rust.package.clone());
            features.extend(rust.features.iter().cloned());
        }
    }
    for artifact in artifacts.iter_mut() {
        if let ArtifactDefinition::Rust(rust) = &mut artifact.definition
            && let Some((packages, features)) = rust
                .build_group
                .as_ref()
                .and_then(|group| groups.get(group))
        {
            rust.group_packages = packages.iter().cloned().collect();
            rust.features = features.iter().cloned().collect();
        }
    }
}

pub(crate) fn compile_artifact_execution(
    raw: crate::raw::RawArtifactExecutionConfig,
) -> Option<ArtifactExecutionSpec> {
    let docker = DockerArtifactExecutionSpec {
        image: raw.docker.image,
        dockerfile: raw.docker.dockerfile,
        context: raw.docker.context,
    };
    match raw.backend {
        Some(crate::raw::RawArtifactExecutionBackend::Host) => Some(ArtifactExecutionSpec::Host),
        Some(crate::raw::RawArtifactExecutionBackend::Docker) => {
            Some(ArtifactExecutionSpec::Docker(docker))
        }
        None if docker.image.is_some() || docker.dockerfile.is_some() => {
            Some(ArtifactExecutionSpec::Docker(docker))
        }
        None => None,
    }
}

pub(crate) fn compile_build_mode(mode: String) -> BuildModeSpec {
    match mode.as_str() {
        "debug" => BuildModeSpec::Debug,
        "release" => BuildModeSpec::Release,
        _ => BuildModeSpec::Custom(mode),
    }
}

pub(crate) fn compile_artifact_install_class(
    raw: Option<RawArtifactInstallClass>,
) -> ArtifactInstallClassSpec {
    match raw.unwrap_or(RawArtifactInstallClass::Binary) {
        RawArtifactInstallClass::Binary => ArtifactInstallClassSpec::Binary,
        RawArtifactInstallClass::Library => ArtifactInstallClassSpec::Library,
        RawArtifactInstallClass::Archive => ArtifactInstallClassSpec::Archive,
        RawArtifactInstallClass::Config => ArtifactInstallClassSpec::Config,
        RawArtifactInstallClass::Service => ArtifactInstallClassSpec::Service,
        RawArtifactInstallClass::Data => ArtifactInstallClassSpec::Data,
    }
}
