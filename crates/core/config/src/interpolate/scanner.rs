use crate::raw::{
    RawArtifactDefinition, RawBuildConfig, RawImageDefinition, RawSourceDefinition,
    RawUnresolvedInterpolation,
};

pub(crate) fn collect_unresolved_tokens(raw: &RawBuildConfig) -> Vec<RawUnresolvedInterpolation> {
    let mut unresolved = Vec::new();

    scan_string("build.name", &raw.build_name, &mut unresolved);
    if let Some(display_name) = &raw.display_name {
        scan_string("build.display_name", display_name, &mut unresolved);
    }
    scan_optional("build.version", raw.version.as_deref(), &mut unresolved);
    scan_optional(
        "build.description",
        raw.description.as_deref(),
        &mut unresolved,
    );
    scan_optional("build.branch", raw.branch.as_deref(), &mut unresolved);
    scan_optional("build.target", raw.target.as_deref(), &mut unresolved);
    scan_optional("build.profile", raw.profile.as_deref(), &mut unresolved);
    for (key, value) in &raw.labels {
        scan_string(&format!("build.labels.{key}.key"), key, &mut unresolved);
        scan_string(&format!("build.labels.{key}"), value, &mut unresolved);
    }

    scan_optional(
        "product.family",
        raw.product.family.as_deref(),
        &mut unresolved,
    );
    scan_optional("product.name", raw.product.name.as_deref(), &mut unresolved);
    scan_optional("product.sku", raw.product.sku.as_deref(), &mut unresolved);
    scan_optional("preset.name", raw.preset.as_deref(), &mut unresolved);
    for (key, value) in &raw.selected_inputs {
        scan_string(&format!("input.{key}"), value, &mut unresolved);
    }

    for (key, value) in &raw.interpolation.values {
        scan_string(
            &format!("interpolation.values.{key}.key"),
            key,
            &mut unresolved,
        );
        scan_string(
            &format!("interpolation.values.{key}"),
            value,
            &mut unresolved,
        );
    }

    scan_string(
        "workspace.root_dir",
        &raw.workspace.root_dir,
        &mut unresolved,
    );
    scan_string(
        "workspace.build_dir",
        &raw.workspace.build_dir,
        &mut unresolved,
    );
    scan_string("workspace.out_dir", &raw.workspace.out_dir, &mut unresolved);
    for entry in &raw.workspace.named_paths {
        scan_string(
            &format!("workspace.paths.{}.alias", entry.alias),
            &entry.alias,
            &mut unresolved,
        );
        scan_string(
            &format!("workspace.paths.{}", entry.alias),
            &entry.path,
            &mut unresolved,
        );
    }
    scan_optional(
        "clean.default",
        raw.clean.default.as_deref(),
        &mut unresolved,
    );
    for (name, profile) in &raw.clean.profiles {
        scan_string(
            &format!("clean.profiles.{name}.name"),
            name,
            &mut unresolved,
        );
        scan_optional(
            &format!("clean.profiles.{name}.description"),
            profile.description.as_deref(),
            &mut unresolved,
        );
        for (index, path) in profile.paths.iter().enumerate() {
            scan_string(
                &format!("clean.profiles.{name}.paths[{index}]"),
                path,
                &mut unresolved,
            );
        }
    }

    for (index, value) in raw.env_files.iter().enumerate() {
        scan_string(&format!("env_files[{index}]"), value, &mut unresolved);
    }
    for (key, value) in &raw.env {
        scan_string(&format!("env.{key}.key"), key, &mut unresolved);
        scan_string(&format!("env.{key}"), value, &mut unresolved);
    }

    for source in &raw.sources {
        scan_string(
            &format!("sources.{}.id", source.id),
            &source.id,
            &mut unresolved,
        );
        match &source.definition {
            RawSourceDefinition::Git {
                repo,
                branch,
                tag,
                rev,
                subdir,
                ..
            } => {
                scan_string(
                    &format!("sources.{}.git.repo", source.id),
                    repo,
                    &mut unresolved,
                );
                scan_optional(
                    &format!("sources.{}.git.branch", source.id),
                    branch.as_deref(),
                    &mut unresolved,
                );
                scan_optional(
                    &format!("sources.{}.git.tag", source.id),
                    tag.as_deref(),
                    &mut unresolved,
                );
                scan_optional(
                    &format!("sources.{}.git.rev", source.id),
                    rev.as_deref(),
                    &mut unresolved,
                );
                scan_optional(
                    &format!("sources.{}.git.subdir", source.id),
                    subdir.as_deref(),
                    &mut unresolved,
                );
            }
            RawSourceDefinition::Path { path, .. } => {
                scan_string(
                    &format!("sources.{}.path.path", source.id),
                    path,
                    &mut unresolved,
                );
            }
            RawSourceDefinition::Archive { path, .. } => {
                scan_string(
                    &format!("sources.{}.archive.path", source.id),
                    path,
                    &mut unresolved,
                );
            }
            RawSourceDefinition::Download {
                url,
                sha256,
                output_path,
                ..
            } => {
                scan_string(
                    &format!("sources.{}.download.url", source.id),
                    url,
                    &mut unresolved,
                );
                scan_optional(
                    &format!("sources.{}.download.sha256", source.id),
                    sha256.as_deref(),
                    &mut unresolved,
                );
                scan_string(
                    &format!("sources.{}.download.output_path", source.id),
                    output_path,
                    &mut unresolved,
                );
            }
        }
    }

    for artifact in &raw.artifacts {
        scan_string(
            &format!("artifacts.{}.id", artifact.id),
            &artifact.id,
            &mut unresolved,
        );
        scan_optional(
            &format!("artifacts.{}.source", artifact.id),
            artifact.source.as_deref(),
            &mut unresolved,
        );
        scan_optional(
            &format!("artifacts.{}.profile", artifact.id),
            artifact.profile.as_deref(),
            &mut unresolved,
        );
        scan_optional(
            &format!("artifacts.{}.install_name", artifact.id),
            artifact.install_name.as_deref(),
            &mut unresolved,
        );
        scan_optional(
            &format!("artifacts.{}.install_dest_hint", artifact.id),
            artifact.install_dest_hint.as_deref(),
            &mut unresolved,
        );
        scan_string(
            &format!("artifacts.{}.output_path", artifact.id),
            &artifact.output_path,
            &mut unresolved,
        );
        for (index, dependency) in artifact.dependencies.iter().enumerate() {
            scan_string(
                &format!("artifacts.{}.dependencies[{index}]", artifact.id),
                dependency,
                &mut unresolved,
            );
        }
        match &artifact.definition {
            RawArtifactDefinition::Rust {
                package,
                target_name,
                ..
            } => {
                scan_string(
                    &format!("artifacts.{}.rust.package", artifact.id),
                    package,
                    &mut unresolved,
                );
                scan_optional(
                    &format!("artifacts.{}.rust.target_name", artifact.id),
                    target_name.as_deref(),
                    &mut unresolved,
                );
            }
            RawArtifactDefinition::Java { build_target } => {
                scan_string(
                    &format!("artifacts.{}.java.build_target", artifact.id),
                    build_target,
                    &mut unresolved,
                );
            }
            RawArtifactDefinition::Node { package_dir } => {
                scan_string(
                    &format!("artifacts.{}.node.package_dir", artifact.id),
                    package_dir,
                    &mut unresolved,
                );
            }
            RawArtifactDefinition::Python { package_dir } => {
                scan_string(
                    &format!("artifacts.{}.python.package_dir", artifact.id),
                    package_dir,
                    &mut unresolved,
                );
            }
            RawArtifactDefinition::Go { package } => {
                scan_string(
                    &format!("artifacts.{}.go.package", artifact.id),
                    package,
                    &mut unresolved,
                );
            }
        }
    }

    for install in &raw.install {
        scan_string(
            &format!("install.{}.id", install.id),
            &install.id,
            &mut unresolved,
        );
        scan_string(
            &format!("install.{}.artifact", install.id),
            &install.artifact,
            &mut unresolved,
        );
        scan_string(
            &format!("install.{}.dest", install.id),
            &install.dest,
            &mut unresolved,
        );
        scan_optional(
            &format!("install.{}.owner", install.id),
            install.owner.as_deref(),
            &mut unresolved,
        );
        scan_optional(
            &format!("install.{}.group", install.id),
            install.group.as_deref(),
            &mut unresolved,
        );
    }

    for file in &raw.stage.files {
        scan_string(
            &format!("stage.files.{}.id", file.id),
            &file.id,
            &mut unresolved,
        );
        scan_string(
            &format!("stage.files.{}.src", file.id),
            &file.src,
            &mut unresolved,
        );
        scan_string(
            &format!("stage.files.{}.dest", file.id),
            &file.dest,
            &mut unresolved,
        );
    }
    for env_set in &raw.stage.env_sets {
        scan_string(
            &format!("stage.env_sets.{}.id", env_set.id),
            &env_set.id,
            &mut unresolved,
        );
        scan_string(
            &format!("stage.env_sets.{}.name", env_set.id),
            &env_set.name,
            &mut unresolved,
        );
        for (key, value) in &env_set.entries {
            scan_string(
                &format!("stage.env_sets.{}.entries.{key}.key", env_set.id),
                key,
                &mut unresolved,
            );
            scan_string(
                &format!("stage.env_sets.{}.entries.{key}", env_set.id),
                value,
                &mut unresolved,
            );
        }
    }
    for service in &raw.stage.services {
        scan_string(
            &format!("stage.services.{}.id", service.id),
            &service.id,
            &mut unresolved,
        );
        scan_string(
            &format!("stage.services.{}.name", service.id),
            &service.name,
            &mut unresolved,
        );
        scan_string(
            &format!("stage.services.{}.unit_path", service.id),
            &service.unit_path,
            &mut unresolved,
        );
    }

    match &raw.image.definition {
        RawImageDefinition::Buildroot {
            defconfig,
            defconfig_path,
            config_fragments,
            config_overrides,
            external_tree,
            expected_images,
            ..
        } => {
            scan_optional(
                "image.buildroot.defconfig",
                defconfig.as_deref(),
                &mut unresolved,
            );
            scan_optional(
                "image.buildroot.defconfig_path",
                defconfig_path.as_deref(),
                &mut unresolved,
            );
            for (index, fragment) in config_fragments.iter().enumerate() {
                scan_string(
                    &format!("image.buildroot.config_fragments.{index}"),
                    fragment,
                    &mut unresolved,
                );
            }
            for (index, (key, value)) in config_overrides.iter().enumerate() {
                scan_string(
                    &format!("image.buildroot.config_overrides.{index}.key"),
                    key,
                    &mut unresolved,
                );
                scan_string(
                    &format!("image.buildroot.config_overrides.{index}.value"),
                    value,
                    &mut unresolved,
                );
            }
            scan_optional(
                "image.buildroot.external_tree",
                external_tree.as_deref(),
                &mut unresolved,
            );
            for (index, expected_image) in expected_images.iter().enumerate() {
                scan_string(
                    &format!("image.buildroot.expected_images.{index}.name"),
                    &expected_image.name,
                    &mut unresolved,
                );
            }
        }
        RawImageDefinition::StartingPoint {
            source,
            source_path,
            rootfs_path,
            image_partition,
            packages,
            ..
        } => {
            scan_optional(
                "image.starting-point.source",
                source.as_deref(),
                &mut unresolved,
            );
            scan_optional(
                "image.starting-point.source_path",
                source_path.as_deref(),
                &mut unresolved,
            );
            scan_string(
                "image.starting-point.rootfs_path",
                rootfs_path,
                &mut unresolved,
            );
            scan_optional(
                "image.starting-point.image_partition",
                image_partition.as_deref(),
                &mut unresolved,
            );
            scan_optional(
                "image.starting-point.packages.manager",
                packages.manager.as_deref(),
                &mut unresolved,
            );
            scan_optional(
                "image.starting-point.packages.release_version",
                packages.release_version.as_deref(),
                &mut unresolved,
            );
            scan_optional(
                "image.starting-point.packages.os_release_path",
                packages.os_release_path.as_deref(),
                &mut unresolved,
            );
            for (index, value) in packages.install.iter().enumerate() {
                scan_string(
                    &format!("image.starting-point.packages.install.{index}"),
                    value,
                    &mut unresolved,
                );
            }
            for (index, value) in packages.remove.iter().enumerate() {
                scan_string(
                    &format!("image.starting-point.packages.remove.{index}"),
                    value,
                    &mut unresolved,
                );
            }
            for (index, value) in packages.extra_args.iter().enumerate() {
                scan_string(
                    &format!("image.starting-point.packages.extra_args.{index}"),
                    value,
                    &mut unresolved,
                );
            }
        }
    }
    for (index, install_entry) in raw.image.feed.install_entries.iter().enumerate() {
        scan_string(
            &format!("image.feed.install_entries.{index}"),
            install_entry,
            &mut unresolved,
        );
    }
    for (index, stage_file) in raw.image.feed.stage_files.iter().enumerate() {
        scan_string(
            &format!("image.feed.stage_files.{index}"),
            stage_file,
            &mut unresolved,
        );
    }
    for (index, stage_env_set) in raw.image.feed.stage_env_sets.iter().enumerate() {
        scan_string(
            &format!("image.feed.stage_env_sets.{index}"),
            stage_env_set,
            &mut unresolved,
        );
    }
    for (index, stage_service) in raw.image.feed.stage_services.iter().enumerate() {
        scan_string(
            &format!("image.feed.stage_services.{index}"),
            stage_service,
            &mut unresolved,
        );
    }
    scan_optional(
        "image.output.collect_dir",
        raw.image.output.collect_dir.as_deref(),
        &mut unresolved,
    );
    scan_optional(
        "image.output.archive_name",
        raw.image.output.archive_name.as_deref(),
        &mut unresolved,
    );

    for checkpoint in &raw.checkpoints {
        scan_string(
            &format!("checkpoints.{}.id", checkpoint.id),
            &checkpoint.id,
            &mut unresolved,
        );
        scan_optional(
            &format!("checkpoints.{}.backend", checkpoint.id),
            checkpoint.backend.as_deref(),
            &mut unresolved,
        );
        scan_optional(
            &format!("checkpoints.{}.anchor", checkpoint.id),
            checkpoint.anchor.as_deref(),
            &mut unresolved,
        );
    }

    scan_optional(
        "provenance.identity.project",
        raw.provenance.identity.project.as_deref(),
        &mut unresolved,
    );
    scan_optional(
        "provenance.identity.vendor",
        raw.provenance.identity.vendor.as_deref(),
        &mut unresolved,
    );
    scan_optional(
        "provenance.identity.channel",
        raw.provenance.identity.channel.as_deref(),
        &mut unresolved,
    );
    for (key, value) in &raw.provenance.identity.labels {
        scan_string(
            &format!("provenance.identity.labels.{key}.key"),
            key,
            &mut unresolved,
        );
        scan_string(
            &format!("provenance.identity.labels.{key}"),
            value,
            &mut unresolved,
        );
    }

    scan_string(
        "reporting.masking.replacement",
        &raw.reporting.masking.replacement,
        &mut unresolved,
    );
    for (index, value) in raw.reporting.masking.patterns.iter().enumerate() {
        scan_string(
            &format!("reporting.masking.patterns[{index}]"),
            value,
            &mut unresolved,
        );
    }
    if let Some(hook) = &raw.reporting.post_build {
        scan_string("reporting.post_build.script", &hook.script, &mut unresolved);
    }

    unresolved.sort_by(|a, b| {
        a.location
            .cmp(&b.location)
            .then_with(|| a.token.cmp(&b.token))
    });
    unresolved.dedup_by(|a, b| a.location == b.location && a.token == b.token);
    unresolved
}

fn scan_optional(
    location: &str,
    value: Option<&str>,
    unresolved: &mut Vec<RawUnresolvedInterpolation>,
) {
    if let Some(value) = value {
        scan_string(location, value, unresolved);
    }
}

fn scan_string(location: &str, value: &str, unresolved: &mut Vec<RawUnresolvedInterpolation>) {
    let mut rest = value;

    while let Some(start) = rest.find("${") {
        let remainder = &rest[start + 2..];
        let Some(end) = remainder.find('}') else {
            unresolved.push(RawUnresolvedInterpolation {
                location: location.to_string(),
                token: rest[start..].to_string(),
            });
            return;
        };
        let token = &remainder[..end];
        unresolved.push(RawUnresolvedInterpolation {
            location: location.to_string(),
            token: token.to_string(),
        });
        rest = &remainder[end + 1..];
    }
}
