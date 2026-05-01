mod artifact;
mod checkpoint;
mod image;
mod policy;
mod precedence;
mod source;
mod when;

use artifact::compile_artifact;
use checkpoint::{compile_checkpoint, compile_stage_content_origin};
use image::{
    compile_buildroot_expected_image_format, compile_buildroot_external_tree_mode,
    compile_image_feed, compile_rootfs_validation_mode, compile_starting_point_output_mode,
};
use policy::{
    compile_backoff_strategy, compile_command_policy, compile_docker_execution, compile_input_kind,
    compile_output_retention, compile_provider_retry_attempts, compile_provider_retry_backoff_ms,
    compile_provider_timeout_seconds, compile_rollback_domains,
};
use precedence::{precedence_layers, selection_precedence_order};
use source::{compile_source_pin_policy, compile_source_refresh_policy};
use when::apply_when_selection;

use crate::raw::{
    RawArtifactConfig, RawArtifactDefinition, RawArtifactInstallClass, RawBuildConfig,
    RawBuildrootExpectedImageFormat, RawBuildrootExternalTreeMode, RawCheckpointConfig,
    RawCheckpointPolicy, RawImageDefinition, RawRollbackDomain, RawSourceDefinition,
    RawSourcePinPolicy, RawSourceRefreshPolicy, RawStageContentOrigin, RawStartingPointOutputMode,
    RawStartingPointRootfsValidationMode, RawWhenConfig, RawWhenImageKind, RawWorkspacePathKind,
};

use gaia_spec::{
    ArtifactDefinition, ArtifactExecutionSpec, ArtifactInstallClassSpec,
    ArtifactInstallIdentitySpec, ArtifactOutputSpec, ArtifactRef, ArtifactSpec,
    ArtifactVariantSpec, BuildMetadataSpec, BuildModeSpec, BuildPolicySpec,
    BuildrootExpectedImageFormatSpec, BuildrootExpectedImageSpec, BuildrootExternalTreeModeSpec,
    BuildrootImageSpec, CheckpointAnchorRef, CheckpointBackendRef, CheckpointId,
    CheckpointPointSpec, CheckpointPolicy, CleanProfileSpec, CleanSpec, CommandProviderPolicySpec,
    DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS, DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS, DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS, DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS, DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS, DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS,
    DockerArtifactExecutionSpec, DockerExecutionSpec, ExecutionPolicySpec,
    FailureHandlingPolicySpec, GitProviderPolicySpec, GitSourceSpec, GoArtifactSpec,
    ImageDefinition, ImageFeedSpec, ImageOutputSpec, ImageSpec, InputKindSpec, InputOptionSpec,
    InputSpec, InstallEntrySpec, InstallId, InterpolationSpec, JavaArtifactSpec, NodeArtifactSpec,
    OutputRetentionPolicySpec, PathSourceSpec, PostBuildHookSpec, PrecedenceLayerSpec,
    PrecedencePolicySpec, PrecedenceSource, PrecedenceTarget, PresetSelectionSpec,
    ProductIdentitySpec, ProvenanceIdentitySpec, ProvenanceSpec, ProviderExecutionPolicySpec,
    PythonArtifactSpec, ReportingOutputsSpec, ReportingSpec, ResolvedBuildSpec,
    RetryBackoffStrategySpec, RollbackDomain, RustArtifactSpec, RustProviderPolicySpec,
    SecretMaskingSpec, SelectionSpec, SourceDefinition, SourcePinPolicySpec, SourceRef,
    SourceRefreshPolicySpec, SourceSpec, StageContentOriginSpec, StageEnvSetSpec, StageFileSpec,
    StageItemId, StageServiceSpec, StartingPointImageSpec, StartingPointOutputModeSpec,
    StartingPointRootfsValidationModeSpec, UnresolvedInterpolationSpec, WorkspaceNamedPathSpec,
    WorkspacePathKindSpec, WorkspaceSpec,
};

pub fn compile_config(mut raw: RawBuildConfig) -> ResolvedBuildSpec {
    apply_when_selection(&mut raw);
    let precedence_order = selection_precedence_order(&raw);
    let precedence_layers = precedence_layers(&raw);
    let applied_presets = raw.preset.clone().into_iter().collect();
    let compiled_image_feed = compile_image_feed(&raw);
    let mut spec = ResolvedBuildSpec::new(raw.build_name);
    spec.identity.display_name = raw
        .display_name
        .clone()
        .unwrap_or_else(|| spec.identity.build_name.clone());
    spec.identity.version = raw.version;
    spec.selection = SelectionSpec {
        requested_build: raw.requested_build,
        selected_build_file: raw
            .source_path
            .as_ref()
            .map(|path| path.display().to_string()),
        selected_preset: raw.preset.clone(),
        selected_inputs: raw.selected_inputs.clone(),
        env_files: raw.env_files.clone(),
        env_overrides: raw.env_overrides,
        explicit_overrides: raw.explicit_overrides,
        precedence_order,
    };
    spec.metadata = BuildMetadataSpec {
        version: spec.identity.version.clone(),
        description: raw.description,
        branch: raw.branch,
        target: raw.target,
        profile: raw.profile,
        labels: raw.labels,
        product: ProductIdentitySpec {
            family: raw.product.family,
            name: raw.product.name,
            sku: raw.product.sku,
        },
    };
    spec.inputs = InputSpec {
        declared: raw
            .inputs
            .iter()
            .map(|(name, input)| InputOptionSpec {
                name: name.clone(),
                description: input.description.clone(),
                kind: compile_input_kind(input.kind),
                required: input.required,
                default: input.default.clone(),
                choices: input.choices.clone(),
            })
            .collect(),
        selected: raw.selected_inputs.clone(),
    };
    spec.policy = BuildPolicySpec {
        preset: PresetSelectionSpec {
            selected: raw.preset.clone(),
            applied: applied_presets,
        },
        interpolation: InterpolationSpec {
            allow_unresolved: raw.interpolation.allow_unresolved,
            values: raw.interpolation.values,
            unresolved: raw
                .unresolved_tokens
                .into_iter()
                .map(|token| UnresolvedInterpolationSpec {
                    location: token.location,
                    token: token.token,
                })
                .collect(),
        },
        precedence: PrecedencePolicySpec {
            layers: precedence_layers,
        },
        execution: ExecutionPolicySpec {
            jobs: raw.execution.jobs,
            docker: compile_docker_execution(&raw.execution),
            output_retention: compile_output_retention(&raw.execution.output_retention),
        },
        failure: FailureHandlingPolicySpec {
            rollback_on_error: raw.failure.rollback_on_error.unwrap_or(true),
            preserve_failed_outputs: raw.failure.preserve_failed_outputs.unwrap_or(false),
            rollback_domains: compile_rollback_domains(raw.failure.rollback_domains),
        },
        providers: ProviderExecutionPolicySpec {
            rust: RustProviderPolicySpec {
                allow_nested_build: raw.providers.rust.allow_nested_build,
                retry_attempts: compile_provider_retry_attempts(raw.providers.rust.retry_attempts),
                retry_backoff_ms: compile_provider_retry_backoff_ms(
                    raw.providers.rust.retry_backoff_ms,
                ),
                retry_backoff_strategy: compile_backoff_strategy(
                    raw.providers.rust.retry_backoff_strategy,
                ),
                timeout_seconds: compile_provider_timeout_seconds(
                    raw.providers.rust.timeout_seconds,
                    DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS,
                ),
            },
            git: GitProviderPolicySpec {
                allow_remote_resolution: raw.providers.git.allow_remote_resolution,
                retry_attempts: compile_provider_retry_attempts(raw.providers.git.retry_attempts),
                retry_backoff_ms: compile_provider_retry_backoff_ms(
                    raw.providers.git.retry_backoff_ms,
                ),
                retry_backoff_strategy: compile_backoff_strategy(
                    raw.providers.git.retry_backoff_strategy,
                ),
                timeout_seconds: compile_provider_timeout_seconds(
                    raw.providers.git.timeout_seconds,
                    DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS,
                ),
            },
            archive: compile_command_policy(
                &raw.providers.archive,
                DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS,
            ),
            download: compile_command_policy(
                &raw.providers.download,
                DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS,
            ),
            go: compile_command_policy(&raw.providers.go, DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS),
            java: compile_command_policy(
                &raw.providers.java,
                DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS,
            ),
            node: compile_command_policy(
                &raw.providers.node,
                DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS,
            ),
            python: compile_command_policy(
                &raw.providers.python,
                DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS,
            ),
            buildroot: compile_command_policy(
                &raw.providers.buildroot,
                DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS,
            ),
            starting_point: compile_command_policy(
                &raw.providers.starting_point,
                DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS,
            ),
        },
    };
    spec.provenance = ProvenanceSpec {
        identity: ProvenanceIdentitySpec {
            project: raw.provenance.identity.project,
            vendor: raw.provenance.identity.vendor,
            channel: raw.provenance.identity.channel,
            labels: raw.provenance.identity.labels,
        },
    };
    spec.clean = CleanSpec {
        default_profile: raw.clean.default,
        profiles: raw
            .clean
            .profiles
            .into_iter()
            .map(|(name, profile)| {
                (
                    name,
                    CleanProfileSpec {
                        description: profile.description,
                        build: profile.build,
                        out: profile.out,
                        paths: profile.paths,
                    },
                )
            })
            .collect(),
    };
    spec.workspace = WorkspaceSpec {
        root_dir: raw.workspace.root_dir,
        build_dir: raw.workspace.build_dir,
        out_dir: raw.workspace.out_dir,
        clean_policy: gaia_spec::CleanPolicy::None,
        named_paths: raw
            .workspace
            .named_paths
            .into_iter()
            .map(|named_path| WorkspaceNamedPathSpec {
                alias: named_path.alias,
                path: named_path.path,
                kind: match named_path.kind {
                    RawWorkspacePathKind::Host => WorkspacePathKindSpec::Host,
                    RawWorkspacePathKind::Logical => WorkspacePathKindSpec::Logical,
                },
            })
            .collect(),
    };
    spec.sources = raw
        .sources
        .into_iter()
        .map(|source| {
            let definition = match source.definition {
                RawSourceDefinition::Git {
                    repo,
                    branch,
                    tag,
                    rev,
                    subdir,
                    update,
                    refresh,
                    pin,
                } => {
                    let default_pin_policy = if rev.is_some() || tag.is_some() {
                        SourcePinPolicySpec::Locked
                    } else {
                        SourcePinPolicySpec::Floating
                    };
                    SourceDefinition::Git(GitSourceSpec {
                        repo,
                        branch,
                        tag,
                        rev,
                        subdir,
                        update,
                        refresh_policy: compile_source_refresh_policy(
                            refresh,
                            if update {
                                SourceRefreshPolicySpec::Always
                            } else {
                                SourceRefreshPolicySpec::Auto
                            },
                        ),
                        pin_policy: compile_source_pin_policy(pin, default_pin_policy),
                    })
                }
                RawSourceDefinition::Path {
                    path,
                    identity_ignore,
                    refresh,
                    pin,
                } => SourceDefinition::Path(PathSourceSpec {
                    path,
                    identity_ignore,
                    refresh_policy: compile_source_refresh_policy(
                        refresh,
                        SourceRefreshPolicySpec::Auto,
                    ),
                    pin_policy: compile_source_pin_policy(pin, SourcePinPolicySpec::Floating),
                }),
                RawSourceDefinition::Archive {
                    path,
                    strip_components,
                    refresh,
                    pin,
                } => SourceDefinition::Archive(gaia_spec::ArchiveSourceSpec {
                    path,
                    strip_components,
                    refresh_policy: compile_source_refresh_policy(
                        refresh,
                        SourceRefreshPolicySpec::Auto,
                    ),
                    pin_policy: compile_source_pin_policy(pin, SourcePinPolicySpec::Floating),
                }),
                RawSourceDefinition::Download {
                    url,
                    sha256,
                    output_path,
                    refresh,
                    pin,
                } => {
                    let default_pin_policy = if sha256.is_some() {
                        SourcePinPolicySpec::Locked
                    } else {
                        SourcePinPolicySpec::Floating
                    };
                    SourceDefinition::Download(gaia_spec::DownloadSourceSpec {
                        url,
                        sha256,
                        output_path,
                        refresh_policy: compile_source_refresh_policy(
                            refresh,
                            SourceRefreshPolicySpec::Auto,
                        ),
                        pin_policy: compile_source_pin_policy(pin, default_pin_policy),
                    })
                }
            };
            SourceSpec::new(source.id, definition)
        })
        .collect();
    spec.artifacts = raw.artifacts.into_iter().map(compile_artifact).collect();
    spec.install.entries = raw
        .install
        .into_iter()
        .map(|install| InstallEntrySpec {
            id: InstallId::new(install.id),
            artifact: ArtifactRef::new(install.artifact),
            dest: install.dest,
            replace: install.replace,
            mode: install.mode,
            owner: install.owner,
            group: install.group,
        })
        .collect();
    spec.stage.files = raw
        .stage
        .files
        .into_iter()
        .map(|file| StageFileSpec {
            id: StageItemId::new(file.id),
            src: file.src,
            dest: file.dest,
            origin: compile_stage_content_origin(file.origin),
        })
        .collect();
    spec.stage.env_sets = raw
        .stage
        .env_sets
        .into_iter()
        .map(|env| StageEnvSetSpec {
            id: StageItemId::new(env.id),
            name: env.name,
            entries: env.entries,
        })
        .collect();
    spec.stage.services = raw
        .stage
        .services
        .into_iter()
        .map(|service| StageServiceSpec {
            id: StageItemId::new(service.id),
            name: service.name,
            unit_path: service.unit_path,
        })
        .collect();
    spec.image = ImageSpec {
        definition: match raw.image.definition {
            RawImageDefinition::Buildroot {
                source,
                defconfig,
                defconfig_path,
                allow_fallback,
                config_fragments,
                config_overrides,
                external_tree,
                external_tree_mode,
                expected_images,
            } => ImageDefinition::Buildroot(BuildrootImageSpec {
                source: source.map(gaia_spec::SourceId::new),
                defconfig,
                defconfig_path,
                allow_fallback,
                config_fragments,
                config_overrides,
                external_tree,
                external_tree_mode: compile_buildroot_external_tree_mode(external_tree_mode),
                expected_images: expected_images
                    .into_iter()
                    .map(|image| BuildrootExpectedImageSpec {
                        name: image.name,
                        format: compile_buildroot_expected_image_format(image.format),
                        required: image.required,
                    })
                    .collect(),
            }),
            RawImageDefinition::StartingPoint {
                source,
                source_path,
                rootfs_path,
                image_partition,
                image_read_only,
                packages,
                rootfs_validation_mode,
                output_mode,
            } => ImageDefinition::StartingPoint(StartingPointImageSpec {
                source: source.map(gaia_spec::SourceId::new),
                source_path,
                rootfs_path,
                image_partition,
                image_read_only,
                packages: gaia_spec::StartingPointPackagesSpec {
                    enabled: packages.enabled,
                    execute: packages.execute,
                    manager: packages.manager,
                    release_version: packages.release_version,
                    allow_major_upgrade: packages.allow_major_upgrade,
                    update: packages.update,
                    dist_upgrade: packages.dist_upgrade,
                    install: packages.install,
                    remove: packages.remove,
                    extra_args: packages.extra_args,
                    os_release_path: packages.os_release_path,
                },
                rootfs_validation_mode: compile_rootfs_validation_mode(rootfs_validation_mode),
                output_mode: compile_starting_point_output_mode(output_mode),
            }),
        },
        feed: compiled_image_feed,
        output: ImageOutputSpec {
            collect_dir: raw.image.output.collect_dir,
            archive_name: raw.image.output.archive_name,
            emit_report: raw.image.output.emit_report,
        },
    };
    spec.checkpoints.points = raw
        .checkpoints
        .into_iter()
        .map(compile_checkpoint)
        .collect();
    spec.reporting = ReportingSpec {
        outputs: ReportingOutputsSpec {
            summary: raw.reporting.summary,
            provenance: raw.reporting.provenance,
            manifest: raw.reporting.manifest,
        },
        masking: SecretMaskingSpec {
            enabled: raw.reporting.masking.enabled,
            replacement: raw.reporting.masking.replacement,
            patterns: raw.reporting.masking.patterns,
        },
        post_build: raw.reporting.post_build.and_then(|hook| {
            if hook.script.trim().is_empty() {
                None
            } else {
                Some(PostBuildHookSpec {
                    script: hook.script,
                    timeout_seconds: hook.timeout_seconds,
                })
            }
        }),
    };
    spec
}
