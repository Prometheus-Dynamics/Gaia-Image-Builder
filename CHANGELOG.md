# Changelog

All notable changes to this workspace should be documented in this file.

The format is based on Keep a Changelog and this project follows Semantic Versioning.

## [Unreleased]

- API cleanup: 329 exported items that no other crate used are now crate-private (`pub(crate)`), and the dead code that exposed (about 20 helpers, builders and constants nothing called, plus a duplicate source command-policy lookup) is gone. Error and record types that public functions return stay public. `gaia-app --no-default-features` builds without warnings (the TUI's file cache is behind the `tui` feature), and the docs no longer link private items.
- RAM work dirs: two runs starting at once could both find `<base>/gaia-<user>` missing, and the second failed with "File exists" (seen when nextest ran the assembly tests as parallel processes on GitHub). The dir is now created first and whatever exists is checked, so the loser accepts the winner's dir; a symbolic link or another user's dir is still refused.
- CI runners. The repository is a GitHub Action (`action.yml`): it installs Gaia (cached by its source), restores Gaia's download, git, Buildroot `dl`/ccache and cargo caches with `actions/cache`, runs `gaia run|validate|plan|prefetch` and uploads the export. `gaia run` on GitHub Actions prints validation diagnostics and failed operations as annotations and appends a Markdown job summary (outcome, run time, built/reused operations, image sizes and changes, errors, export/resume lines) to `$GITHUB_STEP_SUMMARY`. The `Gaia action` workflow runs the action on `imported-rootfs-minimal`. Guide: `docs/ci.md` (per-push vs nightly jobs, caches, registry, secrets, self-hosted runners).
- A run whose image or assembly operation is reused now carries the previous run's published outputs forward (when still on disk), so `--export` exports them and the summary names the primary image; before, a reused run exported nothing.
- CI: `cargo package --workspace` failed on every push since 2026-10-18 (the GitHub `docs-and-lints` job): the dev-only, unpublished `gaia-testkit` was declared with a version, so packaging looked it up on crates.io. It is now a path-only dev-dependency, which packaging drops.
- Rust artifacts take `build_env` (`[KEY, value]` pairs for cargo, config tokens and `${secret:NAME}` in values): the secret is read when cargo starts and reaches a docker build as `-e KEY`, never the command line or the spec. Gaia's toolchain, sccache and offline variables win; `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` are refused with a toolchain (cargo would drop the toolchain's flags), and build group members must share `build_env`.
- `src_dir` copies keep each directory's source permission bits; `dir_mode = "0750"` sets every copied directory instead. Modes are applied after the contents, deepest first, so read-only directories copy.
- Gate speed: the gate runs its tests with cargo-nextest (`.config/nextest.toml`; `scripts/ci.sh` falls back to `cargo test` where nextest is missing, CI installs it and uploads a JUnit report), and the dev profile keeps line tables only (no debuginfo for dependencies). Measured on a 24-core machine, cold: clippy 11 s, test build 31 s → 24 s, tests 63 s (`cargo test`) → 9 s; target dir 19 GB → 8.6 GB; full gate (fmt, sizes, clippy, tests, doc tests) 17 s warm. The two slowest tests no longer wait 10 s for a timeout or scan a 256 MiB disk.
- `[[stage.files]] content = "..."`: text written to `dest` in place of a copied `src` (exclusive), with config tokens rendered, so `/etc/os-release` can carry `${build.version}` (asked by HeliOS, whose static os-release showed the release number instead of the build). A patch setting `content` replaces the entry's `src`, and the reverse.
- Packages: an ipk's data tar leaves out the `./` root entry, which opkg recorded as a file `.` and reported as obsolete on upgrades ("Removing obsolete file ."); a deb keeps it. The generated postinst runs `systemctl reset-failed <unit>` before enabling and starting it, so a unit that hit its start limit under the old version starts after the upgrade (both seen on a SystemCore).
- `${project.commit_count}`: the number of commits HEAD has, for versions that order (`...-dev.${project.commit_count}+${project.commit_short}`).

- Version: the workspace is `2.0.1-dev` (dev builds carry `-dev`; only PATCH moves until a release). `gaia_version` requirements compare the installed version without its pre-release tag, so `2.0.1-dev` satisfies `>=2.0.0` and `>=2.0.1`. Products can stamp their commit with `${project.commit_short}` / `${project.describe}` (already available).

- Assembly with `[image] kind = "none"`: commands (package compression, checks) got a 1 s timeout because no image provider sets one; an unset timeout now means 3600 s (found by HeliOS packing its SystemCore ipk).

- Profiles: one build file makes several device profiles from shared layers. A layer selected by `when = { target = ... }` adds to the shared software; `--set build.target=<name>` picks the profile. Patch entries: an `[[artifacts]]`, `[[install]]`, `[[stage.files]]` or `[[stage.services]]` entry without its declaring key (`kind`, `artifact`, `src`, `name`) changes the earlier entry with its id, key by key (a table value replaces the whole table). A patch with no earlier entry, or with no key but `id`, is an error that names its layer and line. The merge records what each patch and each image kind switch changed (`note:` lines in `validate` and `plan`, and `config_notes` in the spec). A later layer that sets `[image] kind` to another kind replaces the kind-specific image fields (the feed, checks, hardware, layers, signing and the Buildroot or starting-point fields) and keeps `[image.output]` and `[image.assembly]`. A toolchain id may be interpolated (`toolchain = "${input.toolchain}"`); empty means the host. `examples/profiles` builds a Buildroot `raze` image and a `systemcore` ipk from one file. Docs: `docs/configuration.md` ("Profiles").

- `[image] kind = "none"` no longer feeds the installs and stage entries of the build to an image: the feed is empty unless `[image.feed]` sets it (the rootfs defaults made every such build fail validation).

- `src_dir = "<path>"` on `[[image.assembly.files]]` copies a whole directory tree under `dest` (its contents): recursive, sorted, symbolic links kept as links, regular files keep their modes unless `mode` is set, `exclude` by name at any depth, an absent or empty directory fails unless `optional`. Each copied file gets `strip` and the dynamic-library check as `src` does. The assembly's fingerprint covers the tree's names, contents, modes and link targets, so a nested change reruns it. `src_glob` (file entries and archive members) now skips a directory that matches instead of failing.

- Archive sources: `allow_absolute_symlinks = true` accepts symlinks with an absolute target (a rootfs or SDK sysroot); still refused by default. Verified against Limelight's SystemCore ALPHA 17 SDK: downloaded and checked by sha256, extracted, relocated, used to cross-build a rust program with 16 KiB pages into an ipk that passes the device checks. A tree packaged with `device` checks no longer warns that it lacks the libraries the device provides.

- Packages: `format = "deb"` writes a Debian package (the ipk layout with `debian-binary` required, `<Package>_<Version>_<Architecture>.deb` names); `dpkg-deb` reads it back.
- Tar sources: a symlink target with `..` that stays inside the archive (kernel headers' `include/dt-bindings/input/../../uapi/...`) and hard links between members (toolchains link `gcc` to `gcc-14.x`) are extracted; links that resolve outside are still refused. An archive source's `url` download uses the archive provider's `timeout_seconds` and retries.

- `[[toolchains]] page_size` (a power of two, at least 4096): a rust artifact of the toolchain links with `-z max-page-size=<page_size>` (in its flags, part of its fingerprint), and a device package of it checks segments against that size by default. Explicit `min_page_size` wins.

- Cross builds against a Buildroot SDK. `[[toolchains]]` declares an SDK (its root is a source's tree or a local directory; its compiler prefix, triple, sysroot and Rust target; `relocate` runs `relocate-sdk.sh` once per root). A rust artifact takes `toolchain = "<id>"`: Gaia cross-compiles it with the SDK's linker, C compiler and `--sysroot`, in the docker backend too, and the toolchain is part of the artifact's fingerprint and registry key. Validation covers the ids, the roots and sysroots, the non-rust use and the missing target. Docs: `docs/configuration.md` (Toolchains, with the SystemCore example; Sources; Packages (ipk)).

- `[image.assembly.packages.device]`: the ELF files of an ipk are checked against a device sysroot (`toolchain` or `sysroot`) before the package is written. The architecture comes from the sysroot's `libc.so.6`; each needed `GLIBC_x.y` must be at most `max_glibc` (default: the sysroot's highest); each `PT_LOAD` alignment must be at least `min_page_size` (default: the toolchain's new `page_size`, else 4096); each `DT_NEEDED` library must be found in the sysroot or the package (`$ORIGIN` honoured; `check_needed`). Every problem is reported, and a failing package is not written. The ELF reader reads symbol versions and `PT_LOAD` alignments.

- `[[sources]] kind = "archive"` takes `url` with its `sha256` as an alternative to `path`: downloaded once into the download cache, verified before extraction (a mismatch names the expected and the actual digest), and read from the cache alone offline. Its content identity is its sha256.

- Workspace paths inside the workspace's `build_dir` or `out_dir` resolve even when those sit outside `root_dir` (the examples' `build_dir = "../../.gaia/<name>/build"`): an assembly `work_dir = "${workspace.build_dir}/assembly"` failed with "escapes its workspace root". Other paths outside the root are still refused.

- `[image] kind = "none"`: a build with no rootfs, kernel or image. No image provider runs, so the plan has no `image:prepare`, `image:build` or `image:check`; the build is its sources, artifacts, installs and `[image.assembly]`, which runs after the artifacts, installs and stage entries (and their `${source.<id>...}` sources) it can read. A checkpoint anchored at `image` anchors at the assembly. The primary output is `image.output.archive_name`'s archive, else the first package, archive or disk the assembly makes, so an ipk package is a build's deliverable. `$provider.*` assembly tokens, `[image.feed]`, `[image.checks]`, `[[image.hardware]]` and `[image.layers]` are validation errors (each names the field and says there is no image). Documented under "None" in `docs/configuration.md`; `examples/systemcore-package` is an ipk-only build.

- `[[image.assembly.packages]]`: opkg packages (`.ipk`) written from an assembly tree. The tree as installed is the data; the control fields (`Package`, `Version`, `Architecture` with no default, `Maintainer`, `Description`, `Depends`, `extra` fields), the maintainer scripts generated for systemd `services` (or given as files), and `conffiles` come from the spec. `debian_binary` (default on) sets the `debian-binary` member, `allowed_prefixes` limits where the files may be, and `dest` is a file or a directory the `<Package>_<Version>_<Architecture>.ipk` name goes in. The archive is deterministic (the OCI tree-tar walk, `gzip -n -9`, a hand-written ar) and is read back before it is published: members, control, scripts, conffiles, service units and prefixes are checked, and a failing package is not published. `format = "deb"` is reserved. Documented under "Packages (ipk)" in `docs/configuration.md`.

- `image:check` evaluates the firmware rule on the boot trees the image ships: the assembly trees with a `config.txt`, and Buildroot's `images/rpi-firmware` only when the assembly makes none. A slot image's build failed on Buildroot's own `rpi-firmware/config.txt` (an include it lacks), which the board never reads.

- Assembly: the parallel step scheduler compared paths with a plain prefix test, so a step reading a glob (`$assembly.tree.payload/overlays/*.dtbo`) did not wait for a step writing a file the glob matches (`payload/overlays/ov9782.dtbo`, copied from a transform's output) and could run first, silently missing that file. It now uses the same glob-aware test as the step ordering. Since the parallel scheduler (0c5a6a9), images whose trees fill from globs over files other steps write could miss files; Raze slots lost ov9782.dtbo and raze-fan.dtbo from the FAT root overlays this way (found by Atlas).

- `image:check` reads the assembly's boot trees from a copy of their boot files (`*.txt`, READMEs, `.dtbo`, `.dtb`) that the assembly leaves in `<build_dir>/image-check/boot-trees/<tree>`: with a RAM work dir (the `auto` default when it fits) the trees are gone when the check runs, so the firmware rule and the hardware list saw only Buildroot's overlay dirs and reported overlays the assembly compiles (Raze's ov9782 and raze-fan) as missing. An assembly from before this runs once more to leave the copy.

- `[[image.assembly.checks]]`: commands (`command = [...]`, assembly tokens expanded, optional `timeout_secs`) run after every assembly step over the finished trees and outputs; a non-zero exit fails the build with the output tail.

- `image:check` follows the Raspberry Pi firmware's boot model. It passed an image whose camera overlay (`ov9782.dtbo`) lived only in a slot's `overlays/` that has no README: the firmware reads the root `overlays/` there, so the shipped images had no camera. That false pass is closed. Every boot tree's `config.txt` and `tryboot.txt` are read as the firmware reads them (`include`, `os_prefix`, the README rule), each `dtoverlay` must have its `.dtbo` in the directory read, a slot `overlays/` without its README is an error, and the root fallback copy must hold the slot's overlays. Hardware overlays are looked up in the same directories only. `[image.checks] pi_model` evaluates the model sections.

- Registry checkpoints: an artifact's registry key is now a sha256 over its build fingerprint, its sources' content (git commit, path-source file contents independent of location, archive bytes) and its dependencies' keys. Before, the key was the build's 64-bit fingerprint alone, which does not carry source content: after a source edit a build could pull the entry built from the old source. Found in the live local-registry test. Entries pushed under the old keys are no longer used.
- Docker-backed builds pass an environment value that holds a secret as `-e KEY` with the value in the docker client's environment, never on the command line.

- Registry checkpoints: a refused (unverifiable) entry and a failed pull or push are `WARN` lines in the run instead of only operation-log lines; a push overwrites a tag whose entry does not verify instead of skipping it, so a poisoned entry is replaced by the next signed build; an artifact restored from the registry counts as reused. Found in a live test against a local `registry:2` with gpg signing.

- `[image.layers] erofs_options` (and per layer, `[image.layers.<name>] erofs_options`) sets the layers' `mkfs.erofs` options; the default is LZMA level 6, 256 KiB clusters, all-fragments, dedupe and ztailpacking. Layer erofs images were uncompressed before. They are now made from the layer tar unpacked under `fakeroot` instead of `mkfs.erofs --tar`, which loses file data with `-Eall-fragments` (erofs-utils 1.8.10).

- The Buildroot provider's `.gaia-package-durations` (the progress estimate's per-package times) drops packages the tree no longer builds, so it describes only the current config.

### Secrets

- **`[secrets]` names values a build reads without storing them.** An entry takes one source: `env = "VAR"`, `file = "path"` (mode 0600 or stricter, `~` expanded, refused with `chmod 600` named otherwise) or `command = [...]` (trimmed stdout, stderr discarded). `required = false` makes a missing source resolve to nothing. Values are read when they are used, never at load, so they are not in the resolved spec, fingerprints, state, provenance or `gaia config show`.
- **References.** `secret:NAME` as a whole value in `[image.signing] key` and in checkpoint `sign_key`, `verify_key`, `token` and `username`; `${secret:NAME}` inside an artifact's `build_env` only, substituted when the build process starts. Anywhere else `${secret:...}` is a validation error. Checkpoints gain `token` (`secret:` or `env:`, before `GAIA_REGISTRY_TOKEN`, the docker config and `gh`) and `username` (default `gaia`).
- **Masking.** Each resolved value of at least four characters is replaced with `reporting.masking.replacement` in captured output, operation logs, failure messages and report files, whatever `masking.enabled` says.
- **`${env:NAME}` warning.** Validation warns when a `${env:NAME}` whose name contains `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY` or `KEY` lands in the spec.
- Docs: `docs/configuration.md` (Secrets, Registry checkpoints, Image signing).

### Registry checkpoints

- **`[[checkpoints]] backend = "oci"` makes an OCI registry a build cache.** Buildroot package builds, artifact outputs and image layers are stored as signed OCI artifacts at `<repository>/<kind>:<key>` and fetched on a miss before they are built; new ones are pushed after the build. Entries are signed with the `[image.signing]` tool (minisign, gpg or cosign-blob) and a pull is used only when its signature verifies, its annotations name the requested kind and key, and its blob matches the manifest digest. A signature that does not verify refuses the entry with a loud warning. Pulls need a `verify_key` unless `trust = "unsigned"` is set (unsafe). Pull and push failures are warnings.
- **Registry access is curl.** Gaia runs the external `curl`, `tar` and `zstd`; credentials come from `GAIA_REGISTRY_TOKEN`, the docker config, or `gh auth token` for ghcr.io, and reach curl through a 0600 config file. A bearer realm receives the credentials only on the registry's own host.
- **`gaia cache --prune --remote --checkpoint <id> [--keep-days 30] [--dry-run]`** deletes registry entries older than the window (created time from the manifest; unreadable times are kept), with their signature tags. Only the generic OCI DELETE is used.
- Not yet: the roots index (keeping entries an older build still uses) and per-kind hit and miss counts in `gaia cache stats`.
- Docs: `docs/configuration.md` (Registry checkpoints).

### Canonical tree path

- **`[execution] canonical_tree = true` (opt-in).** With `work_dir = "ram"`, the Buildroot tree lives at `/dev/shm/gaia/<hash>/buildroot-output`, where the hash names the build id and the Buildroot config identity (not the checkout). Checkouts of one build share that tree path, so pinned package-cache entries (built into one output directory) restore into another checkout. A live owner record stops a second build of the same id from sharing the tree: it falls back to its per-checkout tree with a message. Disk work dirs keep the per-checkout placement.
- Docs: `docs/configuration.md` (Canonical tree path).

### Image layers

- **`[image.layers]` splits the Buildroot rootfs into layer tars.** Gaia forces `BR2_TARGET_ROOTFS_TAR=y` and splits `rootfs.tar` by path into one deterministic tar per layer (`layers/<name>.tar`), keeping each entry's owner, mode and links. Membership comes from `paths`, the stage, install and service ids, the Buildroot packages (`packages-file-list.txt`) and a default layer; overlapping non-directory paths are an error unless listed in `overrides`. Each layer also gets an erofs image (`mkfs.erofs`, on by default) and an optional dm-verity tree, and `layers/layers.json` describes them (schema 1, with the layer `id` as the erofs sha256).
- **Unchanged layers are reused.** Each layer's fingerprint covers its members and output settings; a layer whose fingerprint and outputs are unchanged is reused without rewriting its tar, erofs or verity, and the summary says reused or rebuilt per layer.
- **Without `[image.layers]` nothing changes:** no tar is forced and the full image is built as before.
- Docs: `docs/configuration.md` (Image Layers).

### OCI images from layers, without a daemon

- **`[[image.assembly.oci]]` writes an OCI image.** Its layers are uncompressed layer tars (`{ tar = ... }`) or assembly trees (`{ tree = ... }`), lowest first. The output is an OCI image layout directory (`format = "oci-layout"`, the default; `podman load`, `skopeo` and `docker load` read it) or a `docker load` tarball (`format = "docker-archive"`). Layers are gzip (default), zstd or uncompressed. The image config takes the architecture (required, `amd64` or `arm64`), `os`, `entrypoint`, `cmd`, `env`, `working_dir`, `stop_signal`, `labels` and an optional `ref`.
- **Same inputs, same bytes.** Tree layers are tarred deterministically (sorted, mtime 0, uid and gid 0, modes kept, symlinks and hard links kept). The config's `created` is `SOURCE_DATE_EPOCH` or the epoch.
- **Only what changed is compressed.** Compressed blobs are cached under the build dir by layer `diff_id`, so an app-only change rewrites one blob, the config and the manifest. The run state records each layer's digest and whether its blob was reused.
- Docs: `docs/configuration.md` (OCI images).

### Image checks: kernel modules (`image:check`)

- **A new `image:check` operation verifies the kernel modules of a Buildroot image against its kernel.** It runs after the image build and the assembly, reads only, and writes `<out dir>/<build>.checks.json`. Errors fail the run; warnings show in the run summary. It is on by default for Buildroot images and skips with a "skipped" report when the Buildroot config builds no kernel (`BR2_LINUX_KERNEL=y`). `[image.checks] kernel_modules = false` turns it off. `modules_dir`, `symvers` and `kernel_image` override where the module tree, the kernel's `Module.symvers` and the kernel image are read from.
- **What it checks:** each shipped module's vermagic against the kernel release read from the shipped kernel image (and that all modules agree on the flags); every import against the kernel's exports and the shipped modules; each `__versions` CRC against the kernel's (an ABI mismatch is an error); `depends=` against the shipped and built-in modules; duplicate module names, with the copy `modprobe` loads named (from `modules.dep`, else depmod's search order); and `firmware=` files under `lib/firmware` (a warning).
- **Reader:** a hand-written ELF64 reader for the parts a module carries. Compressed modules and a gzipped kernel are decompressed with `xz`, `zstd` or `gzip` from `PATH`. No new crates. On the Raze Buildroot output (252 modules, 11,613 imports, one kernel) the checks take about 0.36 s in a debug build and find no error; they warn on three modules shipped twice and on optional `r8152` firmware the image lacks.
- **Known limits:** the check's reuse follows the content of the build it reads (the assembly, or the image build's collected output), so a change to a module that leaves the collected output unchanged is seen on the next build that changes it. Symbols are not checked without a `Module.symvers`.

### Stage files mirror their sources

- **A `[[stage.files]]` entry removes what its source no longer has.** A directory entry used to keep the files it installed earlier after they were removed from the source (HeliOS's `ui/mock` stayed in `target/` and shipped). Each successful apply now records what every entry installed in `<output>/.gaia-image-feed-stage-owned.txt`. The next apply removes the paths an entry owned and no longer produces, before it copies: a file whose size and modification time still match what the stage left, a symlink with the same target, and a directory once it is empty. A file a package rewrote, a path another entry or the feed now produces (a moved path), and anything the stage did not record are kept. A removed entry's paths go the same way; a single-file entry whose destination changed removes the old one.
- **Upgrading needs one clean build per existing tree.** The first run after this change has no record, so leftovers from earlier runs cannot be told apart from current files: they are not removed, and the build says "no stage ownership record yet; leftovers from earlier runs are not removed". Clean that image's Buildroot output once (or run a clean build) to drop them. From the next run on, the managed-paths list no longer names stage files and the runtime state no longer removes them: the record decides. On that first run the previous managed-paths list still removes the destinations it names, as before, so a removed stage item's old destination can go then, even when a package changed it.
- Docs: `docs/configuration.md` (stage files, the image feed).

### Image checks: hardware list

- **`[[image.hardware]]` declares what a Buildroot image must support, per device.** Each entry (`id`, `kind`, `modules`, `compatible`, `overlays`, `files`, `firmware`) is checked by `image:check` as a chain: each named module passes the module checks (the copy modprobe loads, for a name shipped twice; built-in drivers count through `modules.builtin`); each compatible is claimed by a driver's `of:` alias and named by an overlay of the entry or a shipped board DTB; each overlay ships as a `.dtbo` in a boot overlays directory; each file and firmware is in the rootfs. An error names the build, the entry and the broken link. `modules` may be empty, meaning any shipped or built-in driver may claim the compatible.
- **An assembly is checked too.** Its trees' `overlays/`, top-level `*.dtb` and `config.txt` (each `dtoverlay=` must have its `.dtbo`, in the overlays dir next to the config or a Buildroot one) are read alongside Buildroot's. An assembly's boot files are where a board's overlays are built, so an image built with an assembly is checked against them.
- **Every `dtoverlay=` in `config.txt` must have its `.dtbo`,** whether or not an entry names it.
- **`files` and `firmware` are checked against the final rootfs:** `target/` without the files of `[image.rootfs] exclude_packages`.
- **The check reads device trees with a small FDT reader** (header, structure and strings blocks; `compatible` properties; overlay fragments with `target-path` and `__overlay__`). No new crates. A truncated or corrupt file is an error, not a panic.
- **`kernel_modules = false` turns off only the module checks;** a non-empty hardware list still runs. Without a kernel, a non-empty list is an error (it needs the kernel's modules), not a skip.
- **Config and validation:** `image.hardware` is a known key (it no longer warns as unknown); a duplicate or missing `id` is an error, and so is a hardware list on a non-Buildroot image (nothing would check it). Adding or changing an entry changes the build's plan fingerprint, so the check runs again.
- **Report:** `<build>.checks.json` gains `hardware_entries`; the hardware errors use the codes `hardware_*` (listed in `docs/configuration.md`, Hardware list).
- Docs: `docs/configuration.md` (Image checks, Hardware list).

### Offline builds and prefetch

- **`gaia prefetch <build>` and `gaia run --prefetch`** download what a build needs and stop before anything is built. Every source is materialized; for Buildroot the tree is configured as a run configures it, then `make source` fills the download cache. The output names the new source files and the cache size. Prefetch does not yet fetch cargo git dependencies (`cargo_git_sources`): the first artifact build does that.
- **`gaia run --offline`, or `[execution] offline = true`,** builds without network access. Buildroot takes its sources from the download cache only (`BR2_PRIMARY_SITE` as a `file://` URL, `BR2_PRIMARY_SITE_ONLY=y`, no backup site), and these settings leave the output fingerprint and package cache keys unchanged. A missing download fails the make and names the package and `gaia prefetch`. Git sources check out from the cached mirror with no `ls-remote` or fetch; a commit the mirror lacks is an error. Cargo runs with `CARGO_NET_OFFLINE=true`, and Docker-backed artifacts run with `--pull=never`.
- Docs: `docs/cli.md` (prefetch, offline builds) and `docs/configuration.md` (`[execution] offline`).

### Config: locations, kind-checked keys, JSON Schema, fewer loads

- **Config errors name their place.** A parse error or a value of the wrong type reports `file:line:col` (from toml's span). An unknown key reports the line and column of the key, with a did-you-mean among the keys its table takes.
- **`[[sources]]`, `[[artifacts]]` and `[image]` are checked by kind.** A typo in a source, an artifact or an image key (including `[image.rootfs]`, `[[image.expected_images]]` and `[image.packages]`) is reported with the keys that kind takes; a key of another kind is reported. Keys of every kind that Gaia knows stay valid, and the examples and the TOML blocks of the docs name no unknown key (a test checks this).
- **`gaia config schema`** prints a JSON Schema of the build file, written from the same key lists, for taplo and VS Code's Even Better TOML (see `docs/configuration.md`). It checks key names and `kind` values; value types are not checked.
- **One load per build.** A build with `[[variants]]` loads and merges its file once. A variant whose overrides and presets do not change what the loader reads (no `input.*`, no `build.target/profile/branch`, no source path or `workspace.root_dir`, no preset) takes that merged config; any other variant loads its own. Each variant's plan from `validate`/`run`/`plan` is reused for its run when the spec is unchanged, instead of planned twice. Measured on `examples/buildroot-rust-minimal` with three variants added (four specs), a debug build: `gaia plan` made 4 config loads and 8 plans, now 1 and 4; its median time went from 60 ms to 47 ms. `gaia validate` went from 48 ms to 45 ms (it was already one plan per variant). The resulting specs are unchanged: a test compares them with a load per variant.
- **Variant output collisions compare paths.** Two variants that write the same file (`collect_dir/archive_name`, an assembly output, or the same export file name in one export dir) are reported, whatever else differs between them. Assembly outputs are compared as the config names them.
- **`gaia plan` names what changed.** A fingerprint change is explained from the inputs the last run recorded (`config_overrides changed (BR2_FOO)`), as `run` and `preview` already did. A shared operation of a variant run reads its recorded inputs from the shared store, so its explanation is named too.

### Scheduler, docker and image signing

- **The scheduler starts parallel work first.** When nothing runs, ready parallel operations start before a waiting exclusive one, which starts alone once nothing else runs (it used to start first, so the parallel work waited for it). Once eight starts have passed a waiting operation, nothing new starts until it can run, so an exclusive operation cannot be starved. Resource keys of a plan are computed once per run, and a running-resource ledger replaces the per-pick recomputation. Job budgets for CPU-heavy operations are sized from the cores the running ones do not hold, when each one starts; a running tool keeps its budget. No output bytes change.
- **Docker: streamed, cached context hashes.** `docker_build_context_hash` reads files in chunks and reuses the hash of an unchanged context in the same process. The hash is the same as before, so existing `gaia-local/` image tags stay valid.
- **`[providers.docker] reuse_container = true`** (default off) runs a run's commands in one long-lived container per image, mount set and user, with `docker exec`, and removes the container at the end of the run, on cancel and on panic. Measured on this host: 3.6 s per command with `docker run --rm`, 41 ms with `docker exec` (20 runs of `true`).
- **`[image.signing]`** signs the published outputs after a successful run: `kind = "minisign" | "gpg" | "cosign-blob"`, `key` (a path, or `env:VAR`), `outputs` (globs over the output file names). Each signature is written next to its output (`.minisig`, `.asc`, `.sig`) and listed with the published outputs; a `*.roothash` file is signed too. The tool must be on `PATH`. The validator rejects a bad kind, an empty key or no outputs.
- **Artifact output digests are computed in-process.** The rust/go/java/node/python `output_sha256` values come from the same SHA-256 as before, but without a `sha256sum` process. A file that cannot be read gets the `sha256-error:` placeholder (it used to be the `sha256sum` message, or `sha256-unavailable` when the program was missing).

### Test hygiene and shared helpers

- The buildroot cross-filesystem package-cache test removes its `/dev/shm/gaia-package-cache-trees-<pid>` and `$HOME/.cache/gaia-test-<pid>` directories when it ends, including on failure.
- `gaia-process` now holds the shared `hex`, `sha256_file`, `sha256_hex_bytes` and `git_command`; the plan, spec, app, source, artifact and image-providers crates use them in place of their own copies. Error texts are unchanged except for the artifact digest noted above.

### Assembly: output changes (rebuild the affected images)

These change image bytes, approved for this release. Reuse does not notice a tool or code change, so an image built before this release with an unchanged spec is reused as it is: remove the affected images to get the new bytes.

- **ext4 has no root reserve by default.** `mkfs.ext4` runs with `-m 0`, so the whole image is usable (mke2fs keeps 5%). The new option `reserved_percent` (0-50) restores a reserve. Images that relied on the 5% block reserve must set `reserved_percent = 5`.
- **Hard links are stored once in cpio archives.** The links of one file share an inode number and link count, and the data goes with the last link, so busybox-style hard links no longer duplicate their data. Inode numbers stay sequential in archive order; a tree without hard links packs to the same bytes as before.
- **The zstd transform defaults to level 19** (it was zstd's 3), the level `cpio-zstd` already used. `level` still overrides it. `long = true` (zstd only, default `false`) adds `--long=27` for level 19 and up over 128 MiB; a stream written that way decodes only with `zstd -d --long=27`, which is why it is opt-in.
- **Deterministic vfat images are reproducible.** `deterministic = true` (the default) now fixes the volume serial (`mformat -N`, from the filesystem id), gives every entry the modification time 1980-01-01 00:00 UTC (`mcopy -m` from a staging tree), and runs mtools with `TZ=UTC`. Before, the serial and the copy times changed on every run, so the image never repeated.

### Assembly: concurrent steps, cpio and tar

- **Independent assembly steps run concurrently.** The steps run on a worker pool (one per core) in dependency order: the writers of a tree run one after another, and a step that reads or writes a path another step uses waits for it. The state, messages and outputs are recorded in step order, so a run is the same as a serial one; the first failing step in step order decides the error, and the steps after it do not start. Measured on a synthetic assembly (six 40 MB files in three trees, two `cpio-gzip` filesystems and three `gzip` transforms, 24 cores): 19.9 s serial, 8.9 s concurrent, with the same state and messages.
- **cpio streams into the compressor.** `cpio-gzip` and `cpio-zstd` pipe the archive into the compressor's stdin instead of a full temporary cpio, and `zstd` gets `--stream-size` so its output is the file-input output. The archive is written through a buffered writer, and its size is planned from metadata first. `cpio-xz` keeps the temporary file: its proven-bootable command is unchanged. The output bytes of `cpio-gzip` and `cpio-zstd` are unchanged (checked against the file-input output in a test).
- **Device nodes and FIFOs in cpio.** A char, block or fifo entry in the tree is written with its mode and device numbers (it errored before). `[[image.assembly.devices]]` adds nodes the tree does not have, e.g. `dev/console` (char 5:1, mode `0600`) for an unprivileged build; missing parent directories are added. Validated: the tree, the path, the device numbers for the kind, and the mode.
- **Long tar member names and link targets.** The ustar `prefix` field holds a name that splits at a `/`; other long names, and long link targets, use PAX `path` and `linkpath` records. Member names may be up to 4096 bytes (100 before). `tar -tf` lists a 250-character path as written.
- **strip runs in batches.** ELF files are planned from their headers (read by seeking, not read whole), and the planned files are stripped in chunks of up to 64 per strip run, the chunks in parallel. The totals and the output are the same.
- **verity reads its report regardless of `output_retention`.** The root hash is read from a fixed retention for the tool report, so a small `output_retention` cannot break a verity build.

### Assembly: RAM work dirs are checked

- Before a RAM run starts, `/dev/shm/gaia-<user>/<hash>` dirs of dead runs are removed: older than 24 hours, and their owner pid is gone, their build dir no longer exists, or they have no owner record. Each run now records its owner (`owner`: pid, start time, build dir) in its `<hash>` dir.
- `/dev/shm/gaia-<user>` must be a real directory owned by the current user (created with mode 0700 when missing). A symbolic link, a non-directory, or a directory another user owns stops the build with an error; the tmpfs is shared, so such a directory is not used.

### Process: less work per output chunk

- A command's output is read into chunks the drain keeps: with `output_retention` at 0, nothing is copied or split into lines, so a chatty command costs the reader thread a read per chunk. Retained lines are sent one message per chunk, not one per line, and a line is split with a slice search, not per byte.
- The run loop wakes on output (at most every 100 ms when the child is quiet, so cancel and timeout still answer within 100 ms) instead of every 10 ms, and polls for the exit every millisecond only once the pipes have closed.
- Measured with `yes | head -c N` (process CPU, read from `/proc`): 20 MB with zero retention went from 8.2 s CPU and 268 s wall to 0.07 s CPU for 200 MB (12 s wall, the pipeline itself). With the default retention, 20 MB went from 8.8 s CPU and 181 s wall to 3 s CPU per 20 MB (30 s for 200 MB, 428 s wall): the default retention still keeps every line, which is the remaining cost.

### Assembly: ext4 walks the tree once

- An ext4 image walks its tree once: the size, the inode count and the ownership script share one entry list (three walks before), and the debugfs script is written in one buffer without a `format!` per field. The script's lines are the same as before.

### Buildroot: faster EROFS images, bounded make retries, reports

- **EROFS images build with several threads.** `[providers.buildroot] erofs_workers` (default `"auto"`: the smaller of 8 and the host's cores) builds erofs-utils with `--enable-multithreading` and passes `--workers` to `mkfs.erofs`. The image is byte-identical for every count (checked for the HeliOS target with `-C262144 -Ededupe -Efragments -Eztailpacking`, 8 and 24 workers), and on a loaded host the EROFS step ran about twice as fast with 8 workers. `"off"` or `0` keeps the single-threaded build. The first build after upgrading rebuilds `host-erofs-utils` once, and changing the setting on or off does so again. Every Buildroot build now gets Gaia's generated external tree (holding these make lines) unless the setting is off.
- **The long `make` has its own timeout and retries.** `[providers.buildroot] make_timeout_seconds` (default `0`: none) and `make_retry_attempts` (default `1`) apply to the build's `make` only; defconfig, olddefconfig and clean keep `timeout_seconds` and `retry_attempts`. A `make` that failed is retried only when its stderr reports a download or network failure, never on a compile error.
- **Buildroot's rebuild decisions hash files in parallel and reuse digests.** The target tree, package override trees, external trees and package directories are digested on up to 8 threads, and a file whose size and mtime match its recorded digest in the output tree's `.gaia-content-digests` is not read again. The digests themselves are unchanged. The package cache stores scan each package's files in parallel, with one read buffer per thread and `memchr` instead of a byte-window search.
- **The RAM work dir is private.** `work_dir = "ram"` places the tree under `/dev/shm/gaia-<user>/`, where `<user>` is now the account name of the real user id from the password database (it was `$USER`, which any process can set); for the usual case the name is unchanged, so existing trees are found again. Gaia creates that directory with mode 0700, tightens an existing one of yours to 0700, and refuses a symbolic link or a directory owned by someone else. A preview computes the path without creating it.
- **The image feed's fallback make gets the Buildroot environment.** The `target-post-image` make that refreshes images when Buildroot did not run the feed's post-build script now applies the policy environment (download and compiler caches, host tools, job counts), like every other make of the tree.
- **Package sizes.** Each Buildroot image build writes `<build>.package-sizes.json` (installed bytes per package, largest first) into the collect dir and names the ten largest packages in the run notes, with their change since the previous build.
- **License and manifest archive.** `[image] legal_info = true` runs `make legal-info` after a successful build and publishes `legal-info.tar.zst` (`manifest.csv`, `host-manifest.csv`, license files). The archive is kept while the package list is unchanged. Needs `tar` with zstd support.
- Shared helpers: one retry loop and backoff for the Buildroot and starting-point providers (`gaia_image_providers::run_with_retries`), one `hex_lower`, one `assembly_expected_image_names`.

Not in this change: `gaia run --prefetch` and `--offline`. Buildroot's download switches could not be checked against its source from this environment, so they are left for a follow-up.

### Git sources: faster materialization

- **Upgrading rebuilds the dependents of each git source once.** A git source's recorded `materialized_tree_digest` is now a git tree id (`git-tree:<id>`) for a clean checkout, not a hash of every file, so the state the dependents read changes form.- A git source's checkout is updated in place on each run instead of being deleted and cloned again. The checkout is made to match its commit: tracked files reset, untracked and ignored files removed, `.gaia` state kept, and any failure (another origin, a damaged `.git`, a commit that will not check out) falls back to a fresh clone. Measured on a 727-file local repository, an unchanged source went from about 0.30 s to 0.02 to 0.09 s and a locked source from 0.28 s to 0.02 s; a new commit went from 0.30 s to 0.06 s.
- A remote source's checkout shares its mirror's objects (`git clone --shared`) instead of copying the history, so a locked or `rev` checkout no longer copies every object. A checkout that lost its mirror is cloned again on its next run.
- A locked commit, or a `rev` that is a full commit id, already in the mirror is checked out without a fetch. A branch or tag is resolved from the mirror after its fetch, so the separate `ls-remote` round trip is gone; a failed fetch fails the source instead of building from stale refs.

### TUI: idle screens stop redrawing; bounded run logs

- The TUI redraws only when something changed: a key or a resize, a message from the run, a refreshed build or picker state, or a status notice. While something on screen animates (a run in this TUI or an attached live run) it redraws at 4 Hz, and the frame after the animation stops shows its final state. An idle picker went from about 2.9% CPU to about 0.4% in a 10 s pty measurement (debug build), and from 2.5 kB to 0.25 kB of terminal output.
- The monitor's view is built once per frame instead of once for the footer and once for the body.
- Operation output borrows the run's log lines when the popup draws them; it is no longer copied on every frame.
- The picker re-reads a build dir's status and last-run files only when their modification time or size changed, and redraws only when the picker's view changed.
- The run's log text is capped at 64 MiB across all operations (5,000 lines per operation still applies). Past the cap the oldest lines of finished operations go first, then those of the largest operation. Lifecycle events are capped at the most recent 10,000; the first failed operation is still reported.

### Rust artifacts: no placeholder outputs; safer retries and batch failures

- A rust artifact whose output does not exist, with `[providers.rust] allow_nested_build = false` (the default), used to be written as a text file (`mode=placeholder`) and reported as built. It now fails with an error that names the artifact and the output and says to enable `allow_nested_build` or provide the file. An existing output is still used as it is. `examples/templates/full` names placeholder packages (`acme-api`, `cv-plugin`) and a placeholder git source, so it still needs them replaced before it builds.
- `retry_attempts` retries only failures that look transient (a signal kill, or stderr that reports a network failure). A cargo compile error fails at once instead of being retried and then rebuilt for each artifact of its batch. When a failed batch names the packages that failed to compile, only those artifacts fail; the others are built on their own.
- New opt-in `[providers.rust] sccache = true` compiles through sccache (`RUSTC_WRAPPER`) when sccache is available on the host or in the docker image.
- New opt-in `[providers.rust] strip = true` strips each collected Rust output with `<triple>-strip`, `strip` or `llvm-strip`. Cargo's own files are not touched. It is part of the recorded state.
- Cargo git mirrors are locked per mirror instead of through one lock for all of them, and a locked commit's branch is written only when it does not already name that commit.
- Recorded toolchain versions for docker builds name the image (`docker:<image id>`) instead of the host's rustc and cargo. Host versions are probed once per process instead of once per artifact.

### Project-neutral defaults and examples

- vfat filesystems take `fat_size = 12 | 16 | 32`. Unset keeps the current rule (an image from 3 MiB up to 16 MiB is FAT16, because some boot firmwares, e.g. Raspberry Pi's, do not boot FAT12; other sizes keep `mformat`'s choice), so existing configs build the same images and keep their fingerprints. `12` keeps `mformat`'s choice, `32` formats FAT32, and a set `fat_size` fails the run when `mformat` could not make that type at the image's size.
- With no build config named and no `build.toml` or other `*.toml` in the current directory, commands that need a build fail with "no build config found; pass a path or run in a directory with build.toml or another *.toml build config" instead of falling back to `examples/default-workspace/configs/default.toml`. `status`, `pause`, `resume`, `cancel` and `tui` work without one as before.
- `examples/default-workspace` and the templates are board-neutral (`qemu_aarch64_virt_defconfig`, target `generic-aarch64`, no vendor name). `examples/buildroot-raspberrypi4-go` stays as the labelled Raspberry Pi example.
- Docs, doc comments, tests and fixtures use neutral example names instead of downstream project names (and a `.update` bundle instead of `.pdupdate`); the unused `docker/aarch64` vision-stack image is removed.

### Reuse: stable fingerprints (one-time rebuild after upgrading)

- **Upgrading to this version rebuilds everything once.** Reuse state written by an earlier Gaia is read without an error, but none of it is reused: every operation executes on the first run with the reason `fingerprint_scheme_changed` ("recorded by an older Gaia fingerprint scheme"), Buildroot images included. The run after that reuses as before. Nothing needs to be deleted by hand.
- Operation fingerprints, input signatures, the spec fingerprint, and the digests providers record (source trees, path sources, artifact and image directories, Buildroot state, imported config files) were `std`'s `DefaultHasher` over `Debug` text. Rust documents that hasher as unstable between releases, so a toolchain upgrade could silently invalidate every reuse decision. They are now SHA-256 over an explicit, versioned encoding (`gaia_spec::Fingerprint`, scheme 2); structured spec values are encoded as JSON with sorted keys, and spec types derive `serde::Serialize`. The reuse state records the scheme on a `scheme=` line, and entries of another scheme are never compared or carried over.
- The fingerprint and the named components that explain a changed fingerprint (`config_overrides changed (BR2_A)`) are now built from one list of inputs, so the explanation can no longer drift from what decides reuse. Every serialized field of a source, artifact or image reaches the fingerprint, including fields added later. Options that are only encoded when set (`strip`, `include`, `allow_missing_libs`, `extra_sources`, `locked_commit`, the squashfs/EROFS and GPT options, ...) keep that behaviour, and where git runs is still not part of a source's fingerprint. The custom `Debug` impls that existed only to keep hashes stable are derived again.
- Git mirrors under `.gaia/cache/git` get new directory names. A mirror under its old name is renamed on first use (and by `gaia clean --unused-mirrors`, so it is not pruned), so nothing is cloned again.

### Buildroot images: a script edit rebuilds the images

- The inputs digest that decides whether the filesystem images and the post-image step rerun read the image settings' paths raw. A post-image script named as `$(BR2_EXTERNAL_<NAME>_PATH)/board/post-image.sh` never resolved, so its content (and its genimage config) stayed out of the digest, and editing it could keep the old images. The settings now expand `$(BR2_EXTERNAL_*_PATH)`, `$(TOPDIR)`, `$(BASE_DIR)` and `$(BR2_CONFIG)` the way Buildroot does before the file is read.

### Buildroot defconfig: support files, not the whole directory

- A defconfig's support files were copied as its whole parent directory on every run. A defconfig at a project root therefore copied the workspace, output dir included, and could overwrite Buildroot's `Makefile`, `target` or `build`. Now only the files the defconfig and its config fragments name by a relative path are copied (`-c genimage.cfg`, `overlays/config.txt`), a named directory is copied whole, nothing is copied into or over the output dir, and a copy whose size and mtime already match is kept.

### Buildroot archives: no stub file, no full-output tar

- A Buildroot archive name that is neither `.tar` nor `.txz` and does not match one expected image no longer tars the whole Buildroot output (tens of GB). It is an error that names the archive, the expected images found, and how to fix the name.
- A raw disk archive that typed assembly builds later was given a placeholder text file (`provider=…\narchive=true`) with a recorded `archive_sha256`. The path now stays absent and records no digest or size; a placeholder from an earlier build is removed.
- A compressed raw archive (`.img.xz`, `.img.zst`) is skipped when the image and the compressor and its arguments are unchanged, as tar archives already were, with the same state file.

### Buildroot collected images: reflinks and sparse copies

- Collected expected images and assembly inputs were byte-copied on every build, which lost sparseness and copied multi-GB files each time. A collected file is now reflinked (`FICLONE`) where the filesystem supports it, else copied with zero runs left as holes. Its mtime follows the source, and a copy whose size and mtime already match is not written again. Hard links are not used, because later image steps rewrite the Buildroot images in place.

### Release binary: smaller and stripped

- The workspace has a `[profile.release]` with `lto = "thin"`, `codegen-units = 1` and `strip = "symbols"`. `panic` stays `unwind`: the app turns command panics into a failed outcome with `catch_unwind`. Measured on a clean `cargo build --release -p gaia` with the same toolchain: 19.2 MB (default release, unstripped) became 10.2 MB, and the build went from 71 s to 198 s.
- `tracing-subscriber`'s `env-filter` feature (and its regex engine) is gone. `RUST_LOG` still takes the same `level` and `target=level` lists, parsed by `tracing_subscriber::filter::Targets`; a value it cannot parse falls back to `warn`, as before. Span-field directives (`target[{field}]=level`) are no longer honoured. The filter adds about 0.56 MB less than the profile alone.
- The unused workspace dependency `clap` is removed.

### Clean: caches it missed, and mirrors only on request

- `clean --target caches` now also removes shared cargo target dirs under the user cache (`<cache>/cargo-target/<key>`) whose source directories are all gone (the `gaia-owners` claims name them). A dir with no claims, or whose claimed source still exists, is kept.
- `--all-caches` also removes `.gaia/cache/import-sources` and `.gaia/docker-home`.
- Git mirrors are shared by the whole workspace, so `--target caches` no longer prunes them by default. The new `--unused-mirrors` prunes the mirrors this build does not use, counting the mirrors of `cargo_git_sources` as used. `--all-caches` still removes every mirror. Use `--unused-mirrors` only when this build is the workspace's only build.

### Assembly: less hashing, sparse disks, fewer probes

- Each published assembly output is hashed once. Its digest is computed when the output is published and used both for the step's summary and for the content manifest (it was hashed up to three times).
- A disk's partition images are copied with holes kept for zero runs, so unwritten partition space stays sparse on disk. The output bytes are unchanged.
- A tool's `--version` probe runs once per process for each program path and argument list, instead of at every step that records the tool.
- Runtime state files are written to a temporary file and renamed into place, so a reader never sees a partial file.
- Skipped file entries are recorded under `file.skipped.<entry>.*`. They used `file.<entry>.*`, which shared the numbering of staged files (`file.<count>.*`). Nothing read these keys back.
- `partition_table = "gpt"` is rejected by validation (`assembly_disk_partition_table_unsupported`), which stops the run before any assembly step; the assembly-time error remains as a backstop.

### Process: one run loop for captured and file output

- `run_command_with_timeout_and_retention` and `run_command_stdout_to_file_with_timeout_and_retention` share one run loop (timeouts, cancellation, pauses, output retention). The file variant now also logs the completion debug line the piped variant logs. Behaviour is otherwise unchanged.

### Planning: each signature and lookup once per plan

- Planning a build with a large Buildroot tree or many installs was slow: every stage item hashes every install's artifact (it depends on all of them), and each reuse decision recomputed output signatures. On a synthetic build with 40 installs, 60 stage files and a 20k-file Buildroot tree, `gaia plan` with a recorded reuse state took about 11 s; it now takes about 0.25 s. The decisions are identical (same operations reused, same reasons).
- Within one plan, output and input signatures are computed once per operation, and a plan memoizes the Buildroot tree state and each local repository's HEAD. Outside a plan nothing is memoized, so a later plan always reads the tree afresh. Fingerprint values are unchanged.
- Each config file is read and parsed once per resolution, not three times.
- `refresh = "auto"` choices (git tags or branches, GitHub releases, JSON) are fetched once per minute within the process, so variants and validate-then-plan share one fetch. The on-disk cache TTL is unchanged. A choice added within that minute appears after it.
- Floating git refs resolved for a plan are reused for 60 seconds in the same process instead of being cleared when the plan ends. `GAIA_FLOATING_REF_TTL_SECONDS` changes the window (`0` resolves every plan, as before). A branch that moves within the window is seen once it expires.
- Long-running processes (the TUI) no longer keep stale values: `${project.commit}`, `${project.describe}` and `${build.date}` follow new commits and branch moves at once (the `-dirty` suffix can lag by up to 10 seconds), and the tool-version and docker image signatures are re-probed after 60 seconds.

### CLI: `--json` everywhere scripts need it

- `--json` now applies to `run`, `plan`, `status`, `validate` and `cache` as well as `preview`. Each prints one JSON document on stdout (progress and logs stay on stderr), with `schema_version` (1), `command`, `exit_code` and `status` in every document, and an `error` field when the command fails. `run --json` carries the run summary as written to `<build>.summary.json` (including `sizes`, `operation_timings` and the new `critical_path`), the report files, diagnostics, execution errors and the export/resume lines; `plan --json` the operations with their dependencies and reasons and the estimate; `validate --json` the diagnostics; `status --json` the runs with their live status snapshot; `cache --json` the entries, removals or stats. `--follow` does not combine with `--json`. Without the flag the output is unchanged. The schema is documented in docs/cli.md.

### `gaia cache stats`

- Lists every cache Gaia keeps for a build with its path, disk usage and file count: git mirrors, downloads, import-sources, the Buildroot download dir, package cache levels and shared output trees, ccache, `docker-cache`, `docker-home`, the shared `cargo-target/<key>` dirs, the Gradle and Java homes and the user-wide Buildroot download, package and compiler caches under the user cache root, then the total. Usage is counted like `du`: allocated blocks, hard links once (by device and inode, also across caches), symlinks never followed, a few threads walking each tree. `--json` gives the same with apparent sizes. Without a build config that resolves, it reports the user-wide caches and the current directory's workspace caches.

### Run profile: critical path and Chrome trace

- After a run, the summary prints the actual critical path from the recorded start and finish times and the plan's dependencies, e.g. `critical path 9m40s: source:eidos 5s → artifact:helios-api 3m18s → image:build 4m12s → image:assembly 58s`. It is the chain ending at the operation that finished last, each step back taking the dependency that finished last; reused operations are passed through. The summary report has it as `critical_path`, and each operation timing gains `start_ms`.
- Each run writes `<build>.trace.json` beside the reports: Chrome trace events, loadable in `chrome://tracing` or Perfetto, with one slice per operation on non-overlapping rows (status, duration, pause and critical-path flag in its args) and the assembly steps as slices inside the assembly operation.

### CI output: log groups per operation

- With `--ci`, or when `CI` is set (as GitHub Actions and most CI services do), `gaia run` prints no progress lines. Each operation's output is kept until it finishes and printed as one GitHub Actions log group (`::group::<operation>: done in 1m20s` ... `::endgroup::`), so parallel operations never interleave; a failure is also reported as an `::error::` annotation. Output that looks like a workflow command is wrapped in `::stop-commands::`. The last 5,000 lines of each operation are kept.

### Image assembly: squashfs and EROFS filesystems

- New filesystem kinds `squashfs` (`mksquashfs`) and `erofs` (`mkfs.erofs`), resolved from provider host tools, then `PATH`. Options: `compression` (squashfs `gzip`/`xz`/`zstd`/`lz4`, default `gzip`; erofs `lz4`/`lz4hc`/`lzma`/`deflate`/`none`, default `lz4hc`), `compression_level` where the compressor takes one, `block_size`, and for erofs `pcluster_size`, `dedupe`, `fragments`, `ztailpacking`, `label` and `uuid`. Validation checks each against the kind and compressor.
- Both are reproducible: every entry root-owned with epoch times, no host xattrs, and the erofs UUID derived from the filesystem id. The image is as large as its content, works as a `verity` source and a partition image, and is reported and published like the other filesystems.

### Image assembly: GPT disks

- `partition_table = "gpt"` is implemented (it failed at assembly time as not implemented): a protective MBR, the primary header and 128 entries at LBA 1-33, and the backup entries and header at the end, with their CRC32s. Partitions are aligned as on MBR disks. `type_alias` takes GPT names (`linux`, `esp`, `bios-boot`, `xbootldr`, the Discoverable Partitions root, `/usr` and verity types such as `linux-root-arm64`, and more) and `type` a raw type GUID; the partition `name` is the GPT name, and `bootable` sets the legacy BIOS bootable attribute.
- The disk GUID (`disk_guid`) and partition GUIDs (`guid`) can be set; unset they are derived from the disk id and partition names, so the same config gives byte-identical disks and stable `PARTUUID`s. MBR-only options (`signature`, `signature_text`, `ebr_placement = "packed"`, `truncate`) are rejected on GPT disks, and GPT options on MBR disks.
- Fingerprints of existing filesystems and disks are unchanged: the new options are part of them only when set.

### Fixes: tar path checks, interpolation cycles, override keys, rootfs symlinks

- Tar archives are checked entry by entry. The validation read only the last 1,000 lines of the listing, so in a larger archive an earlier `..`, absolute, or hardlink entry was never checked. The listing is now streamed to a scratch file beside the archive and every line is validated.
- An interpolation cycle (`build_name = "x-${build.display_name}"` with `display_name = "${build.name}"`, or a `${interpolation.values.x}` that refers to itself) overflowed the stack and aborted gaia. It is now a config error naming the cycle: `interpolation cycle: build.display_name -> build.name -> build.display_name`.
- A `--set` key, or a preset override, that names no setting was silently ignored (`--set image.rootfs_pth=/x`). It is now an error with a `did you mean` hint. `--set input.<name>` for an input the build does not declare is also an error; it was ignored.
- The starting-point rootfs copy followed symlinks, so an absolute link in the rootfs (for example `etc/resolv.conf` to the host's `stub-resolv.conf`) copied host files into the image, and modes were lost. The copy now recreates symlinks without following them, keeps file and directory modes and in-tree hardlinks, and recreates device nodes and fifos only as root (otherwise it fails naming the file). Ownership is still not carried over.

### Buildroot EROFS: lzma without the extra image

- With `BR2_TARGET_ROOTFS_EROFS_LZMA=y`, Buildroot's generic image compression (keyed on the same symbol) also ran `lzma -9` over `rootfs.erofs`, writing a second image larger than the first and costing about 35 s per build. The settings step now rewrites that choice to `custom` with `BR2_TARGET_ROOTFS_EROFS_COMPRESSION_ALGORITHMS="lzma,<level>"`, which gives `mkfs.erofs` the same `-zlzma,<level>` argument, and reports it. Other filesystems are unchanged. The `config_overrides` check accepts that rewrite: a requested `BR2_TARGET_ROOTFS_EROFS_LZMA=y` and its `_LZMA_LEVEL` count as honored when the final `.config` has the custom choice with the same `lzma,<level>` (517a311 reported them as dropped and failed the build).

### Reuse: floating git refs resolved at plan time

- A git source that tracks a floating branch or `HEAD` (`refresh = "auto"`, `pin = "floating"`) was re-materialized on every run, and so were the artifacts built from it (about 3 minutes each for HeliOS), even when the remote's branch had not moved. Planning now resolves the ref with `git ls-remote` (once per ref per plan, in parallel, 30 second limit) and compares it with the commit the last materialization recorded. An unchanged ref keeps the source and its dependents, and the plan reports `reused from floating ref 'main' still at <short commit>`. A moved ref materializes the source and rebuilds its dependents as before. An unresolvable ref (offline, unreachable, missing branch) still materializes, with the reason in the plan.
- The resolved commit is part of a floating source's fingerprint. A build upgraded to this version materializes each such source once.

### Content digests: no fsync per write

- Writing the `.gaia-content-digests` manifest no longer calls `fsync`. Every assembly output published (a `compile-dts` `.dtbo`, a transform's output, the files of an assembly step) rewrote the manifest and waited for the device to flush everything written so far, which took 0.1 to 0.5 s on a loaded disk and could take seconds on a busy one, while `dtc` itself ran in milliseconds. The manifest is still written to a temporary file and renamed into place, so a crash leaves the old or the new manifest, and a lost entry only costs one re-hash.

### Inputs: tokens in input values

- An input's value (its declared `default`, or a `--set input.<name>=` value) is interpolated like the rest of the build, so `default = "${project.commit_short}"` or `"${build.date}"` resolves instead of being reported as `interpolation_unresolved [input.<name>]`. A build file in a linked git worktree gets that worktree's `HEAD` for `${project.commit}`.

### Buildroot compiler cache: default size from free space

- When `[providers.buildroot.ccache] max_size` is unset, the size is computed at run start as `clamp(free / 2 + current size, 2 GiB, 40 GiB)`, the same rule as the package cache, with `free` the free space on the cache directory's filesystem (a missing directory holds nothing yet) and `current size` what it already holds. It was a fixed `50G`. The size is written to the cache's `ccache.conf` at run time and logged; it is not in `.config` or the config fingerprint, so a change in free space rebuilds nothing.
- The "may grow to ... but its filesystem has ... free" warning fires only for an explicit `max_size` larger than the free space; the default never warns.

### Buildroot package cache: default size from free space

- When `[providers.buildroot.package_cache] max_size` is unset, each level's budget is computed at run start as `clamp(free / 2 + current size, 2 GiB, 40 GiB)`, with `free` the free space on the level's filesystem and `current size` what the level already holds. It was a fixed `100G`, which warned on a disk with less than that free. Without a free-space reading the budget is 40 GiB. Cache keys are unchanged.
- The "may grow to ... but its filesystem has ... free" warning fires only for an explicit `max_size` larger than the free space; the default never warns.
- Eviction is least recently used first (manifest modification time, which restores and stores refresh) and now also runs at the start of a run, before packages are restored, so a lowered `max_size` takes effect. Each eviction is reported in the run output with its count and size.

### Assembly: lean trees (strip, include lists, module metadata, cpio-xz)

- `strip = true` on `[[image.assembly.binaries]]` (the program and every library of its closure), on `[[image.assembly.files]]` (each copied ELF) and on a tree (every ELF in it, just before a filesystem or archive packs it). Only the copies in the tree are stripped. Executables and shared libraries get `--strip-unneeded`; kernel modules and relocatable objects get `--strip-debug`; non-ELF files, symlinks and already-stripped files are left alone; hard links stay linked. The tool is `[image.assembly] strip_tool`, else the provider's `<triplet>-strip`, else `llvm-strip`, else `strip` for host-machine ELFs; if none is found the build fails, naming the file. The bytes saved are recorded per tree.
- `include = [...]` on a `src_glob` files entry copies only the matches named there (file names, simple `*` globs). A name that matches nothing is an error unless `optional = true`.
- `metadata = ["modules.dep", ...]` on a kernel-modules entry installs only the listed `modules.*` files and removes the other indexes depmod writes. Unset keeps today's behaviour.
- A new filesystem kind `cpio-xz`: xz with `--check=crc32` (the only check the kernel decoder takes besides none), deterministic, with an optional `compression_level` 0-9. Its LZMA2 dictionary is capped at 1 MiB (or the cpio size, when smaller) whatever the level, since the kernel decoder allocates it whole: a `-9` initramfs with a 64 MiB dictionary did not boot. It is written by `xz -T2` with xz's own block size, so the block header carries the sizes in the padded form that booted; a stream without sizes (`-T1`), or with `--block-size` set to the cpio size, did not.
- The `gzip` transform takes `level` 1-9 and writes no name or mtime (`gzip -n`), so its output is byte-identical across runs.
- None of the new options changes the fingerprint of a config that does not set it.

### Run summary: image output sizes and their change

- Every `gaia run` prints an `image sizes` table with the file size of each image output it published (archives, disks including `.img.xz`, filesystem images such as `rootfs.erofs` and `initramfs.cpio.zst`, and transform outputs such as a verity hash or a kernel image), sorted by kind then name. Each row shows the change since the previous successful run of the same build and variant: `new`, `=`, `(+1.3 MiB)`, `(-12.0 KiB)` or `removed (was ...)`. It replaces the `image output:` and `primary image size:` lines; `primary image output:` and its `sha256` line stay.
- The summary has the same list as `sizes` (`kind`, `name`, `path`, `bytes`, `change`). It is in the post-build hook's JSON payload too, and the run dashboard's summary and details panels show it in place of the raw and content sizes.
- The baseline is `<build>.sizes.json` beside the reports. It is written only by a clean run that has image outputs, so a failed or cancelled run leaves the comparison with the last good run in place. A reused image operation still reports its outputs, carried forward from that baseline (or the previous `summary.json` before one exists).
- The sizes also cover every filesystem and transform output the assembly spec names (kinds `filesystem` and `transform`, named by path relative to the assembly out dir), read from the spec's own path templates, so a reused assembly still reports them. Kinds come from the spec where it names the output; a kernel `.img` written by a transform is a `transform`, not a `disk`. Bytes used inside a vfat image are not reported.
- An assembly output's size is recorded as its step writes it, so a filesystem built in a RAM work dir (such as the slot initramfs, `cpio-xz`, or `rootfs.erofs`) is listed with its size instead of being missing once the RAM copies are removed. A reused assembly reports the sizes its state file recorded (`image-assembly.state`), still with their change.

### Assembly: copied dynamic files warn about missing libraries

- A `[[image.assembly.files]]` entry that copies a dynamically linked file (one that names a program interpreter or `DT_NEEDED` libraries) is checked once its tree is complete, before a filesystem or archive packs it. Each interpreter and library the file needs must be in the tree, as `[[image.assembly.binaries]]` would bring them. A missing one is a warning in the run output and the summary, naming the file, the tree and the libraries. It is not an error, since a tree may overlay a full rootfs.
- `allow_missing_libs = true` on a file entry accepts it without the warning. It is refused on generated `content` and `json` entries. Static and non-ELF files are never checked.
- A missing program interpreter is reported with the other missing libraries instead of failing the lookup first.

### Image outputs: the disk of a raw archive, xz transforms, published outputs

- `[image.output] disk = "<id>"` picks the assembly disk a raw disk archive (`archive_name` as `.img`/`.raw`, optionally `.xz` or `.zst`) is built from. It is required when the assembly builds several disks; the error names the option and the disks.
- A new assembly transform `kind = "xz"` compresses a file with `xz -T0 --block-size=24MiB -c`, with an optional `level` 0-9.
- The outputs a run writes under the image output dir are published: transform, verity and archive outputs there, and disks written there. They are listed in the run summary (`published_outputs`, `image output:` lines) and `gaia run --export` (or `export_dir`) copies them next to the primary. An uncompressed raw disk other than the primary is listed but not copied.

Changes on `main` since 2.0.0. The workspace version stays `2.0.0` until the
next release; builds from `main` (some of which briefly reported 2.1.0 to
2.5.0) are identified by commit. Build files should not require unreleased
versions through `gaia_version`: a binary that lacks a setting now fails
validation on the unknown key.

This cycle also moves the toolchain pin and `rust-version` to Rust 1.99.0
(installing Gaia needs Rust 1.99 or newer), upgrades every dependency to its
newest release, and updates the Rust docker images to 1.99.0.

### Reuse follows the content of the image feed

- A staged unit or file is fingerprinted by the bytes it resolves to (through
  `@assets/...` as the executor resolves it), not its mtime, and its stage
  operation's content signature includes those bytes. Editing a unit file now
  reruns its stage operation and `image:build`.
- An install's content signature includes the bytes of the artifact it
  delivers, so a rebuilt artifact with new bytes reruns `image:build` instead of
  being cut off by the early cutoff.
- `--rebuild image:prepare` on a build without a prepare step means
  `--rebuild image:build` (see docs/cli.md).

### Source tokens in the assembly, tree digest maps

- `${source.<id>.commit}`, `${source.<id>.commit_short}` and `${source.<id>.path}` resolve in every string of `[image.assembly]`: tree ids and paths, file `src`, `dest` and `content`, `json` values, archive members and outputs, disk and filesystem outputs, and the rest. A slot id can carry a source's commit. An unknown source id, or one with no commit yet, is still reported as an unresolved token. The assembly depends on each declared source it names, so a changed file inside a path source reruns it.
- `${assembly.sha256_tree:<tree id or path template>}`, as a whole JSON value, renders as an object mapping each regular file under the tree to its sha256, sorted by path. It is read after the steps that fill the tree, and its fingerprint covers the tree's content, so a rewrite with the same bytes keeps the assembly reused. In text, or with a file naming its own tree, it is a validation error.

### TUI screens: start, setup, details

- The start screen shows the builds on the system as cards (progress, elapsed time and estimate for running ones; outcome, duration and age for recent ones) and each build config with its target, profile and last run.
- Setup is a form with validation badges beside a plan preview: operations to run and reuse, why they run, recorded time, validation, and a Start button that says why it cannot start. A build that declares `[[variants]]` gets a variant value; the variant is planned and run.
- A details screen (`d`) with Plan, Validation, Reports, Spec and Events tabs replaces the old side panels.
- Every screen has the same header and footer; `?` lists the keys of the screen.
- The run dashboard's chart tabs are always five (Summary last), so the footer's `1-5` is right on every run. The Progress chart draws the last run as dots beside this run's braille line, with a marker for now.

### Assembly archive trees, auto-size headroom and layer kind

- Archive members can take a whole tree: `tree = "<tree id>"` or `src_dir = "<path>"` archives every entry under it (regular files with mode 0644 or 0755, directories and symbolic links) under `prefix`, in sorted order and deterministic bytes. `src_glob` on a member archives each match under `name`. `exclude = [...]` on a `tree` or `src_glob` archive member, and on a file entry's `src_glob`, leaves out matches by file name.
- `free = "256M"` on a vfat or ext4 filesystem with `size = "auto"` leaves that much room beyond the content estimate.
- `${source.<id>.commit_short}` and `${project.commit_short}`: the first 8 characters of the commit, with a `-dirty` suffix kept.
- The image `kind` may be left out of a layer that is extended by a build file which sets it; a build that has an `[image]` table and in which no file sets `kind` fails with an error naming `image.kind`.
- Path sources inside a git work tree compute their identity from `git ls-files` (tracked, plus untracked and not ignored), so ignored build output is no longer read. Their identity changes once, which can cause one rebuild.

### Run dashboard for the TUI monitor and attached runs

- The TUI's monitor (a run it started) and attached view (a `gaia run` another process started) share one run dashboard: a header bar with the state, variant, elapsed time and ETA; an overall progress gauge weighted by the recorded duration of each operation, with the Buildroot packages of a running make; an operations table with each operation's state, time, expected time, delta and reason; chart tabs (Progress: this run against the last one; Resources: CPU, memory, tmpfs, load and disk; Steps; Packages; and a completion Summary); and an output pane with follow, per-operation filter, search and a height control. `Enter` opens an operation's details, `?` lists the keys, `f`/`s` filter and sort, `p` pauses or resumes, and `c` then `c` cancels.
- The live status file (`.gaia-run.status.json`) carries what the dashboard needs: every planned operation with its state, timing, expected duration and reason, the variant, the step times and the errors. The new fields are optional on read, so snapshots from older Gaia versions still load, and older readers ignore them.
- The dashboard lays out for 40x8 and up: the wide layout (charts beside the table) from 110 columns; narrower terminals show the table, and `v` switches to the charts.

### Generated files, JSON documents and listed binaries for image assembly

- New file entry sources `content = "..."` (text) and `json = { ... }` (a JSON document) write a generated file into a tree, in place of `src`/`src_glob`. Archive members and generated members take `json` in place of `entries`/`src`. `json` tables keep their key order, render numbers for numeric tokens, and write pretty-printed output with a trailing newline, byte-identical across runs.
- Generated text and JSON render `${assembly...}` tokens at assembly time: `${assembly.sha256:...}`, `${assembly.verity:...:<field>}`, and the new `${assembly.size:<path>}` (size in bytes), in archive entries too. A generated file runs after the steps whose outputs it reads, and works in RAM work dirs. A token that reads an assembly path no step writes is a validation error.
- New `${build.date}`: the build's time as RFC 3339 UTC, from `SOURCE_DATE_EPOCH` or the build file's git `HEAD` commit time.
- New `[[image.assembly.binaries]]` copies a binary into a tree with its runtime closure (interpreter and libraries from the sysroot), reusing the BusyBox closure copy. A static binary is only copied. Reuse fingerprints cover the binary and its closure.
- Docs: "Generated content" and "Listed binaries" in `docs/configuration.md`, with a dynamic slot example combining verity, a JSON manifest, a generated cmdline and a binary.
- The `toml` crate's `preserve_order` feature is enabled for the config crate, so JSON tables keep their written key order.

### Rust artifacts build private cargo git dependencies from git sources

- New artifact key `cargo_git_sources` (rust only) maps a cargo git dependency to a declared `kind = "git"` source: `cargo_git_sources = { "https://github.com/Prometheus-Dynamics/Eidos.git" = "eidos" }`. The key is the repository URL as `Cargo.lock` records it. The artifact depends on the source, and the source's id must be declared and of kind git.
- Before cargo runs, Gaia makes each commit the lock pins for that repository present in the source's mirror under `.gaia/cache/git` (fetching it on the host with your git credentials), keeps it reachable from a branch of the mirror, and runs cargo with `CARGO_NET_GIT_FETCH_WITH_CLI=true` and `url.<mirror>.insteadOf` rewriting. The container gets no credentials: the mirror is mounted read-only. A locked commit the source does not have fails the build with the URL, the commit and the source id.
- The mapping is part of the artifact's fingerprint only when set, so artifacts without it keep their fingerprints. `Cargo.toml` and `Cargo.lock` are unchanged. Docs: "Private cargo git dependencies" in `docs/configuration.md`.

### dm-verity transform and ext4 filesystem for image assembly

- New transform kind `verity` builds a dm-verity hash tree over a data image (`src`, such as an EROFS root filesystem) with `veritysetup format`. The tree is appended to the padded data in `dest`, or written to `hash_dest`. Options: `data_block_size` and `hash_block_size` (powers of two, 512 to 4096, default 4096), `salt` (default: the sha256 of `src`), `uuid` (default: derived from `src`). Two runs give byte-identical output and the same root hash.
- A verity transform's layout is recorded in the assembly state under `verity.<n>.*`: root hash, salt, UUID, hash offset, data blocks and block sizes.
- Archive entries read those values with `${assembly.verity:<dest>:<field>}` (`root_hash`, `salt`, `hash_offset`, `data_blocks`, `data_block_size`, `hash_block_size`). The path is the transform's `dest` as written; other names and fields are validation errors.
- New filesystem kind `ext4` packs a tree into an ext4 image with `mkfs.ext4 -d`. Options: `size` (required, a byte size or `"auto"`), `label` (at most 16 bytes), `uuid`, `journal` (default on). Every entry is root:root with timestamps of 0, and two runs over the same tree are byte-identical. `publish` and RAM work dirs apply as for the other filesystems.
- Validation covers the new options (rejected on the kinds that do not take them) and the verity tokens. Docs: "dm-verity and ext4" in `docs/configuration.md`, with a dynamic slot bundle example and an ext4 data partition example.
- Both tools are host tools resolved like the other assembly tools (provider host tools first, then `PATH`): `veritysetup` for `verity`, `mkfs.ext4` and `debugfs` for `ext4`. The build fails with a clear error when one is missing.

### Artifacts rebuild when a source they read changes

- An artifact whose Java build fields name another source through `${source.<id>.path}` or `${source.<id>.commit}` (for example `build_env = [["EIDOS_PATH", "${source.eidos.path}"]]`) now depends on that source. Before, a changed source re-materialized but the artifact was reused, shipping a stale jar.
- New artifact key `sources = ["eidos"]` names sources an artifact reads besides its own `source`; an undeclared id is a validation error.
- The rebuild reason names the source (`source:eidos runs ...`). An artifact still skips its rebuild when the source's files hash the same after a rev bump.
- Artifacts that name extra sources get a new fingerprint, so each rebuilds once after upgrading. Artifacts without them keep their fingerprints.

### Git sources use host git and credentials

- A `kind = "git"` source runs its git commands (mirror, fetch, `ls-remote`, clone, checkout) on the host with the user's environment, so private repositories work when the build runs in docker. Builds still run in docker. `git_backend = "execution"` restores the old behaviour per source; without host git the source falls back to the execution backend with a note. See "Git source credentials" in `docs/configuration.md`.
- Git runs with `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never`: a missing credential fails with the source id and a hint instead of prompting.
- URL credentials (`https://user:token@host/...`) are redacted to `https://***@host/...` in logs, errors, source state files, reports and `<build>.gaia.lock`. Lockfile entries keep their match with the configured URL.
- Source fingerprints do not change, so builds upgraded from 2.0.0 keep their materialized sources.

### Conditional layers keyed on inputs

- A `when` table takes `input = { name = "value" }`: every named input must have that value (compared as a string; booleans and integers as written). It applies to imports, artifacts, installs and stage items. An undeclared input name is an error.
- Import conditions see the selected inputs: declared defaults, the preset's `input` overrides, `--set`, and a variant's overrides and presets. Each variant resolves its own imports, so `[[variants]]` can pick different layers per input value.
- `[[variants]]` accepts `preset`/`presets`; see "Variants" below.
- `--set sources.<id>.path=DIR` names a source that an input-gated import declares. It is checked against the config the resolution selects after presets, variants and conditional imports, not the base file alone. A variant applies it to the variants that declare the source; it is an error only when no selected variant (or the base, when selected) declares it, and the message lists the declared sources.

### Docker never creates mount sources

- A command argument like `BR2_ROOTFS_POST_BUILD_SCRIPT=/x/exclude.sh
  board/post-build.sh` was taken as one path, and docker created the
  missing "directory" `/x/exclude.sh board/...` as root inside the build
  tree; the next variant's image:build could not remove it ("Permission
  denied"). Values are split on whitespace, each absolute path is mounted
  only when it (or its parent) exists, and no missing mount source is ever
  passed to docker. A stale directory holding files another user owns is
  moved into `.gaia-trash` instead of failing the build.

### Generated JSON text is checked

- Generated `content` written to a `.json` destination must parse as JSON
  once its tokens are filled in. A TOML `'...'` literal string keeps a
  backslash-n as two characters and once shipped an invalid slot
  `state.json`; the error now says so and suggests `"..."`, `'''` or
  `json = { ... }`.

### Assembly tools from the provider's host/sbin

- Assembly tools are looked up in the provider's host `bin`, `sbin`,
  `usr/bin` and `usr/sbin` (then PATH). Buildroot installs veritysetup,
  mkfs.ext4, debugfs and depmod into `host/sbin`, so a slot build failed
  with "assembly tool 'veritysetup' was not found" although
  host-cryptsetup was built.

### Build variants

- A build file can declare `[[variants]]`: each is an `id` and `overrides` in the `--set` form. One `gaia run` then builds the base config (variant `default`) and each variant, in order, from one shared base. `gaia run --variant pv2026[,...]` builds a subset; `preview` and `plan` show each variant. See "Variants" in `docs/configuration.md`.
- Operations that are the same in every variant (sources, the Buildroot base, unchanged artifacts) run once and are recorded in the shared state `<build>.reuse-state`. Variant-specific operations write to `variants/<id>/` paths, keep their state in `<build>@<id>.reuse-state` and their install and stage records in `.gaia/runtime@<id>`, and report as `<build>@<id>.*`.
- A source a variant checks out differently (for example another `rev` through an input) is checked out per variant under `build/sources/variants/<id>/<source>`; `${source.<id>.path}` and `${source.<id>.commit}` name the variant's own checkout. The shared git mirror stays shared. A source the Buildroot base or the image reads, or a differing Buildroot base, is an error.
- `[[variants]]` takes `preset = "name"` or `presets = [..]`: the variant applies those presets in place of the command line's `--preset`, then its own overrides.
- Errors: a variant id that is malformed, reserved (`default`) or repeated, an override key Gaia does not know, a preset the build does not define, and two variants that would publish the same image file.
- A build with variants always plans image:prepare, so the Buildroot base
  is one operation shared by every variant (before, variants without an
  artifact building against the prepared sysroot had none, and the base
  could not be shared). image:prepare's fingerprint no longer includes the
  image feed (prepare applies none), nor image naming or rootfs exclusions.
  A refused image:prepare says which variant differs in what.
- `gaia validate` validates every variant (or those `--variant` names) as
  `run` resolves them, each diagnostic prefixed `variant <id>:`, plus the
  checks across variants (shared operations, sources the base needs,
  colliding outputs). `preview`/`run` print the variants' validation errors
  instead of only counting them.

- `${source.<id>.path}` and `${source.<id>.commit}` are also substituted in Java artifacts' `build_target`, `build_args`, `build_command` and `build_env`; with variants they name the variant's own checkout of a source it checks out for itself, or the shared one.
- `exclude_packages` reruns the host `depmod` for each kernel version that lost a kernel module (Buildroot indexes modules in target-finalize, before the exclusion), so leaving out an out-of-tree module package keeps `modules.dep` exact. With it, a variant matrix can build such a module once in the shared base and leave it out of the variants that must not ship it.
### Buildroot images can leave packages out of the rootfs

- `[image.rootfs] exclude_packages` (kind `buildroot`; `--set image.rootfs.exclude_packages=a,b`) removes the files of the listed Buildroot packages from `target/` during `image:build`, before the post-build scripts and the filesystem images. One Buildroot tree serves every image; the excluded files are copied back into `target/` on each finalize of an image without the exclusion.
- Checks before the removal: unknown names, `host-*` packages, and kept target packages that depend on an excluded one (the error names the chain). Checks after the make and before the images: no object left in `target/` needs a shared object name that only a removed file provides (ELF `DT_NEEDED`/`DT_SONAME`).
- Refused without per-package directories (`parallel_packages`) and with shared Buildroot output. `image:prepare` does not apply it, and its fingerprint ignores the list. `gaia preview` shows the names and the checks. See "Leaving packages out of the rootfs" in `docs/configuration.md`.

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

### Globs into one file are refused

- An `[[image.assembly.files]]` entry whose `src_glob` matched several files
  and whose `dest` had no trailing `/` copied every match onto that one file
  (386 overlays became one file called `overlays`). It now fails and says
  to end `dest` with `/`; a single match can still be renamed.

### Small vfat images are FAT16

- `mformat` formats images under 16 MiB as FAT12, which the Raspberry Pi
  firmware did not boot a USB-boot `boot.img` from (the vendor image and
  every Raze boot partition are FAT16). vfat images from 3 MiB up to
  16 MiB are now formatted with one sector per cluster, which makes them
  FAT16; smaller and larger images keep `mformat`'s choice.

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
- Fixed image assembly running steps in a fixed kind order regardless of what they read: a transform reading a filesystem the same assembly builds (for example compressing `$provider.images/boot.vfat` for an update bundle) compressed the previous run's image, so the bundle carried a stale kernel. Steps now run after every step whose output they read (same file, a file in a directory it fills, or a glob it matches); independent steps keep the classic order. Dependency cycles are rejected at validation and run time, outputs of transforms, filesystems, disks and archives from an earlier run are removed before the steps run. A step may read what a later-declared step produces (it runs after it); validation reports only dependency cycles.
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
