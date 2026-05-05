# Changelog

All notable changes to this workspace should be documented in this file.

The format is based on Keep a Changelog and this project follows Semantic Versioning.

## [Unreleased]

### Fixed

- Fixed Buildroot image execution so provider-level expected-image reuse no longer bypasses scheduled Buildroot runs, ensuring config fragments, config overrides, `olddefconfig`, and package rebuild decisions are applied when the planner marks image operations dirty.
- Fixed Buildroot package overrides so package directories that intentionally replace core Buildroot packages are copied into the materialized Buildroot source package tree instead of being staged through `BR2_EXTERNAL`, which cannot redefine existing package names. Gaia now also cleans the Buildroot output when those replacement inputs change so stale target files from the previous package definition do not survive into the image.
- Fixed Buildroot config-change handling so an existing output tree is cleaned when the effective `.config` changes, preventing stale package install outputs from surviving after Kconfig options start requiring new files.
- Fixed Buildroot feed refresh for assembly-generated raw disk outputs so Buildroot no longer tries to run stale provider post-image hooks for images now produced by typed image assembly.

## [2.0.0] - 2026-05-01

### Breaking Changes

- Rebuilt Gaia around the typed configuration model and removed the legacy `buildroot` / `program` / `stage` bucket compatibility layer.
- Split legacy program definitions into explicit `[[sources]]`, `[[artifacts]]`, and `[[install]]` domains.
- Replaced legacy checkpoint anchors with typed anchors such as `image`, `install:<id>`, `stage-file:<id>`, `stage-env:<id>`, and `stage-service:<id>`.
- Standardized generated image names and example output paths on the `2.0.0` release version.

### Added

- Added the current multi-crate workspace structure for core Gaia domains: config, spec, validation, planning, execution, process helpers, reporting, CLI/app orchestration, and the `gaia` binary.
- Added provider crates for source acquisition, artifact builds, image generation, and default provider registration.
- Added source providers for local paths, archives, downloads, and git-backed sources with identity and state tracking.
- Added artifact providers for Rust, Go, Java, Node, Python, and provider-level artifact output contracts.
- Added image providers for Buildroot images and starting-point image/rootfs mutation workflows.
- Added first-class Buildroot expected image formats for `cpio`, `ext2`, `ext3`, `ubifs`, `ubi`, `jffs2`, `romfs`, `cramfs`, `cloop`, `f2fs`, `btrfs`, and `erofs`.
- Added image assembly support for staged trees, file transforms, generated filesystems, MBR disks, BusyBox initramfs generation, typed assembly path templates, and reusable assembly fingerprints.
- Added dynamic input choices from git refs, GitHub releases, JSON sources, and commands with bounded subprocess execution, cache/lock files, fallback choices, and template-driven selected values.
- Added typed reporting and state outputs for summaries, manifests, provenance, backend state, reuse decisions, and runtime state.
- Added examples for Buildroot squashfs, SD card, Raspberry Pi 4, aarch64, minimal Rust, imported rootfs, imported raw image mutation, polyglot git projects, and template-based starting points.
- Added Docker build environments and smoke-test scripts for CI, Buildroot, polyglot artifacts, raw starting-point images, and aarch64 artifact builds.

### Changed

- Bumped all Gaia workspace crates and internal path dependency requirements from `0.2.0` to `2.0.0`.
- Bumped example build definitions, seed applications, package manifests, documentation paths, verification scripts, and generated artifact references to `2.0.0`.
- Refreshed `Cargo.lock` so workspace package entries resolve to `2.0.0`.
- Reworked the README and documentation index around the current typed domain model and command set.
- Expanded CLI documentation for `resolve`, `validate`, `plan`, `run`, `clean`, feature-gated `tui`, presets, environment files, environment overrides, and `--set` overrides.
- Updated migration guidance for translating older Gaia trees into typed source, artifact, install, stage, image, checkpoint, reporting, and policy declarations.
- Hardened shared process execution with timeout/cancellation support, bounded stream retention, direct stdout-to-file execution, process-tree cleanup, and bounded stream-reader queues.
- Reworked Buildroot package overrides to stage generated package trees through `BR2_EXTERNAL` instead of mutating the Buildroot source checkout.
- Added Buildroot cache policy support for download and compiler cache directories, including generated `.config` updates with escaped Kconfig string values.
- Strengthened dynamic input cache identity with versioned deterministic keys and removed process-global environment mutation from dynamic input tests.

### Validation

- Updated test fixtures and expected image/archive names that depended on pre-2.0.0 sample versions.
- Kept the release aligned with the existing Rust 2024 workspace settings, shared lint policy, and `rust-version = "1.94"` requirement.
- Added regression coverage for image assembly cleanup, Buildroot expected image collection, dynamic input cache separation, bounded process output, and Buildroot external tree validation.

## [0.1.3] - 2026-04-20

- Shifted the repository onto the shared workspace-skeleton baseline.
- Added standardized repo docs, scripts, testing notes, toolchain pinning, and CI entrypoints.
- Kept Gaia-specific architecture, module, and guide documentation intact under `docs/`.
