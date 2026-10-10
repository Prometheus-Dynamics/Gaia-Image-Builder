# Changelog

All notable changes to this workspace should be documented in this file.

The format is based on Keep a Changelog and this project follows Semantic Versioning.

## [Unreleased]

Changes on `main` since 2.0.0. The workspace version stays `2.0.0` until the
next release; builds from `main` (some of which briefly reported 2.1.0 to
2.5.0) are identified by commit. Build files should not require unreleased
versions through `gaia_version`: a binary that lacks a setting now fails
validation on the unknown key.

This cycle also moves the toolchain pin and `rust-version` to Rust 1.99.0
(installing Gaia needs Rust 1.99 or newer), upgrades every dependency to its
newest release, and updates the Rust docker images to 1.99.0.

### BusyBox runtime libraries come from the target sysroot

- `[[image.assembly.busybox_initramfs]] include_runtime_libs = true` no longer runs the host's `ldd`. A cross-compiled BusyBox (for example an aarch64 glibc binary from a Buildroot target tree on an x86_64 host) was reported "not a dynamic executable" by the host `ldd`, so Gaia shipped it as static and the initramfs panicked at `/init`. A host-architecture binary got the host's libraries instead.
- Gaia now reads the binary's ELF headers (program interpreter, `DT_NEEDED`, `DT_RUNPATH`/`DT_RPATH`) and resolves each library under a new `sysroot` path template (default: the directory above the binary's `bin/` or `sbin/`, or two levels up from `usr/bin/` or `usr/sbin/`). Symlinks inside the sysroot are followed and recreated in the tree, the interpreter goes at its exact path, a missing library fails the build naming every missing name, and a static binary copies nothing. Libraries of another class, byte order or machine are skipped.
- The reuse fingerprint covers the sysroot, the interpreter, the symlinks and the digest of every library file, so a changed libc rebuilds the assembly. The `ldd --version` probe is gone.

### vfat images sized automatically

- `[[image.assembly.filesystems]] kind = "vfat"` accepts `size = "auto"`: the image is the content (each file and directory rounded up to 4 KiB) plus 1 MiB for FAT overhead plus a margin of 5% (at least 1 MiB), rounded up to a MiB. The size depends only on the tree's content. `size = "auto"` is rejected for other kinds. Omitting `size` still gives 32M.

### Image assembly: cpio-zstd, kernel modules, published filesystem images

- `[[image.assembly.filesystems]]` takes `kind = "cpio-zstd"`: a newc cpio of the tree, compressed with zstd. The optional `compression_level` (1-19, default 19) is accepted only there. Output is deterministic, as for the other cpio kinds.
- `[[image.assembly.kernel_modules]]` copies named kernel modules, with their `modules.dep` dependency closure, from a kernel's module directory into a tree, copies `modules.order`, `modules.builtin` and `modules.builtin.modinfo`, and runs `depmod` over the tree. Built-in modules are skipped with a message; an unknown module fails the build. The step runs before the filesystems that pack the tree.
- `publish = true` on a filesystem copies its image to the image output dir under the output's file name, and reports it like a raw disk (run summary, image sizes, content digests). Validation rejects a published name that collides with another output. Works in disk and RAM work dirs.
- The flasher boot image example is in `docs/configuration.md` (Image Assembly).

### Buildroot source mirror kept while the source is unchanged

- A RAM build's Buildroot source mirror is no longer copied again on every operation when the source is unchanged. `rsync` had to stat every file of the source on a busy disk (36 s in one run). The mirror is kept when its marker (`.gaia-mirror-source`, the source's recorded tree digest and its top-level entries' metadata) matches the source; the run reports "buildroot source mirror: unchanged". A missing or incomplete mirror, a changed source, or a source with no recorded tree digest is copied in full, as before.

### External files of a kernel extension rebuild linux

- A changed file under an external tree's `linux/` directory (Buildroot's kernel-extension directory, such as `linux/patches/*.patch` applied by a linux post-patch hook) now rebuilds `linux`, instead of reassembling the target as a file that "maps to no package". A `<name>/patches/<file>` path names package `<name>`.
- A file under a `BR2_GLOBAL_PATCH_DIR` entry names the package of the subdirectory it is in, when that package is known.

### Run explains its rebuilds by input

- `gaia run` now names the changed inputs of a rebuild, as `gaia preview` does: the run log and `<build>.rebuild-reasons.json` say, for example, "config_overrides changed (BR2_A)" instead of "fingerprint changed (no detail recorded)". Both commands share one explanation, read from the recorded inputs before the run overwrites them.
- A Buildroot run whose clean plan cleans anything adds one run summary note (in the run output and `image_notes`): "buildroot clean: rebuild linux, linux-headers (...)", "buildroot clean: full (...)", or "buildroot clean: finalize only (...)", with the reasons.

### Host tools probe reused when its inputs are unchanged

- The "host tools probe" step asked the build environment for each system
  host tool (`command -v`, then `--version`). With a docker execution backend
  that is one `docker run` per tool, ~2.5 s each (5.0 s in `image:prepare`, 3.0 s
  in `image:build` on a PhotonVision build). The answers are now recorded in
  `.gaia-host-tools-probe` in the output tree with a key: Gaia's version and
  binary, the backend (for docker, the image id), `PATH` and locale, each tool's
  config symbol and policy, and for host probes each `PATH` entry up to the
  tool's binary with its inode, size, mode, mtime and ctime (ns). A matching key
  reuses the answers and reports "host tools probe: reused (inputs unchanged)".
  A changed input, a missing or corrupt file, or an unreadable input probes as
  before.

### Buildroot config overrides and cache settings in one olddefconfig run

- The config steps ran `olddefconfig` three times on a PhotonVision tree (after defconfig, after the config overrides, and after the download, compiler cache and parallel build settings). Each `make` is a container start with the docker backend (about 5 s each). The overrides and cache settings are now merged into `.config` together and applied by one `olddefconfig`, so the steps run defconfig and then one `olddefconfig` (two when there are fragments, as before). The final `.config` is byte-identical to the three-run sequence on a PhotonVision tree, with and without fragments; the cache settings still win a key the overrides also set.
- Cache-space warnings from the config steps now reach the step messages on a fresh configuration (they were dropped when the cache settings were applied in their own run).

### Buildroot host-finalize skipped when nothing changed

- Every Buildroot `make` ran `host-finalize`, which copies each package's
  per-package host tree into `host/`: on a real tree (130 packages) it walks
  ~770,000 entries to write ~25,000 files, ~14 s, even with nothing to build.
  Gaia's Buildroot patch now skips that copy while a marker in `host/` is newer
  than every installed stamp and everything under `per-package/` and lists the
  same packages. Gaia removes the marker whenever it changes those trees itself
  (package cache restores, package rebuilds and dircleans, full cleans, redone
  interrupted packages); refreshing the stamps of unchanged cached packages
  keeps it current. Measured: `make target-finalize` 24 s -> 13 s.

### Failure text and assembly checks

- The run summary (`<out>/.gaia/reports/<build>.summary.json`) has an `errors` array with each failed operation's `operation_id`, `code`, `class` and `message`. The last-run file (`<build_dir>/.gaia-run.last.json`) has an `errors` array with the same operation, code and message; `gaia status` and the TUI's finished-run view list them. Older last-run files read as having no errors.
- Assembly references into `$provider.images/<name>` (`files.src`, `files.src_glob`, `transforms.src`, partition `image`, archive member `src`) are validated against the Buildroot `expected_images` when that list is not empty: a name that is neither collected nor produced by the assembly is an `assembly_provider_image_not_collected` error. A glob is checked only when no expected name can match its literal prefix.
- `[[image.expected_images]] format = "file"` accepts a plain file of any name (no defconfig support needed). Use it for images Buildroot's post-image scripts write under a name without a format suffix. The other formats keep their name checks.
- Preview says a reuse component the recorded state never named "has no earlier record (first run with this Gaia)" instead of "changed".
- `image:prepare` no longer depends on `BR2_ROOTFS_POST_IMAGE_SCRIPT`: prepare stops at target-finalize, so only `image:build` takes the post-image script into its fingerprint. Post-build, post-fakeroot and external files still count for both. The first run after upgrading reruns prepare once if the build sets a post-image script, since its fingerprint changes.
- The run summary's `image_sizes` and size notes skip compressed archives (`.xz`, `.zst`, `.gz`, `.bz2`, `.lz4`, `.lzma`, `.zip`, `.tgz`, `.tar.*`): their file length says nothing about the disk inside.

### Smaller raw disk images

- `[[image.assembly.disks]] truncate = "last-data"` ends the raw file after the last written byte (rounded up to the alignment) while the partition table still lists every partition at its full size. MBR only.
- `[[image.assembly.disks.partitions]] materialize = false` keeps a partition in the table at its full size with nothing written (no image, no wipe). Validation rejects `image` or `wipe` with it.
- `[[image.assembly.disks]] ebr_placement = "packed"` writes the MBR extended partition's EBR chain in consecutive sectors at its start, so logical partitions stay in the table when the file is truncated. `"default"` (the default) keeps today's layout.
- The run summary reports `image_sizes` (raw file length and content bytes per published image) and a note per image.

### Buildroot reuse and reproducibility

- The build operation no longer repeats the prepare operation's config steps (defconfig, fragments, overrides, cache settings) when their inputs and the resulting `.config` are unchanged, and, for builds without an image feed, no longer repeats the `target-finalize` (about 40 s on a PhotonVision tree with nothing to build) when every package is installed, the config is unchanged and nothing was built since the last finalize. A build with an image feed still finalizes, since the feed is applied by a post-build script inside `target-finalize`.
- Buildroot's `make` and its scripts get `SOURCE_DATE_EPOCH` from the environment, else from the workspace git `HEAD` commit time. Package cache keys do not include it.
- The package cache reports installed packages it cannot key (for example packages with local sources). Installed packages with a key but no cache entry are already stored by the store step.

### Content digests

- Gaia records the sha256 of the files it writes (collected images, the image archive and assembly outputs) in a `.gaia-content-digests` manifest next to them. Each line holds the relative path, size, mtime in nanoseconds and digest. Later plans, previews and runs use a recorded digest while the file's size and mtime still match, so they no longer re-read multi-GB images. A file whose size or mtime differs is hashed again. The trust limit is (size, mtime): an edit that keeps both is not seen. The manifest is skipped by the collect-dir digest, so writing it does not change reuse decisions.

### Previews

- The Buildroot prepare operation stops after `target-finalize`; only the build operation (which adds the image feed and so changes the target tree) makes the filesystem images. Before, both did, and the prepare operation's images were always discarded (about 2.5 minutes per PhotonVision build).
- `gaia preview <build.toml>` (also `gaia run <build.toml> --dry-run`) shows,
  before a run, which operations run or are reused and why, and for a
  Buildroot image: the work dir placement (a disk tree moved into RAM is
  discarded), the config changes, host tool decisions, the clean a run would
  make (nothing, finalize, package rebuilds with reasons, or a full clean),
  the package cache restores and builds, and every path a run would delete.
  It works on a scratch copy of the tree's state and never changes the real
  output tree. `--json` prints the report; `--fail-on-clean` exits `3` when a
  run would clean the whole tree or delete something outside trash.
- `gaia preview` and `gaia plan` name the change behind each rerun: a git source whose locked commit moved reports `rev <old> -> <new>`, a dependent reads `because <op> runs`, and a top line counts the operations a change invalidates, direct and cascaded. A git source's dependents now key on its checked-out tree digest rather than its commit, so a rev bump with identical files reuses them at run time (the run checks this after the source is fetched; dependents of git sources rebuild once after upgrading). The prepare operation's output is now its Buildroot `.config` alone: the image build rewrites the shared collect-dir state on every run, which used to rerun the artifacts after prepare (prepare and its dependents rerun once after upgrading).

- `gaia preview` names the inputs that changed for an operation whose fingerprint changed: `image:build: config_overrides changed (BR2_X)` for Buildroot overrides (per key), `artifact:<id>: build_env changed (GRADLE_USER_HOME)` for Java environments, and the same for defconfig, fragments, the Buildroot source, toolchains, source revisions and dependency outputs. The inputs are recorded per operation in a `<build>.reuse-state.details` sidecar next to the reuse state, which older Gaia ignores; state written before it still reports `fingerprint changed (no detail recorded)`.

### Run output

- `gaia validate` ends with a verdict line, `validate: ok (N error(s), W warning(s))` or `validate: FAILED (...)`, so `gaia validate build.toml | tail -1` tells a pass from a failure. Exit codes are unchanged (`0`, `2` for validation errors); a build config that does not resolve ends with `validate: FAILED` on stderr.
- `export_dir` in `[image.output]` (per build) and `[workspace]` (the default for the workspace) names the directory a successful `gaia run` exports to, so `--export` is not needed on every run. Precedence is `--export <dir>`, then the build's `export_dir`, then the workspace's; `--no-export` skips the configured export for one run. Relative paths resolve from the workspace root and `~/` expands to the home directory. The `--set` keys are `image.output.export_dir` and `workspace.export_dir`.
- `gaia run <build.toml> --export <dir>` copies the primary image output (the image archive or the collect directory's disk images) into `<dir>` after a successful run, under a versioned name (the existing name when it already carries a version number, otherwise `<build>-<version>.<ext>` or `<build>-<UTC time>.<ext>`), and prints `exported: <path>  sha256 <hex>  <size>` per file. An existing file with different content is never overwritten (`-1`, `-2`, ...). A failed or cancelled run exports nothing and says so. The directory can also come from the build config (`export_dir`, see the bullet above).
- Every archive the run writes reports its compression: a step time `archive <file> (<xz|zstd|gzip|tar>)` in `summary.json` and the slowest-steps list (measured without paused time), and a log line `archived <file>: 1.4 GiB -> 142 MiB in 23s` with the input and output sizes. This covers the compressed raw disk image (`.img.xz`, `.img.zst`), the Buildroot image and tar archives, and the assembly's gzip and zstd transforms and tar archives.
- A failed or cancelled `gaia run` ends with `resume: gaia run <build> <same --preset/--env-file/--env/--set/--only>  # finished work is reused`, so the invocation to pick the run back up is at the bottom of the output. Secret values stay masked.

### Config safety

- Buildroot `config_overrides` are verified against the final `.config` after
  the defconfig, fragments, overrides, cache settings and `olddefconfig`, for
  private and shared trees. Entries that Kconfig dropped (missing or
  `# KEY is not set` when `y`/`m`/a value was requested) or changed are listed
  with the requested and final value and the hint "usually an unmet
  `depends on`; check menuconfig for <KEY>". `[providers.buildroot]
  override_check = "error" | "warn" | "off"` (also
  `--set policy.providers.buildroot.override_check=...`) defaults to `"error"`,
  which fails the image operation before the long `make`. **This can fail
  builds that previously succeeded with silently missing packages**; set
  `"warn"` to keep building. Warnings appear in the `gaia run` output,
  `summary.json` (`image_warnings`, counted in `warning_count`) and on the
  `manifest.json` image record (`warnings`).
- Build files accept `gaia_version` (a semver requirement such as
  `">=2.0.0"`). It is checked on the raw TOML of every loaded file before any
  other parsing or validation; a too-old binary fails with "this build
  requires gaia >=X, but gaia Y is installed; upgrade with: cargo install
  ...".
- Empty or whitespace-only config files (entrypoint, `extends` parents, imports) fail to load instead of contributing nothing: a truncated payload layer had silently dropped four applications from an image while validation reported no problems. Files that set nothing (only comments) are reported as `config_layer_empty` warnings.
- Added `[expect] artifacts / installs / sources`: ids the build must have, from any layer; validation fails (`config_expected_missing`) when one is missing.
- **Unknown keys fail validation** (`config_unknown_key` errors; top level,
  the non-`kind` tables such as `providers.*`, `execution`, `stage`,
  `reporting`, and the image's nested tables), so a typo or a setting this
  binary does not support stops the build instead of being ignored.
- The Buildroot provider's config steps moved from `buildroot.rs` to
  `buildroot_config.rs`, bringing `buildroot.rs` under the 800-line limit.

### Reuse fingerprints

- Docker-backed artifacts no longer probe `cargo`/`rustc`/`mvn`/`gradle`/...
  on the host (which also logged "process start failed" warnings when the
  tools were only in the container). Their fingerprint uses the execution
  image: the Dockerfile content hash for Dockerfile-built images (no Docker
  call), otherwise the image id from `docker image inspect`, memoized per
  process, or `image-missing:<tag>` when the image is not present. This
  changes the fingerprint of docker-backed artifacts once, so they rebuild on
  the first run after upgrading.
- Host tool probes check `PATH` first, so a missing tool no longer logs a
  process start failure (its signature is unchanged).
- Path sources fingerprint their tree by names and file contents (plus mode,
  length and symlink targets), not directory timestamps or sizes. Gaia's own
  state inside the tree (`.gaia`, `.gaia-trash`, `.gaia-run*`, and the
  workspace build and out dirs) is skipped, so a `source:workspace-root` at
  the workspace root no longer rebuilds on the first run after a fresh
  workspace. Real edits, additions and removals under the root still change
  the fingerprint, and the planner and the source provider share one rule.
  The fingerprint format changed, so path sources rebuild once after upgrading.
- Image build and image assembly fingerprints hash content instead of provider state. The build's signature is the digests of the files in its collect dir (the collected images, the provider marker and the archive), and assembly's is the digests of its transform, filesystem, disk and archive outputs, keyed by the spec's output templates. The provider rewrites `.gaia-image-state.txt` on every build (mtime digests, absolute paths, `reused=`), so before this a rebuild that produced identical images still re-ran assembly and everything after it. Identical rebuilds now keep assembly reused; a changed image byte or assembly output still re-runs it. The signature format changed, so image assembly re-runs once after upgrading. Digests are cached per process by path, size and mtime.
- Image assembly is recorded under the fingerprint its inputs have after the run. The build rewrites the Buildroot files and collect dir that assembly fingerprints, so the planned fingerprint (taken before the run) differed from the next plan's, and `gaia preview` reported `image:assembly` as changed with nothing changed. The state now keeps the post-run fingerprint, with the components and input signature recorded from the same state. Assembly inputs are also named per input (`assembly inputs[src <path>]`, `archive <id>/<member> <path>`), so a changed input is named in the explanation; an explanation no longer says "no named input differs".
- The reuse planner decides operations in dependency order. An artifact planned before its dependency (for example one that waits for the image prepare step) used to read that dependency as rebuilding, so it and its dependents showed `dependency_rebuilt` even when the dependency was reused. The cascade message also dropped the dependency's reason. Now a dependent cascades only when a dependency finally runs.

- Edits to files a build reads now invalidate it. A Java artifact's `build_command` fingerprints the contents of each argument that names a file in the workspace, plus the files a shell script sources by a relative path (`build command script <path>`). Buildroot image operations fingerprint the post-build, post-image and post-fakeroot scripts (with the regular files beside each) and every file of each `BR2_EXTERNAL` tree (`post-image script <path>`, `external file <tree>:<path>`). A changed external file names the Buildroot packages its `.mk` assignments touch (for example `HOST_EROFS_UTILS_CONF_OPTS` names `host-erofs-utils`): those packages are rebuilt, and their package cache keys include the external files that touch them. A changed external file that touches no package no longer forces a full Buildroot clean. Gaia classifies it by what the configured tree reads it through: a file a package setting names (a kernel config fragment, a package config file) rebuilds that package, and a `patches/<package>/` patch rebuilds that package at any depth; a file under an overlay, a device or users table, or beside the post-build script reassembles `target/` without rebuilding packages; Kconfig, documentation and metadata files clean nothing; a `.mk` file, which may assign anything, still forces a full clean, and any other file reassembles `target/`. Trees built before this change record no external files: their current files become the baseline (nothing is rebuilt, and the run says so), and the next run compares them. Image operations rerun once after upgrading when the tree has an external tree or post scripts.

### Executor and runtime

- Nested Rust artifacts that share a workspace, target triple, profile, feature
  flags and execution backend are built with one `cargo build -p a -p b ...`
  and their outputs copied individually (`providers.rust.batch_builds`,
  opt-in, default `false`, because cargo unifies features across the batch). A failed batch falls back to per-artifact builds so errors
  land on the right artifact.
- CPU-heavy operations running concurrently split the available cores:
  each gets a job budget exported as `CARGO_BUILD_JOBS`, `MAKEFLAGS=-jN` and
  `CMAKE_BUILD_PARALLEL_LEVEL` (and Buildroot `make -j` when `local_jobs = 0`).
  A heavy operation running alone is unchanged; user-set variables win.
- `[failure] keep_going = true` (`policy.failure.keep_going`): after a failure,
  independent operations finish, dependents of the failed operation are
  skipped (new `Skipped` event, `skipped_operation_ids` in the summary), the
  run still fails, and finished work is kept and saved in the reuse state
  instead of being rolled back. Default behavior is unchanged.
- Streamed build output is no longer retained in full: operations keep a
  bounded tail (`failure_tail_lines`) for failure reports and successful
  operations no longer re-emit their whole log as events. Failure messages
  are now the provider's error; the streamed tail is in `output_tail`.
  Subprocess output retention uses ring buffers.
- Per-operation wall-clock timings are recorded in the run outcome and
  `summary.json` (`operation_timings`), printed by `gaia run` (slowest
  operations), and persisted in the reuse state (`dur=` lines). `gaia plan`
  and the TUI Plan panel show last durations, the estimated total work and the
  critical path.
- The TUI keeps its live log after a run instead of rebuilding it from the
  outcome, and shows skipped operations.
- Spawning a freshly written executable retries briefly on `ETXTBSY`, fixing
  intermittent "Text file busy" failures in tests that exec fake scripts.

### Rust artifact features

- Rust artifacts accept `features = [...]`, `no_default_features` and
  `all_features`, passed straight through to cargo. `all_features` cannot be
  combined with the other two. The flags are part of the artifact fingerprint
  and are recorded in the artifact backend state when set. Adding the fields
  changes every Rust artifact's fingerprint once, so existing Rust artifacts
  rebuild on the first run after upgrading.
- Rust build groups: rust artifacts with the same `build_group = "<name>"`
  are always built by one cargo invocation (`-p` for every member package,
  the sorted union of the members' features), so an engine binary and its
  `cdylib` plugins get identical feature resolution for shared crates. The
  invocation is the same whether members build together or alone, and group
  members batch without `[providers.rust] batch_builds`. Members must share
  `source`, `target`, `profile`, `execution` and
  `no_default_features`/`all_features` (`rust_build_group_conflict`);
  `build_group` on a non-rust artifact fails config loading. Artifacts
  outside a group keep their fingerprints. `cdylib` outputs are collected by
  setting `target_name = "lib<name>.so"`.

### Changed

- A failed or cancelled `gaia run` now keeps the work that finished. The
  default is `policy.failure.rollback_on_error = true` with the new
  `policy.failure.rollback_completed = false`: only the failed operation's own
  partial outputs are cleaned (`preserve_failed_outputs` still applies), and
  completed operations keep their outputs and are recorded in the reuse state,
  so the next run reuses them. Before, every completed operation of the run
  was rolled back and the next run rebuilt it. To get the old unwinding, set
  `[failure] rollback_completed = true` or pass
  `--set policy.failure.rollback_completed=true`. The run summary says which
  behavior applied.

### Added

- Added `--rebuild <operation>[,...]` to `gaia run`, `gaia plan` and `gaia preview`: the named operations (ids, domains, or `artifact:*` globs) execute regardless of reuse state, with reason `rebuild_requested`, and their dependents follow the usual `dependency_rebuilt` logic. Added `--rebuild-package <package>[,...]` for Buildroot images: the named packages are dircleaned before `make`, are not restored from the package cache in that run (the cache entry is stored again), and make `image:prepare` and `image:build` execute. `gaia preview` shows which packages would be cleaned. `gaia resume` does not repeat either flag. See `docs/cli.md`.

- Added `[providers.java] gradle_home = "workspace" | "user-cache"` (also
  `--set policy.providers.java.gradle_home=...`): `user-cache` moves a Docker
  Java build's Gradle user home to the per-user Gaia cache
  (`<user cache root>/gradle-home`), mounted into the container at the same
  path. Any other value is a validation error. The `GAIA_GRADLE_HOME`
  environment variable still overrides the setting but is deprecated; an
  unknown value fails the build. `gradle_home` set under another provider
  table is a validation warning. Java artifacts' `build_env` now also applies
  to default Maven and Gradle builds (it was ignored unless `build_args` or
  `build_command` was set).

- Added `gaia status [build] [--follow]`, which shows what a running `gaia run` of a build is doing from another terminal: the operations, Buildroot packages, running operations and recent output, read from `.gaia-run.status.json` in the build dir (also written with `GAIA_RUN_PROGRESS=quiet`; the final outcome lands in `.gaia-run.last.json`). `gaia tui <build>` attaches to a build that is already running (`p`/`r`/`c` to pause, resume or cancel, `q` to leave the monitor), and the picker marks running builds. Every `gaia run` now registers itself in a per-user registry (`$GAIA_RUNS_DIR`, else `$XDG_RUNTIME_DIR/gaia/runs`, else `~/.local/state/gaia/runs`), so the runs on the system can be found from any directory: `gaia status` with no argument lists them all (live runs, and runs that ended in the last 24 hours), `gaia status <n|name|build config>` shows one, and `gaia pause|resume|cancel` with no argument act on the only live run (they list the runs and fail when several are live). `gaia tui` from anywhere starts with a Running builds section, attaches to any of them, and opens even outside a project.
- Added the `xfs` Buildroot expected-image format (`BR2_TARGET_ROOTFS_XFS`, Buildroot 2026.08+), including shared-output packing. Gaia's Buildroot provider was checked against 2026.08: defconfig handling, the `.config` format used by `override_check`, kernel module install paths, rootfs fakeroot scripts and make targets are unchanged from 2025.11.
- Added `${project.commit}` (full sha, `-dirty` for uncommitted tracked changes) and `${project.describe}` (`git describe --tags --always --dirty`) for the git repository holding the build file, so image versions and update bundles can be unique per build.
- `${source.<id>.path}` resolves to the directory holding a source's files
  (an import source's checkout, a path source's directory, or
  `<build_dir>/sources/<id>` for git, archive and download sources). It and
  `${source.<id>.commit}` now also work in Buildroot `config_overrides`
  values, e.g. `BR2_ROOTFS_USERS_TABLES =
  "${source.orion.path}/packaging/buildroot/orion-users.table"`. The image
  operations depend on every source whose directory an override names.
  `BR2_ROOTFS_USERS_TABLES`, `BR2_ROOTFS_DEVICE_TABLE` and
  `BR2_ROOTFS_STATIC_DEVICE_TABLE` no longer force a full Buildroot clean
  when they change.

- Image assembly MBR disks accept more than 4 partitions: partitions 1-3 stay
  primary, slot 4 becomes an extended partition (`0x05`) and the rest become
  logical partitions p5, p6, ... with the standard sfdisk EBR chain. Layouts
  with 4 or fewer partitions are unchanged. `bootable` is rejected on logical
  partitions.
- Assembly disk partitions take an optional `size` (`"128M"`, `"2G"`, plain
  bytes); the image must fit and the rest stays unwritten (sparse). `image` is
  optional when `size` is set, giving an empty partition, and `wipe = true`
  zeroes the first MiB of an empty partition to clear stale filesystem
  signatures. `ByteSize` values also accept a `T` suffix.
- `zstd` assembly transform (`zstd -q -c --no-progress -T0`) with an optional
  `level` (1-19); the tool path and version are recorded like `gzip`.
- `[[image.assembly.archives]]` builds deterministic ustar archives after the
  disks from ordered file `members` and generated `KEY=value` files whose
  values can use `${assembly.sha256:<path>}`. Archives are published
  atomically, recorded in assembly state, allowed in publish-dir hygiene and
  covered by reuse fingerprints.
- Added imports from a git source: `imports = [{ source = "<id>", path = "...", when = {...} }]` reads a layer from the checkout of a pinned (`rev` or lockfile) `kind = "git"` source declared in a local config file, at resolve time, cached in `.gaia/cache/import-sources/<id>-<rev>` and never re-fetched. `@self/...` in any config string resolves to the containing file's directory and `@source:<id>/...` to the source checkout. Nested imports must stay inside the checkout. `--set sources.<id>.path=<dir>` reads the layer from a local directory and materializes the source as a path source. `gaia lock` pins unpinned import sources. The import source identity is folded into the fingerprints of items the layer declares, so changing `rev` rebuilds them.
- Added `--only <targets>` to `gaia run` and `gaia plan` to execute part of the build graph (a domain such as `artifacts` or `image`, or an operation id such as `artifact:<id>`) plus its dependencies. Partial runs keep the reuse state of operations they skip.
- Added `gaia tui --builds-dir <dir>` to choose the directory the build picker scans.
- Added a git source lockfile. `gaia lock <build>` records the commit each git source's branch/tag resolves to in `<build>.gaia.lock` next to the build entrypoint, and `gaia lock <build> --update [source-id]` re-resolves. Locked sources check out exactly that commit, count as pinned for reuse, and include the commit in their fingerprint. Validation warns about stale entries (`git_lock_stale`), rejects unreadable lockfiles (`git_lockfile_invalid`), and warns when two sources use the same repo at different refs (`git_source_ref_divergence`). Builds without a lockfile behave as before.
- Added Dockerfile-backed artifact execution images: `[artifacts.execution.docker] dockerfile` (and optional `context`) builds the image when missing and tags it `gaia-local/<name>:<content-hash>`. The hash is part of the artifact fingerprint, and the artifact state records the tag, image id and hash.
- Added `gaia clean <build> --target caches` to prune git mirrors no current source uses (including the old per-ref mirrors), leftover `.gaia-preserved` stashes and Buildroot `target.refresh` trees, reporting the size freed; `--all-caches` also removes the shared git, download, Buildroot download and Docker tool caches.
- Added a `stage_file_src_placeholder` validation warning for `[[stage.files]]` sources that are directories holding nothing but placeholders such as README or `.gitkeep`.
- Reworked the TUI:
  - The Plan panel lists every operation with its execute/reuse decision and reason.
  - The monitor follows the newest running operation (`f` toggles following) and jumps to the failed operation's logs when a run fails.
  - A `?` key-help overlay, `j`/`k` navigation, and `m` to return to a running build from setup.
- Added opt-in shared Buildroot output trees (`[providers.buildroot] shared_output = true`, optional `shared_output_dir`). Builds with identical Buildroot inputs (source identity, defconfig, fragments, config overrides, external tree, package overrides) compile one tree keyed by a digest of those inputs, under a file lock; each build gets private copy-on-write `target/` and `images/` clones, so feeds never leak between builds. Unused trees are removed when their last build moves to another key.
- Added the `image.buildroot.config_overrides.<SYMBOL>` override key for presets and `--set`, for example to pick zstd squashfs compression in a development preset while releases keep XZ.

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
- The Buildroot image feed is delivered through a generated post-build script on the `make` command line, so one `make` packs every image once with the feed included, instead of packing, applying the feed to `target/` and repacking (2–3 squashfs packs per build). Stale feed files are still pruned first; if Buildroot does not run the script, Gaia falls back to the previous refresh. The direct squashfs refresh fallback now deletes its `target.refresh` copy after packing.
- Squashfs tuning settings (compression, block size, padding) no longer count as config changes, so switching XZ and zstd between presets does not clean the Buildroot output tree. Existing config state remains valid.
- With `shared_output`, builds that differ only in their image feed (for example HeliOS `base-os` and `full`) share one ~16 GB Buildroot tree; after the first full `make`, the shared tree only runs `make target-finalize` and each build packs its own images once.
- Download sources with a `sha256` are kept in a content-addressed cache (`.gaia/cache/downloads/sha256/<sha>`) after verification and restored from it on re-materialization instead of being downloaded again; each download is hashed once instead of twice.
- Tool version probes used in fingerprints run once per process with a 10 second timeout, instead of per artifact with a 2 second timeout whose expiry forced rebuilds on loaded machines.
- Opt-in `[providers.rust] shared_target_dir = true` builds the cargo artifacts of every source into one target dir per toolchain, target, profile and execution backend under the user cache, so sources reuse each other's compiled dependencies (synthetic two-source build: the second source compiled only its own crate, 5.5-7.4 s for both instead of 11-12.5 s; under docker the same). A source whose local package name and version collide with another directory's package in that dir keeps its own dir, because cargo would otherwise reuse the other package's binary. Off by default.
- Buildroot operations report step times for nearly every phase (work dir placement, the RAM tree's source mirror, config steps, host tools probe, package graph, clean planning and application, build state records, collect expected images, state digest, archive checks, image output digests, and the feed phases), so an operation's steps add up to its duration. Steps derived from `make`'s log (`buildroot package <name>` and `buildroot finalize and images`) overlap the make and are not part of that sum.
- Buildroot hashes each collected image once. The copy into the collect dir computes the digest as it writes, and the state digest, the collect dir's content manifest and a tar archive's signature reuse it. Before, an expected image was read by `sha256sum` for the state digest, again to hash the collect copy, and again for the archive signature.
- File digests (the target tree, package directories, archive entries) hash in-process instead of spawning `sha256sum` per file: about 1.4 ms per file on the development machine.
- A RAM tree's placement uses the size recorded when it was last built instead of walking the whole tree; the walk still happens once at the end of a RAM build to record that size.

### Fixed

- Fixed operations stopped by the first failure's stop signal keeping their
  partial outputs. Their outputs are now cleaned under the failed operation's
  rules (`preserve_failed_outputs`, `rollback_on_error`, `rollback_domains`),
  and they are not recorded as completed, so the next run redoes them. Any
  operation whose error has kind `Cancelled` (stopped by the stop or cancel
  signal) is reported as cancelled, not as a second failure; a cancelled
  assembly step that used to report an error now reports the run as cancelled.
- Fixed timeouts, cancellation and Ctrl-C leaving docker containers running.
  - Gaia stopped only the docker client, so the container kept building with nobody reading its output; its next write then failed. A PhotonVision build shipped 58 of 1898 kernel modules this way, because Buildroot ignores a failed `modules_install`.
  - Docker runs now use `--init` and `--cidfile`, and a command stopped early has its container force-removed.
  - `gaia run` now handles Ctrl-C and SIGTERM by cancelling the build, which stops each command and its container; a second Ctrl-C exits at once.
- Added `[providers.buildroot] kernel_modules_check` (default `"error"`): an image whose `target/lib/modules` holds fewer modules than the kernel build's `modules.order` now fails instead of shipping incomplete.
- Added `${source.<id>.commit}` for stage env set values and build labels. It resolves to the exact commit each source builds from (lock, full-sha `rev`, import checkout, or a path source's git HEAD with `-dirty`), so images can record their device package and component commits.
- Fixed spurious full Buildroot cleans (a from-scratch rebuild) after changes that only affect image generation:
  - Gaia now keeps a snapshot of the `.config` each output tree was built from and compares settings semantically, ignoring `BR2_TARGET_ROOTFS_*` (filesystem image formats, sizes, compression), post-image and fakeroot scripts, download/cache locations, job counts and mirrors. Buildroot regenerates images on every `make`, so for example resizing the ext4 rootfs now just repacks it.
  - Package replacement changes are detected by content (relative paths, modes, file contents) instead of timestamps and absolute paths, so re-syncing the override trees or bumping an import source's rev without changing them no longer cleans.
  - Shared Buildroot trees are keyed by defconfig, fragment and package override content rather than paths.
  - Trees built by earlier versions keep their existing state and are not cleaned by the upgrade.
  - The clean step now says which input changed, and for a config change names the changed settings (`buildroot clean: effective config changed (BR2_PACKAGE_FOO, ...)`).
  - Import-source checkout directories (`.gaia/cache/import-sources/<id>-<rev>`) are ignored in config values, so bumping the rev of a source that provides a `BR2_EXTERNAL` tree, users table or patch directory no longer changes `BR2_EXTERNAL_*_PATH` and cleans the tree.
  - The `BR2_LEGACY=n` rewrite happens before the config is compared and recorded, so the snapshot always matches what the next run compares against.
  - The config snapshot and package replacement state are recorded once the config is final, before `make`, so a retry after a failing post-image script or assembly step resumes (`make` reruns only image generation and the post-image script) instead of cleaning.
- Buildroot config and package override changes rebuild only the affected packages instead of cleaning the whole output tree (often 1-2 hours). Gaia records Buildroot's package graph (`make show-info`) with each built config. On the next change:
  - Newly enabled packages, including new override or external packages, just build.
  - A built package whose options, override directory contents or version changed is uninstalled (files it alone installed in `target/`, `staging/` and `host/` are removed) and dircleaned with everything depending on it.
  - Disabled packages are uninstalled and their dependents rebuilt.
  - Packages that gained or lost a dependency (kmod once xz is enabled) are rebuilt.
  - Toolchain, architecture, libc and system-wide settings still clean the tree, as does any change Buildroot cannot report a package graph for.
  - The log names the packages and the settings that caused each rebuild or clean.
- Added `[providers.buildroot] parallel_packages`: `BR2_PER_PACKAGE_DIRECTORIES=y` and a top-level `make -j<local_jobs> -l<local_jobs>`, so independent packages build concurrently. Targeted rebuilds also remove the packages' per-package directories.
- Buildroot downloads and the compiler cache default to a cache shared by every workspace of the user (`$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`; `buildroot/dl`, `buildroot/ccache`), so other projects and rebuilds after a wipe reuse them. Previously downloads were cached per workspace and the compiler cache defaulted to Buildroot's `~/.buildroot-ccache`, which Docker builds lost.
- `[providers.buildroot.ccache]` sets `BR2_CCACHE_USE_BASEDIR=y` and takes `max_size` (default `50G`, written to the cache's `ccache.conf`). The run summary reports the run's hit rate (`buildroot ccache: 8123/9410 compilations from cache (86.3%)`), counted from a per-run ccache stats log so concurrent builds sharing the cache do not skew it.
- Added a Buildroot package cache (`[providers.buildroot.package_cache] enabled = true`, needs `parallel_packages`): packages built by any build of the user are archived by a content key (Buildroot infrastructure and referenced settings, the package's definition, patches, version, referenced settings and files by content, execution image, dependency keys) and restored instead of compiled when another build, or the same build after a wipe, needs the same package. Packages are restored dependencies first, with their dependencies' per-package trees linked in as Buildroot would. Packages built from local directories are not cached; text files holding the output path have it rewritten on restore, and packages whose binaries hold it (most host tools) are kept per output path, so they restore after a wipe of the same build but not into trees elsewhere; `linux` is restored only when no out-of-tree module still needs its build tree. LRU-evicted beyond `max_size` (default `100G`).
- Fixed installed artifacts with musl (or any non-`gnu`) targets, such as `aarch64-unknown-linux-musl`, failing the Buildroot image build at the end with "Gaia cannot verify that target": targets are checked by architecture (`aarch64`/`arm64`, `x86_64`/`amd64`, `riscv64*`, `arm*`/`armv*`/`thumbv*`, and `linux/<arch>` platforms) whatever their vendor, OS and C library. A declared target of an installed artifact whose architecture Gaia cannot check is now a validation error (`artifact_target_unverifiable`), before any build.
- `gaia run` reports where the time went: each operation's steps (every Buildroot command, the package cache restore and store, each assembly step, the slowest Buildroot packages of the run from `build/build-time.log`, and the finalize-and-images time after the last package) are recorded in `summary.json` (`operation_timings[].steps`) and the ten slowest are printed as `step time: <duration> <operation> > <step>`.
- With `parallel_packages`, Buildroot's filesystem images and post-image script run only when something they read changed: Gaia builds up to `target-finalize`, digests the finalized target tree (by content), the images packages installed, the `.config`, the files the image settings name and their directories, Buildroot's `fs/` and `support/scripts` and the host packages, and runs the rest of `make` (without finalizing again) only when that digest differs from the one recorded with the current images. Verified on Buildroot 2026.08: the images are identical, and an unchanged rebuild skips the image step.
- With the package cache, the stamps of built packages whose key is unchanged are refreshed before each `make`: sources copied again (newer modification times, same content) made restored kconfig packages such as busybox fail ("No rule to make target 'oldconfig'") and normally built ones redo steps.
- `archive_name` accepts `.img.zst`/`.raw.zst`: the raw disk image compressed with zstd (`-T<jobs>`), several times faster than xz to compress and decompress, for development images; `.img.xz` stays the smaller choice for releases. Applies to both assembly disks and a Buildroot provider's primary image.
- The Buildroot package cache stores each package as a directory of cloned files (`cp --reflink=auto`) with a manifest, instead of a zstd tarball: on a reflink filesystem (btrfs, XFS) storing and restoring copy no file data, and btrfs already compresses. A package's own files are found by checking each file's path in its direct dependencies' trees, instead of indexing every dependency's whole tree. The default cache directory stays on the build's filesystem (`<workspace>/.gaia/cache/buildroot/packages` when the user cache root is elsewhere). The key format is v2, so earlier entries are not read and age out.
- A Buildroot output tree with nothing built yet (no `target/`, `host/` or `per-package/`) is no longer cleaned for a missing config snapshot.
- Failed and cancelled image operations keep their step times too (the failed command's own time, and for a failed or interrupted Buildroot `make` its slowest packages so far), so a timed-out build still shows where the time went.
- While a Buildroot `make` runs, the `gaia run` progress line leads with Buildroot's progress: packages built out of the config's packages, the packages being built, and an estimate of the time left from how long the remaining packages took when last built (kept in `.gaia-package-durations` across cleans) scaled by the run's pace, excluding pauses. The operation count follows, so a long `image:prepare` no longer looks stalled at one operation.
- `gaia run` can be paused and cancelled without losing work. Ctrl-Z or `gaia pause <build>` stops every running command (SIGSTOP of its process group, `docker pause` of its container) and Gaia itself; `fg` or `gaia resume <build>` continues them. Paused time counts toward no timeout and is excluded from operation and step times (`paused_ms` in `summary.json`, `paused` in the run summary). Ctrl-C or `gaia cancel <build>` stops the build: finished operations are reused by the next run, rollback never touches the Buildroot output tree, finished Buildroot packages are stored in the package cache (also after a build error), and packages a killed `make` was in the middle of are built again from the start by the next run, which says it resumes.
- Buildroot settings that belong to no package no longer always clean the whole output tree. Gaia looks up what reads them: Buildroot's infrastructure makefiles, an architecture choice, or an `external.mk` use it cannot follow still clean everything; package `.mk` files and `<PKG>_*` variables or hooks an `external.mk` sets from them rebuild those packages; settings only target finalization reads (`BR2_ROOTFS_OVERLAY`, `BR2_ROOTFS_POST_BUILD_SCRIPT`, hostname, issue, root password, or `external.mk` lines feeding `PACKAGES_USERS` and the other finalize tables and hooks) rebuild nothing and, with per-package directories, reassemble `target/`; settings nothing reads rebuild nothing. Each decision is logged with its reason. Previously a users-table option in an external tree cleaned the whole tree.
- The Buildroot package cache has two levels: a system level shared by every project (`system_dir`, default `<user cache root>/buildroot/packages`; `dir` remains an alias) and a project level (`project_dir`, default `<workspace>/.gaia/cache/buildroot/packages`). `level = "system" | "project"` picks where packages are stored, and `project_packages` / `system_packages` globs place individual packages; restores look in the project level, then the system level. `max_size` applies per level. Also settable with `--set policy.providers.buildroot.package_cache.level|system_dir|project_dir=...`.
- `gaia cache [build]` lists the package cache (entries largest first with level, package, key, version, size and whether they restore only at their build path, plus totals per level), `--remove pkg[@key-prefix],...` removes single packages or builds, `--clear system|project|ccache` empties a level or the compiler cache; `--level`, `--package <glob>` and `--dry-run` narrow or preview any of them.
- Package keys ignore Gaia's own patches to Buildroot's `package/pkg-utils.mk` (the reflink copy of per-package directories): applying it changed every key, so each package was cached twice.
- A warning (in the run output and report) when the filesystem holding the compiler cache or the package cache has less free space than the cache's `max_size`.
- With `parallel_packages` on a filesystem that supports reflinks (btrfs, XFS), Buildroot's final assembly of `target/` and `host/` from the per-package trees clones files (`cp --reflink`) instead of copying them with `rsync`, keeping them independent of the per-package trees. Gaia applies this to the exact upstream `package/pkg-utils.mk` text only, after checking the output directory supports reflinks; the upstreamable patch is in `contrib/buildroot/`.
- Fixed image assembly running steps in a fixed kind order regardless of what they read: a transform reading a filesystem the same assembly builds (for example compressing `$provider.images/boot.vfat` for an update bundle) compressed the previous run's image, so the bundle carried a stale kernel. Steps now run after every step whose output they read (same file, a file in a directory it fills, or a glob it matches); independent steps keep the classic order. Dependency cycles are rejected at validation and run time, outputs of transforms, filesystems, disks and archives from an earlier run are removed before the steps run, and validation warns (`assembly_reads_later_output`) where earlier Gaia versions ran a step before what it reads.
- Fixed import-source fetches ignoring `[providers.git] timeout_seconds` and timing out after 60 seconds on slow disks or networks; they now use it (or `--set policy.providers.git.timeout_seconds`), with a 1800-second default. The git provider's own default timeout (clones and fetches of build sources) is now 1800 seconds as well, instead of 60.
- Package override changes are tracked per package (`.gaia-buildroot-package-overrides.digests`), so adding a package to an override tree, or changing one, no longer counts as a change to the others.
- `BR2_EXTERNAL_*` settings (tree names, paths, `git describe` versions) and `KEY=n` versus unset options are no longer config changes.
- Fixed a raw disk `archive_name` without `.xz` (for example `board.img`) publishing Buildroot's first expected image, such as a bare `rootfs.ext4`, under the disk's name and reporting it as the primary image output. When typed assembly builds disks, every `.img`/`.raw` archive name (compressed or not) now comes from the assembled disk, and with no archive configured the single assembled disk is reported as the primary output. Without an assembly disk, validation warns (`image_archive_not_a_disk`) when the Buildroot expected images are not a single raw disk image.
- The unknown-key check now covers the image's nested tables (`[image.output]`, `[image.feed]`, `[image.assembly]` and its trees, files, filesystems, disks and partitions), so typos such as `archive_format` are reported instead of silently ignored.
- Fixed imports whose `when` does not match still being loaded: local layers for other targets resolved their `@source:` tokens and fetched the source, so a multi-target build failed for every target when one target's device source was unreachable. Non-matching imports are now skipped entirely, and a git source used only as an import source by non-selected layers is no longer planned. `@self` and `@source:` tokens are also resolved in every entry of a `:`-separated list, not only the first.
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
- Fixed the Buildroot provider's default collect dir (`out/images/buildroot`) resolving against the process working directory; without `image.output.collect_dir` it is now `<workspace.out_dir>/images/buildroot`, and relative collect dirs resolve against the workspace root, matching planning and assembly.
- Fixed intermittent "Text file busy" (ETXTBSY) failures: generated fakeroot scripts run through `/bin/sh`, and the Buildroot provider tests create fake tools through a helper that never holds a write descriptor a concurrently forked test could inherit.
- Added typed assembly MBR layout controls for `first_lba` and `alignment_lba`, allowing board images to preserve firmware-sensitive partition layouts instead of always using 1 MiB partition alignment.

### Image assembly in RAM

- `[image.assembly] work_dir = "ram"` keeps an assembly's intermediates (trees, filesystem images such as `boot.vfat`, transform outputs under the work dir, and raw disk images) in tmpfs under `/dev/shm/gaia-<user>/<hash>/assembly`. The archive is compressed from the RAM copy of the raw disk, and every raw disk image is copied to its spec path on disk when built, as a sparse copy. Published outputs keep the same bytes and digests as disk mode. The RAM copies are removed when the assembly ends.
- Unset `work_dir` follows `[providers.buildroot] work_dir`: RAM when that is `"ram"`. A path keeps the intermediates on disk, as before.
- RAM is used only when the spec's expected intermediate size fits in `MemAvailable` and the tmpfs with 4 GiB to spare; otherwise the assembly runs on disk and the run reports why.
- Behavior change: `work_dir = "disk"` and `work_dir = "ram"` now mean the placement above, not directories named `disk` or `ram` under the workspace. In RAM mode a filesystem image such as `boot.vfat` is no longer left in the collect dir.
- The assembly state records `work_dir.placement`, `work_dir.path` and `work_dir.expected_bytes`.

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
