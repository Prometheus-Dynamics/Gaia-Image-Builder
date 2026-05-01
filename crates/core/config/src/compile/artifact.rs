use super::*;

pub(crate) fn compile_artifact(raw: RawArtifactConfig) -> ArtifactSpec {
    let definition = match raw.definition {
        RawArtifactDefinition::Rust {
            package,
            target_name,
            emit_directory,
        } => ArtifactDefinition::Rust(RustArtifactSpec {
            package,
            target_name,
            variant: if emit_directory {
                ArtifactVariantSpec::Directory
            } else {
                ArtifactVariantSpec::File
            },
        }),
        RawArtifactDefinition::Java { build_target } => {
            ArtifactDefinition::Java(JavaArtifactSpec { build_target })
        }
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
        execution: compile_artifact_execution(raw.execution),
        target: raw.target,
        build_mode: raw.profile.map(compile_build_mode),
        dependencies: raw.dependencies.into_iter().map(ArtifactRef::new).collect(),
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

pub(crate) fn compile_artifact_execution(
    raw: crate::raw::RawArtifactExecutionConfig,
) -> Option<ArtifactExecutionSpec> {
    match raw.backend {
        Some(crate::raw::RawArtifactExecutionBackend::Host) => Some(ArtifactExecutionSpec::Host),
        Some(crate::raw::RawArtifactExecutionBackend::Docker) => {
            Some(ArtifactExecutionSpec::Docker(DockerArtifactExecutionSpec {
                image: raw.docker.image,
            }))
        }
        None if raw.docker.image.is_some() => {
            Some(ArtifactExecutionSpec::Docker(DockerArtifactExecutionSpec {
                image: raw.docker.image,
            }))
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
