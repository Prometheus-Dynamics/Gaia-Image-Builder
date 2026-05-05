mod resolver;
mod scanner;

use crate::env::ResolvedEnvironment;
use crate::raw::{
    RawArtifactConfig, RawArtifactDefinition, RawBuildConfig, RawCheckpointConfig,
    RawImageDefinition, RawSourceConfig, RawSourceDefinition, RawWorkspaceNamedPathConfig,
};
use crate::raw_assembly::RawImageAssemblyConfig;

pub fn interpolate_config(raw: RawBuildConfig, env: &ResolvedEnvironment) -> RawBuildConfig {
    let snapshot = raw;
    let mut interpolated = snapshot.clone();

    interpolated.build_name =
        resolver::interpolate_string(snapshot.build_name.clone(), &snapshot, env);
    interpolated.display_name = snapshot
        .display_name
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.description = snapshot
        .description
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.labels = snapshot
        .labels
        .iter()
        .map(|(key, value)| {
            (
                resolver::interpolate_string(key.clone(), &snapshot, env),
                resolver::interpolate_string(value.clone(), &snapshot, env),
            )
        })
        .collect();
    interpolated.product.family = snapshot
        .product
        .family
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.product.name = snapshot
        .product
        .name
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.product.sku = snapshot
        .product
        .sku
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.preset = snapshot
        .preset
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.version = snapshot
        .version
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.branch = snapshot
        .branch
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.target = snapshot
        .target
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.profile = snapshot
        .profile
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.interpolation.values = snapshot
        .interpolation
        .values
        .iter()
        .map(|(key, value)| {
            (
                resolver::interpolate_string(key.clone(), &snapshot, env),
                resolver::interpolate_string(value.clone(), &snapshot, env),
            )
        })
        .collect();
    interpolated.workspace.root_dir =
        resolver::interpolate_string(snapshot.workspace.root_dir.clone(), &snapshot, env);
    interpolated.workspace.build_dir =
        resolver::interpolate_string(snapshot.workspace.build_dir.clone(), &snapshot, env);
    interpolated.workspace.out_dir =
        resolver::interpolate_string(snapshot.workspace.out_dir.clone(), &snapshot, env);
    interpolated.workspace.named_paths = snapshot
        .workspace
        .named_paths
        .iter()
        .map(|entry| RawWorkspaceNamedPathConfig {
            alias: resolver::interpolate_string(entry.alias.clone(), &snapshot, env),
            path: resolver::interpolate_string(entry.path.clone(), &snapshot, env),
            kind: entry.kind,
        })
        .collect();
    interpolated.clean.default = snapshot
        .clean
        .default
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.clean.profiles = snapshot
        .clean
        .profiles
        .iter()
        .map(|(name, profile)| {
            let interpolated_name = resolver::interpolate_string(name.clone(), &snapshot, env);
            let mut profile = profile.clone();
            profile.description = profile
                .description
                .map(|value| resolver::interpolate_string(value, &snapshot, env));
            profile.paths = profile
                .paths
                .into_iter()
                .map(|value| resolver::interpolate_string(value, &snapshot, env))
                .collect();
            (interpolated_name, profile)
        })
        .collect();
    interpolated.env_files = snapshot
        .env_files
        .iter()
        .map(|value| resolver::interpolate_string(value.clone(), &snapshot, env))
        .collect();
    interpolated.env = snapshot
        .env
        .iter()
        .map(|(key, value)| {
            (
                resolver::interpolate_string(key.clone(), &snapshot, env),
                resolver::interpolate_string(value.clone(), &snapshot, env),
            )
        })
        .collect();
    interpolated.sources = snapshot
        .sources
        .iter()
        .cloned()
        .map(|source| interpolate_source(source, &snapshot, env))
        .collect();
    interpolated.artifacts = snapshot
        .artifacts
        .iter()
        .cloned()
        .map(|artifact| interpolate_artifact(artifact, &snapshot, env))
        .collect();
    interpolated.install = snapshot
        .install
        .iter()
        .cloned()
        .map(|mut install| {
            install.id = resolver::interpolate_string(install.id, &snapshot, env);
            install.when = install
                .when
                .map(|when| interpolate_when(when, &snapshot, env));
            install.artifact = resolver::interpolate_string(install.artifact, &snapshot, env);
            install.dest = resolver::interpolate_string(install.dest, &snapshot, env);
            install.owner = install
                .owner
                .map(|value| resolver::interpolate_string(value, &snapshot, env));
            install.group = install
                .group
                .map(|value| resolver::interpolate_string(value, &snapshot, env));
            install
        })
        .collect();
    interpolated.stage.files = snapshot
        .stage
        .files
        .iter()
        .cloned()
        .map(|mut file| {
            file.id = resolver::interpolate_string(file.id, &snapshot, env);
            file.when = file.when.map(|when| interpolate_when(when, &snapshot, env));
            file.src = resolver::interpolate_string(file.src, &snapshot, env);
            file.dest = resolver::interpolate_string(file.dest, &snapshot, env);
            file
        })
        .collect();
    interpolated.stage.env_sets = snapshot
        .stage
        .env_sets
        .iter()
        .cloned()
        .map(|mut env_set| {
            env_set.id = resolver::interpolate_string(env_set.id, &snapshot, env);
            env_set.when = env_set
                .when
                .map(|when| interpolate_when(when, &snapshot, env));
            env_set.name = resolver::interpolate_string(env_set.name, &snapshot, env);
            env_set.entries = env_set
                .entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        resolver::interpolate_string(key, &snapshot, env),
                        resolver::interpolate_string(value, &snapshot, env),
                    )
                })
                .collect();
            env_set
        })
        .collect();
    interpolated.stage.services = snapshot
        .stage
        .services
        .iter()
        .cloned()
        .map(|mut service| {
            service.id = resolver::interpolate_string(service.id, &snapshot, env);
            service.when = service
                .when
                .map(|when| interpolate_when(when, &snapshot, env));
            service.name = resolver::interpolate_string(service.name, &snapshot, env);
            service.unit_path = resolver::interpolate_string(service.unit_path, &snapshot, env);
            service
        })
        .collect();
    interpolated.image.definition =
        interpolate_image_definition(snapshot.image.definition.clone(), &snapshot, env);
    interpolated.image.feed.install_entries = snapshot
        .image
        .feed
        .install_entries
        .iter()
        .cloned()
        .map(|value| resolver::interpolate_string(value, &snapshot, env))
        .collect();
    interpolated.image.feed.stage_files = snapshot
        .image
        .feed
        .stage_files
        .iter()
        .cloned()
        .map(|value| resolver::interpolate_string(value, &snapshot, env))
        .collect();
    interpolated.image.feed.stage_env_sets = snapshot
        .image
        .feed
        .stage_env_sets
        .iter()
        .cloned()
        .map(|value| resolver::interpolate_string(value, &snapshot, env))
        .collect();
    interpolated.image.feed.stage_services = snapshot
        .image
        .feed
        .stage_services
        .iter()
        .cloned()
        .map(|value| resolver::interpolate_string(value, &snapshot, env))
        .collect();
    interpolated.image.output.collect_dir = snapshot
        .image
        .output
        .collect_dir
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.image.output.archive_name = snapshot
        .image
        .output
        .archive_name
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.image.assembly = snapshot
        .image
        .assembly
        .clone()
        .map(|assembly| interpolate_image_assembly(assembly, &snapshot, env));
    interpolated.checkpoints = snapshot
        .checkpoints
        .iter()
        .cloned()
        .map(|checkpoint| interpolate_checkpoint(checkpoint, &snapshot, env))
        .collect();
    interpolated.provenance.identity.project = snapshot
        .provenance
        .identity
        .project
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.provenance.identity.vendor = snapshot
        .provenance
        .identity
        .vendor
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.provenance.identity.channel = snapshot
        .provenance
        .identity
        .channel
        .clone()
        .map(|value| resolver::interpolate_string(value, &snapshot, env));
    interpolated.provenance.identity.labels = snapshot
        .provenance
        .identity
        .labels
        .iter()
        .map(|(key, value)| {
            (
                resolver::interpolate_string(key.clone(), &snapshot, env),
                resolver::interpolate_string(value.clone(), &snapshot, env),
            )
        })
        .collect();
    interpolated.reporting.masking.replacement = resolver::interpolate_string(
        snapshot.reporting.masking.replacement.clone(),
        &snapshot,
        env,
    );
    interpolated.reporting.masking.patterns = snapshot
        .reporting
        .masking
        .patterns
        .iter()
        .map(|value| resolver::interpolate_string(value.clone(), &snapshot, env))
        .collect();
    interpolated.reporting.output_hygiene.transient_dir_names = snapshot
        .reporting
        .output_hygiene
        .transient_dir_names
        .clone()
        .map(|names| {
            names
                .into_iter()
                .map(|value| resolver::interpolate_string(value, &snapshot, env))
                .collect()
        });
    interpolated.reporting.post_build = snapshot.reporting.post_build.clone().map(|mut hook| {
        hook.script = resolver::interpolate_string(hook.script, &snapshot, env);
        hook
    });
    interpolated.unresolved_tokens = scanner::collect_unresolved_tokens(&interpolated);

    interpolated
}

fn interpolate_image_assembly(
    mut assembly: RawImageAssemblyConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawImageAssemblyConfig {
    assembly.work_dir = assembly
        .work_dir
        .map(|value| resolver::interpolate_string(value, raw, env));
    assembly.out_dir = assembly
        .out_dir
        .map(|value| resolver::interpolate_string(value, raw, env));
    assembly.trees = assembly
        .trees
        .into_iter()
        .map(|mut tree| {
            tree.id = resolver::interpolate_string(tree.id, raw, env);
            tree.path = resolver::interpolate_string(tree.path, raw, env);
            tree
        })
        .collect();
    assembly.files = assembly
        .files
        .into_iter()
        .map(|mut file| {
            file.tree = resolver::interpolate_string(file.tree, raw, env);
            file.src = file
                .src
                .map(|value| resolver::interpolate_string(value, raw, env));
            file.src_glob = file
                .src_glob
                .map(|value| resolver::interpolate_string(value, raw, env));
            file.dest = resolver::interpolate_string(file.dest, raw, env);
            file.mode = file
                .mode
                .map(|value| resolver::interpolate_string(value, raw, env));
            file
        })
        .collect();
    assembly.transforms = assembly
        .transforms
        .into_iter()
        .map(|mut transform| {
            transform.src = transform
                .src
                .map(|value| resolver::interpolate_string(value, raw, env));
            transform.dest = resolver::interpolate_string(transform.dest, raw, env);
            transform
        })
        .collect();
    assembly.filesystems = assembly
        .filesystems
        .into_iter()
        .map(|mut filesystem| {
            filesystem.id = resolver::interpolate_string(filesystem.id, raw, env);
            filesystem.source_tree = resolver::interpolate_string(filesystem.source_tree, raw, env);
            filesystem.output = resolver::interpolate_string(filesystem.output, raw, env);
            filesystem.size = filesystem
                .size
                .map(|value| resolver::interpolate_string(value, raw, env));
            filesystem
        })
        .collect();
    assembly.disks = assembly
        .disks
        .into_iter()
        .map(|mut disk| {
            disk.id = resolver::interpolate_string(disk.id, raw, env);
            disk.output = resolver::interpolate_string(disk.output, raw, env);
            disk.signature = disk
                .signature
                .map(|value| resolver::interpolate_string(value, raw, env));
            disk.signature_text = disk
                .signature_text
                .map(|value| resolver::interpolate_string(value, raw, env));
            disk.partitions = disk
                .partitions
                .into_iter()
                .map(|mut partition| {
                    partition.name = resolver::interpolate_string(partition.name, raw, env);
                    partition.kind = partition
                        .kind
                        .map(|value| resolver::interpolate_string(value, raw, env));
                    partition.type_alias = partition
                        .type_alias
                        .map(|value| resolver::interpolate_string(value, raw, env));
                    partition.image = resolver::interpolate_string(partition.image, raw, env);
                    partition
                })
                .collect();
            disk
        })
        .collect();
    assembly.busybox_initramfs = assembly
        .busybox_initramfs
        .into_iter()
        .map(|mut initramfs| {
            initramfs.tree = resolver::interpolate_string(initramfs.tree, raw, env);
            initramfs.busybox = resolver::interpolate_string(initramfs.busybox, raw, env);
            initramfs.applets = initramfs
                .applets
                .into_iter()
                .map(|value| resolver::interpolate_string(value, raw, env))
                .collect();
            initramfs
        })
        .collect();
    assembly
}

fn interpolate_source(
    mut source: RawSourceConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawSourceConfig {
    source.id = resolver::interpolate_string(source.id, raw, env);
    source.definition = match source.definition {
        RawSourceDefinition::Git {
            repo,
            branch,
            tag,
            rev,
            subdir,
            update,
            refresh,
            pin,
        } => RawSourceDefinition::Git {
            repo: resolver::interpolate_string(repo, raw, env),
            branch: branch.map(|value| resolver::interpolate_string(value, raw, env)),
            tag: tag.map(|value| resolver::interpolate_string(value, raw, env)),
            rev: rev.map(|value| resolver::interpolate_string(value, raw, env)),
            subdir: subdir.map(|value| resolver::interpolate_string(value, raw, env)),
            update,
            refresh,
            pin,
        },
        RawSourceDefinition::Path {
            path,
            identity_ignore,
            refresh,
            pin,
        } => RawSourceDefinition::Path {
            path: resolver::interpolate_string(path, raw, env),
            identity_ignore: identity_ignore
                .into_iter()
                .map(|value| resolver::interpolate_string(value, raw, env))
                .collect(),
            refresh,
            pin,
        },
        RawSourceDefinition::Archive {
            path,
            strip_components,
            refresh,
            pin,
        } => RawSourceDefinition::Archive {
            path: resolver::interpolate_string(path, raw, env),
            strip_components,
            refresh,
            pin,
        },
        RawSourceDefinition::Download {
            url,
            sha256,
            output_path,
            refresh,
            pin,
        } => RawSourceDefinition::Download {
            url: resolver::interpolate_string(url, raw, env),
            sha256: sha256.map(|value| resolver::interpolate_string(value, raw, env)),
            output_path: resolver::interpolate_string(output_path, raw, env),
            refresh,
            pin,
        },
    };
    source
}

fn interpolate_artifact(
    mut artifact: RawArtifactConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawArtifactConfig {
    artifact.id = resolver::interpolate_string(artifact.id, raw, env);
    artifact.when = artifact.when.map(|when| interpolate_when(when, raw, env));
    artifact.source = artifact
        .source
        .map(|value| resolver::interpolate_string(value, raw, env));
    artifact.execution.docker.image = artifact
        .execution
        .docker
        .image
        .map(|value| resolver::interpolate_string(value, raw, env));
    artifact.target = artifact
        .target
        .map(|value| resolver::interpolate_string(value, raw, env));
    artifact.profile = artifact
        .profile
        .map(|value| resolver::interpolate_string(value, raw, env));
    artifact.install_name = artifact
        .install_name
        .map(|value| resolver::interpolate_string(value, raw, env));
    artifact.install_dest_hint = artifact
        .install_dest_hint
        .map(|value| resolver::interpolate_string(value, raw, env));
    artifact.dependencies = artifact
        .dependencies
        .into_iter()
        .map(|value| resolver::interpolate_string(value, raw, env))
        .collect();
    artifact.output_path = resolver::interpolate_string(artifact.output_path, raw, env);
    artifact.definition = match artifact.definition {
        RawArtifactDefinition::Rust {
            package,
            target_name,
            emit_directory,
        } => RawArtifactDefinition::Rust {
            package: resolver::interpolate_string(package, raw, env),
            target_name: target_name.map(|value| resolver::interpolate_string(value, raw, env)),
            emit_directory,
        },
        RawArtifactDefinition::Java { build_target } => RawArtifactDefinition::Java {
            build_target: resolver::interpolate_string(build_target, raw, env),
        },
        RawArtifactDefinition::Node { package_dir } => RawArtifactDefinition::Node {
            package_dir: resolver::interpolate_string(package_dir, raw, env),
        },
        RawArtifactDefinition::Python { package_dir } => RawArtifactDefinition::Python {
            package_dir: resolver::interpolate_string(package_dir, raw, env),
        },
        RawArtifactDefinition::Go { package } => RawArtifactDefinition::Go {
            package: resolver::interpolate_string(package, raw, env),
        },
    };
    artifact
}

fn interpolate_when(
    mut when: crate::raw::RawWhenConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> crate::raw::RawWhenConfig {
    when.target = when
        .target
        .map(|value| resolver::interpolate_string(value, raw, env));
    when.profile = when
        .profile
        .map(|value| resolver::interpolate_string(value, raw, env));
    when.branch = when
        .branch
        .map(|value| resolver::interpolate_string(value, raw, env));
    when.all = when
        .all
        .into_iter()
        .map(|item| interpolate_when(item, raw, env))
        .collect();
    when.any = when
        .any
        .into_iter()
        .map(|item| interpolate_when(item, raw, env))
        .collect();
    when.not = when
        .not
        .map(|item| Box::new(interpolate_when(*item, raw, env)));
    when
}

fn interpolate_image_definition(
    definition: RawImageDefinition,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawImageDefinition {
    match definition {
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
        } => RawImageDefinition::Buildroot {
            source: source.map(|value| resolver::interpolate_string(value, raw, env)),
            defconfig: defconfig.map(|value| resolver::interpolate_string(value, raw, env)),
            defconfig_path: defconfig_path
                .map(|value| resolver::interpolate_string(value, raw, env)),
            allow_fallback,
            config_fragments: config_fragments
                .into_iter()
                .map(|value| resolver::interpolate_string(value, raw, env))
                .collect(),
            config_overrides: config_overrides
                .into_iter()
                .map(|(key, value)| {
                    (
                        resolver::interpolate_string(key, raw, env),
                        resolver::interpolate_string(value, raw, env),
                    )
                })
                .collect(),
            external_tree: external_tree.map(|value| resolver::interpolate_string(value, raw, env)),
            external_tree_mode,
            expected_images: expected_images
                .into_iter()
                .map(|image| crate::raw::RawBuildrootExpectedImageConfig {
                    name: resolver::interpolate_string(image.name, raw, env),
                    format: image.format,
                    required: image.required,
                })
                .collect(),
        },
        RawImageDefinition::StartingPoint {
            source,
            source_path,
            rootfs_path,
            image_partition,
            image_read_only,
            packages,
            rootfs_validation_mode,
            output_mode,
        } => RawImageDefinition::StartingPoint {
            source: source.map(|value| resolver::interpolate_string(value, raw, env)),
            source_path: source_path.map(|value| resolver::interpolate_string(value, raw, env)),
            rootfs_path: resolver::interpolate_string(rootfs_path, raw, env),
            image_partition: image_partition
                .map(|value| resolver::interpolate_string(value, raw, env)),
            image_read_only,
            packages: crate::raw::RawStartingPointPackagesConfig {
                enabled: packages.enabled,
                execute: packages.execute,
                manager: packages
                    .manager
                    .map(|value| resolver::interpolate_string(value, raw, env)),
                release_version: packages
                    .release_version
                    .map(|value| resolver::interpolate_string(value, raw, env)),
                allow_major_upgrade: packages.allow_major_upgrade,
                update: packages.update,
                dist_upgrade: packages.dist_upgrade,
                install: packages
                    .install
                    .into_iter()
                    .map(|value| resolver::interpolate_string(value, raw, env))
                    .collect(),
                remove: packages
                    .remove
                    .into_iter()
                    .map(|value| resolver::interpolate_string(value, raw, env))
                    .collect(),
                extra_args: packages
                    .extra_args
                    .into_iter()
                    .map(|value| resolver::interpolate_string(value, raw, env))
                    .collect(),
                os_release_path: packages
                    .os_release_path
                    .map(|value| resolver::interpolate_string(value, raw, env)),
            },
            rootfs_validation_mode,
            output_mode,
        },
    }
}

fn interpolate_checkpoint(
    mut checkpoint: RawCheckpointConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawCheckpointConfig {
    checkpoint.id = resolver::interpolate_string(checkpoint.id, raw, env);
    checkpoint.backend = checkpoint
        .backend
        .map(|value| resolver::interpolate_string(value, raw, env));
    checkpoint.anchor = checkpoint
        .anchor
        .map(|value| resolver::interpolate_string(value, raw, env));
    checkpoint
}
