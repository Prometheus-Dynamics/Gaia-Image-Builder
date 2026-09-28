# Changelog

All notable changes to this workspace should be documented in this file.

The format is based on Keep a Changelog and this project follows Semantic Versioning.

## [Unreleased]

### Added

- Added `--only <targets>` to `gaia run` and `gaia plan` to execute part of the build graph (a domain such as `artifacts` or `image`, or an operation id such as `artifact:<id>`) plus its dependencies. Partial runs keep the reuse state of operations they skip.
- Added `gaia tui --builds-dir <dir>` to choose the directory the build picker scans.
- Added a `stage_file_src_placeholder` validation warning for `[[stage.files]]` sources that are directories holding nothing but placeholders such as README or `.gitkeep`.
- Reworked the TUI:
  - The Plan panel lists every operation with its execute/reuse decision and reason.
  - The monitor follows the newest running operation (`f` toggles following) and jumps to the failed operation's logs when a run fails.
  - A `?` key-help overlay, `j`/`k` navigation, and `m` to return to a running build from setup.

### Performance

- Added early cutoff: each operation records the content signature of the inputs it consumed, and an operation scheduled only because a dependency rebuilt is reused when that dependency produced identical content, so a no-op rebuild no longer cascades into image and assembly rebuilds.
- Reuse state is now saved after failed, cancelled and partial runs, keeping every operation that finished. It records plan-time fingerprints instead of re-probing tools after the run, and is written atomically.
- Buildroot records its config state before the long `make`, so a failed build after a config change resumes on retry instead of cleaning the output tree again.
- The Buildroot config comparison ignores the generated version header and settings that cannot change output (download/ccache locations, `BR2_JLEVEL`, mirrors), avoiding spurious full rebuilds. Existing state files remain valid.
- Buildroot images no longer run a separate prepare step (a second full `make`) unless an artifact sets `after_image_prepare`, and disk-assembly changes no longer re-run the Buildroot build.
- Buildroot downloads default to a workspace-wide cache (`.gaia/cache/buildroot/dl`, passed through the environment), which survives re-fetching the Buildroot source and is shared across builds. Cache directories outside the workspace are mounted into Docker builds.
- Docker artifact builds keep `CARGO_HOME` (registry and git dependencies) and `SCCACHE_DIR` under `.gaia/docker-cache` instead of losing them with each `--rm` container.
- Raw image and `tar.xz` compression use all cores (`xz -T0` with a fixed block size, so output does not depend on the thread count).
- Path-source and git tree digests hash files in-process instead of spawning `sha256sum` per file, and path sources ignore `target`, `node_modules`, `.git`, `.gaia` and `__pycache__` by default. Node and Python scratch output moved under `.gaia/` so builds no longer look like source changes.
- Re-cloned git sources keep their `.gaia` build state (such as the cargo target dir), and the remote git mirror is shared per repository and refreshed before cloning.
- Tool version probes used in fingerprints run once per process with a 10 second timeout, instead of per artifact with a 2 second timeout whose expiry forced rebuilds on loaded machines.

### Fixed

- Added declarative assembly support for creating directories and symlinks before filesystem packing, and expanded assembly glob matching to support versioned parent directories such as Buildroot firmware output paths.
- Fixed Buildroot image execution so provider-level expected-image reuse no longer bypasses scheduled Buildroot runs, ensuring config fragments, config overrides, `olddefconfig`, and package rebuild decisions are applied when the planner marks image operations dirty.
- Fixed Buildroot package overrides so package directories that intentionally replace core Buildroot packages are copied into the materialized Buildroot source package tree instead of being staged through `BR2_EXTERNAL`, which cannot redefine existing package names. Gaia now also cleans the Buildroot output when those replacement inputs change so stale target files from the previous package definition do not survive into the image.
- Fixed Buildroot config-change handling so an existing output tree is cleaned when the effective `.config` changes, preventing stale package install outputs from surviving after Kconfig options start requiring new files.
- Fixed stage file feeds so `[[stage.files]] mode` is preserved in resolved specs and applied by image providers when copying files into root filesystems.
- Fixed Buildroot feed refresh for assembly-generated raw disk outputs so Buildroot no longer tries to run stale provider post-image hooks for images now produced by typed image assembly.
- Fixed raw `.img.xz` publishing for typed image assembly so Gaia compresses the assembled disk image after disk creation instead of allowing the Buildroot provider to publish an expected root filesystem image under the final disk archive name.
- Fixed Buildroot output hygiene so large internal Buildroot work trees live under the build directory instead of the image collect directory, artifact state sidecars live under hidden `.gaia` folders, and reports prefer the final compressed assembly archive as the primary image output.
- Fixed typed assembly `vfat` filesystem generation so Gaia no longer forces FAT32 for small boot images, allowing mtools to choose a valid FAT variant for the requested image size.
- Fixed nested workspace path interpolation so references such as `${workspace.build_dir}/assembly` fully expand embedded build tokens instead of creating literal `${build.name}` directories.
- Fixed argument parsing so flags are never taken as the build path (`gaia tui --builds-dir configs/builds` previously tried to load a build named `--builds-dir`), and unknown flags, missing flag values, malformed `KEY=VALUE` pairs, and extra positional arguments are reported instead of silently ignored.
- Fixed missing `imports` and `extends` entries failing with a bare `failed to canonicalize` error; the error now names the config file that contains the reference.
- Fixed TUI quit during a running build abandoning the build without cancelling it; quitting now asks first, then cancels and waits for the build to stop.
- Fixed TUI typing `q` in a text field quitting the application.
- Fixed the TUI showing the previous run's operation statuses and progress while a new run was starting.
- Fixed TUI slowdown on long builds: operation status and logs are now indexed as events arrive instead of rescanning the whole event stream on every frame, and logs keep the most recent 5,000 lines per operation.
- Fixed the TUI Plan panel claiming a "serial runtime" executor and always showing the first operation in setup.
- Fixed `cargo clippy -- -D warnings` failures in the Buildroot provider and app crate.
- Added typed assembly MBR layout controls for `first_lba` and `alignment_lba`, allowing board images to preserve firmware-sensitive partition layouts instead of always using 1 MiB partition alignment.

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
