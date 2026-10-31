# Build file key reference

Every table a build file can have and the keys it takes. This page is
generated from the config layout Gaia validates against (`gaia config keys`
prints it); do not edit it by hand. Each table links to the page that
describes it. A key not listed here is reported as unknown when the build
is validated.

## `top level`

Described in [Configuration](../configuration.md#top-level-build-fields).

Keys: `branch`, `build_name`, `description`, `display_name`, `env`, `env_files`, `extends`, `gaia_version`, `imports`, `inputs`, `labels`, `preset`, `presets`, `profile`, `target`, `variants`, `version`.

## `[product]`

Described in [Configuration](../configuration.md#product-metadata).

Keys: `family`, `name`, `sku`.

## `[workspace]`

Described in [Policy, workspace and clean](../configuration-policy.md#workspace).

Keys: `build_dir`, `export_dir`, `named_paths`, `out_dir`, `root_dir`.

## `[[sources]]` with `kind = "git"`

Described in [Sources and toolchains](../configuration-sources.md#sources).

Keys: `branch`, `git_backend`, `id`, `pin`, `refresh`, `repo`, `rev`, `subdir`, `tag`, `update`.

## `[[sources]]` with `kind = "path"`

Described in [Sources and toolchains](../configuration-sources.md#sources).

Keys: `id`, `identity_ignore`, `path`, `pin`, `refresh`.

## `[[sources]]` with `kind = "archive"`

Described in [Sources and toolchains](../configuration-sources.md#sources).

Keys: `allow_absolute_symlinks`, `id`, `path`, `pin`, `refresh`, `sha256`, `strip_components`, `url`.

## `[[sources]]` with `kind = "download"`

Described in [Sources and toolchains](../configuration-sources.md#sources).

Keys: `id`, `output_path`, `pin`, `refresh`, `sha256`, `url`.

## `[[toolchains]]`

Described in [Sources and toolchains](../configuration-sources.md#toolchains).

Keys: `id`, `kind`, `page_size`, `path`, `prefix`, `relocate`, `rust_target`, `source`, `sysroot`, `sysroot_source`, `triple`.

## `[[artifacts]]` with `kind = "rust"`

Described in [Artifacts, install and stage](../configuration-artifacts.md#artifacts).

Keys: `after_image_prepare`, `all_features`, `build_env`, `build_group`, `cargo_git_sources`, `dependencies`, `emit_directory`, `features`, `id`, `install_class`, `install_dest_hint`, `install_name`, `no_default_features`, `output_path`, `package`, `profile`, `source`, `sources`, `target`, `target_name`, `toolchain`, `when`.

## `[[artifacts]]` with `kind = "java"`

Described in [Artifacts, install and stage](../configuration-artifacts.md#artifacts).

Keys: `after_image_prepare`, `build_args`, `build_command`, `build_env`, `build_group`, `build_target`, `cargo_git_sources`, `dependencies`, `id`, `install_class`, `install_dest_hint`, `install_name`, `output_path`, `profile`, `source`, `sources`, `target`, `toolchain`, `when`.

## `[[artifacts]]` with `kind = "node"`

Described in [Artifacts, install and stage](../configuration-artifacts.md#artifacts).

Keys: `after_image_prepare`, `build_group`, `cargo_git_sources`, `dependencies`, `id`, `install_class`, `install_dest_hint`, `install_name`, `output_path`, `package_dir`, `profile`, `source`, `sources`, `target`, `toolchain`, `when`.

## `[[artifacts]]` with `kind = "python"`

Described in [Artifacts, install and stage](../configuration-artifacts.md#artifacts).

Keys: `after_image_prepare`, `build_group`, `cargo_git_sources`, `dependencies`, `id`, `install_class`, `install_dest_hint`, `install_name`, `output_path`, `package_dir`, `profile`, `source`, `sources`, `target`, `toolchain`, `when`.

## `[[artifacts]]` with `kind = "go"`

Described in [Artifacts, install and stage](../configuration-artifacts.md#artifacts).

Keys: `after_image_prepare`, `build_group`, `cargo_git_sources`, `dependencies`, `id`, `install_class`, `install_dest_hint`, `install_name`, `output_path`, `package`, `profile`, `source`, `sources`, `target`, `toolchain`, `when`.

## `[artifacts.execution]`

Described in [Artifacts, install and stage](../configuration-artifacts.md#artifacts).

Keys: `backend`.

## `[artifacts.execution.docker]`

Described in [Artifacts, install and stage](../configuration-artifacts.md#artifacts).

Keys: `context`, `dockerfile`, `image`.

## `[[install]]`

Described in [Artifacts, install and stage](../configuration-artifacts.md#install).

Keys: `artifact`, `dest`, `group`, `id`, `mode`, `owner`, `replace`, `when`.

## `[stage]`

Described in [Artifacts, install and stage](../configuration-artifacts.md#stage).

Keys: none of its own.

## `[[stage.files]]`

Described in [Artifacts, install and stage](../configuration-artifacts.md#stage).

Keys: `content`, `dest`, `id`, `mode`, `origin`, `src`, `when`.

## `[[stage.env_sets]]`

Described in [Artifacts, install and stage](../configuration-artifacts.md#stage).

Keys: `entries`, `id`, `name`, `when`.

## `[[stage.services]]`

Described in [Artifacts, install and stage](../configuration-artifacts.md#stage).

Keys: `id`, `name`, `unit_path`, `when`.

## `[image]` with `kind = "buildroot"`

Described in [Image](../configuration-image.md#image).

Keys: `allow_fallback`, `config_fragments`, `config_overrides`, `defconfig`, `defconfig_path`, `external_tree`, `external_tree_mode`, `legal_info`, `source`.

## `[image]` with `kind = "starting-point"`

Described in [Image](../configuration-image.md#image).

Keys: `image_partition`, `image_read_only`, `output_mode`, `rootfs_path`, `rootfs_validation_mode`, `source`, `source_path`.

## `[image]` with `kind = "none"`

Described in [Image](../configuration-image.md#image).

Keys: none of its own.

## `[image.output]`

Described in [Image](../configuration-image.md#image).

Keys: `archive_name`, `collect_dir`, `disk`, `emit_report`, `export_dir`.

## `[image.signing]`

Described in [Image](../configuration-image.md#signing).

Keys: `key`, `kind`, `outputs`.

## `[image.layers]`

Described in [Image](../configuration-image.md#image-layers).

Keys: `default`, `erofs`, `erofs_options`, `order`, `overrides`, `verity`.

Each `[image.layers.<name>]` takes: `erofs_options`, `installs`, `packages`, `paths`, `services`, `stage`.

## `[image.checks]`

Described in [Image](../configuration-image.md#image-checks).

Keys: `kernel_image`, `kernel_modules`, `modules_dir`, `pi_model`, `symvers`.

## `[[image.hardware]]`

Described in [Image](../configuration-image.md#image-checks).

Keys: `compatible`, `files`, `firmware`, `id`, `kind`, `modules`, `overlays`.

## `[image.feed]`

Described in [Image](../configuration-image.md#image-feed).

Keys: `install_entries`, `stage_env_sets`, `stage_files`, `stage_services`.

## `[image.assembly]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `out_dir`, `strip_tool`, `work_dir`.

## `[[image.assembly.trees]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `id`, `path`, `strip`.

## `[[image.assembly.dirs]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `mode`, `path`, `tree`.

## `[[image.assembly.symlinks]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `path`, `target`, `tree`.

## `[[image.assembly.files]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `allow_missing_libs`, `content`, `dest`, `dir_mode`, `exclude`, `include`, `json`, `mode`, `optional`, `preserve_symlink`, `src`, `src_dir`, `src_glob`, `strip`, `tree`.

## `[[image.assembly.transforms]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `data_block_size`, `dest`, `deterministic`, `env_size`, `hash_block_size`, `hash_dest`, `kind`, `level`, `long`, `redundant`, `salt`, `src`, `uuid`.

## `[[image.assembly.filesystems]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `block_size`, `compression`, `compression_level`, `dedupe`, `deterministic`, `fat_size`, `fragments`, `free`, `id`, `journal`, `kind`, `label`, `output`, `pcluster_size`, `publish`, `reserved_percent`, `size`, `source_tree`, `uuid`, `ztailpacking`.

## `[[image.assembly.disks]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `alignment_lba`, `disk_guid`, `ebr_placement`, `first_lba`, `id`, `output`, `partition_table`, `signature`, `signature_text`, `truncate`.

## `[[image.assembly.disks.partitions]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `bootable`, `guid`, `image`, `materialize`, `name`, `offset`, `size`, `table`, `type`, `type_alias`, `wipe`.

## `[[image.assembly.archives]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `id`, `output`.

## `[[image.assembly.archives.members]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `entries`, `exclude`, `json`, `name`, `prefix`, `src`, `src_dir`, `src_glob`, `tree`.

## `[[image.assembly.archives.generated]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `entries`, `json`, `name`.

## `[[image.assembly.oci]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `compression`, `dest`, `format`, `id`, `ref`.

## `[image.assembly.oci.layers]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `tar`, `tree`.

## `[image.assembly.oci.config]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `architecture`, `cmd`, `entrypoint`, `env`, `labels`, `os`, `stop_signal`, `working_dir`.

## `[[image.assembly.packages]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `allowed_prefixes`, `debian_binary`, `dest`, `format`, `id`, `tree`.

## `[image.assembly.packages.control]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `Architecture`, `Depends`, `Description`, `Homepage`, `Maintainer`, `Package`, `Priority`, `Section`, `Source`, `Version`, `extra`.

## `[image.assembly.packages.scripts]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `conffiles`, `postinst`, `postrm`, `preinst`, `prerm`, `services`.

## `[image.assembly.packages.device]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `check_needed`, `max_glibc`, `min_page_size`, `sysroot`, `toolchain`.

## `[[image.assembly.checks]]`

Described in [Image assembly](../configuration-assembly.md#assembly-checks).

Keys: `command`, `id`, `timeout_secs`.

## `[[image.assembly.busybox_initramfs]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `applets`, `busybox`, `include_runtime_libs`, `sysroot`, `tree`.

## `[[image.assembly.binaries]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `dest`, `mode`, `src`, `strip`, `sysroot`, `tree`.

## `[[image.assembly.kernel_modules]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `depmod`, `from`, `kernel_version`, `metadata`, `modules`, `tree`.

## `[[image.assembly.devices]]`

Described in [Image assembly](../configuration-assembly.md#image-assembly).

Keys: `kind`, `major`, `minor`, `mode`, `path`, `tree`.

## `[image.rootfs]`

Described in [Image](../configuration-image.md#image).

Keys: `exclude_packages`.

## `[[image.expected_images]]`

Described in [Image](../configuration-image.md#image).

Keys: `format`, `name`, `required`.

## `[image.packages]`

Described in [Image](../configuration-image.md#image).

Keys: `allow_major_upgrade`, `dist_upgrade`, `enabled`, `execute`, `extra_args`, `install`, `manager`, `os_release_path`, `release_version`, `remove`, `update`.

## `[[checkpoints]]`

Described in [Checkpoints, registry and reporting](../configuration-checkpoints.md#checkpoints).

Keys: `anchor`, `backend`, `id`, `registry`, `repository`, `sign_key`, `sign_kind`, `token`, `trust`, `upload_policy`, `use_policy`, `username`, `verify_key`.

## `[interpolation]`

Described in [Configuration](../configuration.md#interpolation).

Keys: `allow_unresolved`, `values`.

## `[clean]`

Described in [Policy, workspace and clean](../configuration-policy.md#clean).

Keys: `default`, `profiles`.

## `[execution]`

Described in [Policy, workspace and clean](../configuration-policy.md#execution-policy).

Keys: `canonical_tree`, `jobs`, `offline`.

## `[execution.docker]`

Described in [Policy, workspace and clean](../configuration-policy.md#execution-policy).

Keys: `enabled`, `image`.

## `[execution.output_retention]`

Described in [Policy, workspace and clean](../configuration-policy.md#execution-policy).

Keys: `failure_tail_lines`, `stderr_bytes`, `stderr_lines`, `stdout_bytes`, `stdout_lines`.

## `[failure]`

Described in [Policy, workspace and clean](../configuration-policy.md#failure-policy).

Keys: `keep_going`, `preserve_failed_outputs`, `rollback_completed`, `rollback_domains`, `rollback_on_error`.

## `[providers]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: none of its own.

## `[providers.rust]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `allow_nested_build`, `batch_builds`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `sccache`, `shared_target_dir`, `strip`, `timeout_seconds`.

## `[providers.git]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `allow_remote_resolution`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `timeout_seconds`.

## `[providers.docker]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `reuse_container`.

## `[providers.archive]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.archive.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.archive.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.archive.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[providers.download]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.download.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.download.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.download.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[providers.go]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.go.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.go.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.go.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[providers.java]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.java.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.java.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.java.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[providers.node]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.node.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.node.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.node.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[providers.python]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.python.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.python.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.python.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[providers.buildroot]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.buildroot.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.buildroot.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.buildroot.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[providers.starting_point]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `download_dir`, `erofs_workers`, `gradle_home`, `keep_ram_tree`, `kernel_modules_check`, `local_jobs`, `make_retry_attempts`, `make_timeout_seconds`, `override_check`, `parallel_packages`, `ram_budget`, `retry_attempts`, `retry_backoff_ms`, `retry_backoff_strategy`, `shared_output`, `shared_output_dir`, `timeout_seconds`, `work_dir`.

## `[providers.starting_point.ccache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `max_size`.

## `[providers.starting_point.package_cache]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `dir`, `enabled`, `level`, `max_size`, `project_dir`, `project_packages`, `system_dir`, `system_packages`.

## `[providers.starting_point.host_tools]`

Described in [Policy, workspace and clean](../configuration-policy.md#provider-execution-policy).

Keys: `ccache`, `default`, `pkgconf`.

## `[provenance]`

Described in [Policy, workspace and clean](../configuration-policy.md#provenance).

Keys: none of its own.

## `[provenance.identity]`

Described in [Policy, workspace and clean](../configuration-policy.md#provenance).

Keys: `channel`, `labels`, `project`, `vendor`.

## `[reporting]`

Described in [Checkpoints, registry and reporting](../configuration-checkpoints.md#reporting).

Keys: `manifest`, `provenance`, `summary`.

## `[reporting.masking]`

Described in [Checkpoints, registry and reporting](../configuration-checkpoints.md#reporting).

Keys: `enabled`, `patterns`, `replacement`.

## `[reporting.output_hygiene]`

Described in [Checkpoints, registry and reporting](../configuration-checkpoints.md#reporting).

Keys: `large_file_threshold_bytes`, `transient_dir_names`.

## `[reporting.post_build]`

Described in [Checkpoints, registry and reporting](../configuration-checkpoints.md#reporting).

Keys: `script`, `timeout_seconds`.

## `[expect]`

Described in [Configuration](../configuration.md#top-level-build-fields).

Keys: `artifacts`, `installs`, `sources`.

## `[secrets]`

Described in [Configuration](../configuration.md#secrets).

Keys: none of its own.

Each `[secrets.<name>]` takes: `command`, `env`, `file`, `required`.
