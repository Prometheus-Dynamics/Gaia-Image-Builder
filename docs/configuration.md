# Configuration

Gaia resolves one build file into one `ResolvedBuildSpec`. The raw TOML model is intentionally close to the canonical spec, but not identical. This document describes the TOML surface you can actually write today.

## File Loading

Gaia accepts:
- an explicit file path
- or a logical build name that resolves through repo-relative fallback locations configured by the app; in this repo the default fixture lives under `examples/default-workspace/configs/`

Config files can use:
- `extends = "base.toml"` for one base file
- `imports = ["a.toml", "b.toml"]` for additive fragments
- table imports with `when` for conditional fragments, for example
  `{ path = "full.toml", when = { profile = "full" } }`

Merging rules:
- later imports override earlier ones
- vectors of typed objects merge by id/key where supported
- free-form override pairs stay user-controlled
- conditional imports are selected from top-level build metadata, including
  `build.target`, `build.profile`, and `build.branch` overrides

### Imports From a Git Source

A layer can live in another repository and be imported from a git source,
for example shared device support kept next to the hardware it supports:

```toml
# in a local config file (the build entrypoint or a layer it imports by path)
imports = [
  "../layers/base-os.toml",
  { source = "atlas", path = "devices/raze/gaia/device.toml", when = { target = "cm5" } },
]

[[sources]]
id = "atlas"
kind = "git"
repo = "https://github.com/Prometheus-Dynamics/Atlas-Hardware-Manager"
rev = "<full commit sha>"   # or an entry in the build's lockfile
```

- `source = "<id>"` reads `path` (relative to the repository root) from that
  source's checkout while the config is resolved, so `resolve`, `validate` and
  `plan` see the imported layer like any local one.
- The source must be a `kind = "git"` entry of `[[sources]]` in a local file:
  the entrypoint, its `extends` chain, or a layer imported by plain path. Files
  read from a source cannot declare import sources.
- It must be pinned: either `rev`, or an entry in `<build>.gaia.lock`. Running
  `gaia lock <build>` records the current commit of an unpinned import source
  (see [Git Source Lockfile](#git-source-lockfile)). Otherwise resolution fails
  with an error naming the importing file and the source id.
- The checkout is made in `<workspace>/.gaia/cache/import-sources/<id>-<rev>`,
  through the shared per-repository mirror in `.gaia/cache/git` for remote
  repositories. An existing checkout is reused without fetching, so use full
  commit shas for `rev`.
- `when` behaves exactly as for local imports. An import whose `when` does not
  match is not fetched.
- Imports and `extends` inside a source-imported file resolve relative to that
  file, like local imports, and must stay inside the checkout: `..` or symlink
  escapes are rejected.
- The source stays a normal build source with the same id, materialized at
  the same commit.

Path tokens, rewritten when each file is loaded (only at the start of a
string value):
- `@self` / `@self/<rest>`: absolute directory of the file containing the
  value. This is the same for local files and source-imported files, so a
  layer written with `@self/overlays/raze.dtbo` works from a checkout and from a
  vendored copy imported by plain path.
- `@source:<id>` / `@source:<id>/<rest>`: the checkout directory of import
  source `<id>`, for consumers referencing files in the source.

Relative paths elsewhere still resolve against the workspace root, so layers
meant to be imported from a source should reference their own files with
`@self`.

Local development: `--set sources.<id>.path=<dir>` (absolute, or relative to
the workspace root) reads imports and `@source:<id>` from `<dir>` without
fetching or needing a pin, and the source materializes from `<dir>` as a
`kind = "path"` source with the same id. Only `--set` overrides are honored for
import resolution, not preset overrides. Do not run `gaia lock` with such an
override: the source is then a path source and its lock entry is dropped.

Reuse: the resolved import sources are recorded in the spec
(`selection.import_sources`) with their identity: `git:<repo>@<commit>`, or for
a path override, `path:<dir>#<digest of the imported config files>`. Items
declared by source-imported files (sources, artifacts, installs, stage files,
env sets, services, and the image when such a file sets `[image]` fields)
fold that identity into their operation fingerprints. Changing `rev` therefore
rebuilds what came from the layer, even when the resolved values are
identical. Items declared only locally keep their fingerprints. Trade-off: the
attribution is by id, so a local item overriding an id the layer also declares
is rebuilt too; and values a layer contributes without an id (env, inputs,
policy) are covered only through the spec parts they change.

## Top-Level Build Fields

Supported top-level fields:

```toml
build_name = "helios-cm5"
display_name = "HeliOS CM5"
version = "v2026.2.0"
description = "Release image for Raspberry Pi CM5."
branch = "main"
target = "cm5"
profile = "release"
labels = [
  ["stack", "helios"],
  ["board", "cm5"],
]
```

Meaning:
- `build_name`
  Stable canonical identity used for report file naming and persisted state names.
- `display_name`
  Human-facing label.
- `version`
  Optional build version string.
- `description`
  Optional descriptive text.
- `branch`
  Top-level branch metadata. This is also propagated into provider state.
- `target`
  Top-level target metadata. This is also propagated into provider state.
- `profile`
  Top-level profile metadata. This is also propagated into provider state.
- `labels`
  Free-form metadata pairs.

## Product Metadata

```toml
[product]
family = "helios"
name = "vision-system"
sku = "helios-cm5-release"
```

## Inputs

Inputs are declared at the top level, then selected through defaults, presets, or CLI `--set input.<name>=...`.

```toml
[inputs.target]
description = "Hardware target identifier"
kind = "string"
default = "cm5"

[inputs.profile]
description = "Profile selector"
kind = "enum"
default = "release"
choices = ["dev", "ci", "release"]

[inputs.release_ref]
description = "Release tag from the application repository"
kind = "enum"
default_from = "latest-stable"

[inputs.release_ref.choices_from]
kind = "git-tags"
repo = "https://github.com/example/app.git"
include = ["v2026*"]
exclude = ["*-alpha*", "*-beta*", "*-rc*"]
sort = "version-desc"
version_scheme = "semver"
prefer_stable = true
limit = 25
cache_ttl_seconds = 3600
fallback_choices = ["v2026.1.0"]

[inputs.enable_debug]
description = "Turn on debug-only behaviors"
kind = "boolean"
default = "false"

[inputs.build_number]
description = "External CI build number"
kind = "integer"
required = true
```

Kinds:
- `string`
- `integer`
- `boolean`
- `enum`

Dynamic choices:
- `choices_from.kind`: `git-tags`, `git-branches`, `github-releases`, `json`, or `command`.
- `choices_from.repo`: git remote URL or local repository path.
- `choices_from.source`: optional id of a declared git `[[sources]]` entry; use this instead of repeating `repo`.
- `choices_from.url`: URL or local path for `json` choices.
- `choices_from.json_path`: simple JSON path such as `$.releases[*].tag`.
- `choices_from.command`: argv array for command-produced choices; stdout lines become choices.
- `choices_from.pattern`: single `git ls-remote` ref glob passed to git.
- `choices_from.include`: list of glob patterns Gaia applies to returned names.
- `choices_from.exclude`: list of glob patterns to remove from returned names.
- `choices_from.strip_prefix`: remove a prefix from displayed/selected choices after filtering.
- `choices_from.selected_value_template`: transform selected values with `${choice}`.
- `choices_from.display_template`: reserved for richer TUI display labels; current resolved choices remain selected values.
- `choices_from.sort`: `version-desc`, `version-asc`, `lexical-desc`, or `lexical-asc`.
- `choices_from.version_scheme`: `versionish` or `semver`.
- `choices_from.prefer_stable`: rank stable releases before prerelease-looking refs.
- `choices_from.limit`: truncate the resolved list.
- `choices_from.refresh`: `auto`, `always`, or `never`.
- `choices_from.cache_ttl_seconds`: reuse a local `.gaia/input-cache` entry while fresh; stale cache is also used if the remote fetch fails.
- `choices_from.max_age_warning_seconds`: warn when a cache entry is older than this threshold.
- `choices_from.fallback_choices`: offline/error fallback if no cache is available.
- `choices_from.allow_empty`: allow a dynamic source to resolve an empty choice list.
- `choices_from.on_error`: `fail`, `warn`, or `ignore`.
- `choices_from.timeout_seconds`: timeout passed to supported fetch helpers such as `curl`.
- `choices_from.auth_env`: environment variable containing a bearer token for HTTP choices.
- `choices_from.credential_helper`: reserved for explicit git credential-helper policy; git currently uses the user's normal git configuration.
- `choices_from.include_prereleases`: include GitHub prereleases for `github-releases`.
- `choices_from.include_drafts`: include GitHub draft releases for `github-releases`.
- `choices_from.lock` and `choices_from.lock_key`: reserved for future lockfile writing; current resolution still uses cache/fallback behavior.
- `default_from`: `first-choice` or `latest-stable`.

When `strip_prefix` changes the selected value, interpolate the prefix explicitly where
the underlying git ref still needs it:

```toml
[[sources]]
id = "app"
kind = "git"
repo = "https://github.com/example/app.git"
branch = "release/${input.release_branch}"

[inputs.release_branch]
kind = "enum"
default_from = "latest-stable"

[inputs.release_branch.choices_from]
kind = "git-branches"
source = "app"
include = ["release/v2026*"]
strip_prefix = "release/"
sort = "version-desc"
```

GitHub release choices:

```toml
[inputs.release_ref]
kind = "enum"
default_from = "latest-stable"

[inputs.release_ref.choices_from]
kind = "github-releases"
repo = "PhotonVision/photonvision"
include_prereleases = false
include_drafts = false
sort = "published-desc"
limit = 25
cache_ttl_seconds = 3600
fallback_choices = ["v2026.3.4"]
```

JSON manifest choices:

```toml
[inputs.release_ref]
kind = "enum"
default_from = "first-choice"

[inputs.release_ref.choices_from]
kind = "json"
url = "https://example.com/releases.json"
json_path = "$.releases[*].tag"
include = ["v2026*"]
exclude = ["*-rc*"]
selected_value_template = "refs/tags/${choice}"
sort = "version-desc"
```

Command choices:

```toml
[inputs.release_ref]
kind = "enum"
default_from = "first-choice"

[inputs.release_ref.choices_from]
kind = "command"
command = ["./scripts/list-release-tags.sh"]
sort = "version-desc"
timeout_seconds = 5
fallback_choices = ["v2026.3.4"]
```

Validation:
- required inputs must be selected
- integer inputs must parse
- boolean inputs accept `1`, `0`, `true`, `false`, `yes`, `no`, `on`, `off`
- enum inputs must match one of `choices`

Interpolation:
- `${input.target}`
- `${inputs.target}`

## Presets

Presets are named overlays.

```toml
preset = "release"

[presets.release]
env_files = ["runtime.env"]
env = { GAIA_MODE = "release" }
overrides = [
  ["input.profile", "release"],
  ["workspace.out_dir", ".gaia/examples/my-build/out-release"],
]
```

Preset order:
- config defaults
- selected preset
- env files
- inline env
- process env
- CLI env overrides
- CLI `--set`

## Interpolation

```toml
[interpolation]
allow_unresolved = false
values = [
  ["default_branch", "main"],
  ["release_channel", "${env:GAIA_MODE}"],
]
```

Supported interpolation targets include:
- `${build.name}`
- `${build.version}`
- `${build.branch}`
- `${build.target}`
- `${build.profile}`
- `${workspace.root_dir}`
- `${workspace.build_dir}`
- `${workspace.out_dir}`
- `${workspace.paths.<alias>}`
- `${env:NAME}`
- `${preset.name}`
- `${input.name}`
- `${inputs.name}`
- `${interpolation.values.name}`

If unresolved tokens remain:
- warning when `allow_unresolved = true`
- error when `allow_unresolved = false`

## Failure Policy

```toml
[failure]
rollback_on_error = true
preserve_failed_outputs = false
rollback_domains = ["sources", "artifacts", "installs", "stage", "images", "checkpoints"]
keep_going = false
```

Meaning:
- `rollback_on_error`
  Roll back completed current-run outputs on failure.
- `preserve_failed_outputs`
  Keep the failed operation’s partial outputs for debugging.
- `rollback_domains`
  Restrict cleanup to specific domains.
- `keep_going` (default `false`)
  After a failure, let independent operations run to completion instead of
  stopping them. Operations that depend (directly or transitively) on a failed
  operation are skipped and reported as skipped; the run still fails. Work that
  finished is kept rather than rolled back, and is recorded in the reuse state
  so the next run reuses it. Failed operations' partial outputs are still
  cleaned according to `rollback_on_error`, `preserve_failed_outputs` and
  `rollback_domains`. Override with `--set policy.failure.keep_going=true`.

Allowed rollback domains:
- `sources`
- `artifacts`
- `installs`
- `stage`
- `images`
- `checkpoints`

## Execution Policy

```toml
[execution]
jobs = 4

[execution.output_retention]
stdout_bytes = 1048576
stderr_bytes = 1048576
stdout_lines = 1000
stderr_lines = 1000
failure_tail_lines = 100
```

Output retention controls how much external command output Gaia keeps in memory and reports. The byte and line limits apply to retained stdout/stderr tails from subprocess execution. `failure_tail_lines` controls how many merged log lines are copied into structured failure reports.

Each value defaults to the shown release default when omitted or set to `0`. The same fields can be set from the CLI with `--set execution.output_retention.<field>=...` or `--set policy.execution.output_retention.<field>=...`.

`jobs` controls Gaia's operation scheduler. It limits how many independent Gaia operations may run at once; it is not forwarded to backend build tools as-is.

When several CPU-heavy operations (artifact builds, Buildroot prepare/build) run at the same time, Gaia splits the available cores evenly between them and caps each spawned tool with `CARGO_BUILD_JOBS`, `MAKEFLAGS=-jN` and `CMAKE_BUILD_PARALLEL_LEVEL` (and Buildroot's `make -j` when `providers.buildroot.local_jobs = 0`). A heavy operation that runs alone keeps its tools' defaults. Variables you already set in the environment are never overridden, and an explicit `local_jobs` wins.

`failure_tail_lines` also bounds streamed build output: every line goes to the live progress/TUI sink as it arrives, but only the newest `failure_tail_lines` lines are retained, and only failed operations report them. Successful operations do not re-emit their build log.

## Provider Execution Policy

Provider policy lives under `[providers.*]`.

Rust and Git have extra specialized fields:
- Rust: `allow_nested_build`, `batch_builds`
- Git: `allow_remote_resolution`

`batch_builds` (default `false`) builds nested cargo artifacts that share a
source workspace, target triple, profile, feature flags and execution backend
with a single `cargo build -p a -p b ...` (one container start and one
dependency resolution) and then copies each output. Per-artifact outputs,
marker and state files are the same as for individual builds. If the combined
build fails, each artifact is rebuilt on its own so the failure is attributed
to the right artifact. Note that cargo unifies dependency features across all
packages selected in one invocation, so a batched binary can differ from the
same package built alone when packages enable different features of a shared
dependency. It is therefore opt-in; enable it when your packages agree on
shared dependency features.

Every provider supports:
- `retry_attempts`
- `retry_backoff_ms`
- `retry_backoff_strategy`
- `timeout_seconds`

Command providers may also expose provider-local worker counts. Buildroot uses `local_jobs` for `make -j`; leave it at `0` to let Buildroot choose its own default or set it explicitly to avoid nested oversubscription.

Example:

```toml
[providers.rust]
allow_nested_build = false
batch_builds = true  # opt-in; see above
retry_attempts = 2
retry_backoff_ms = 500
retry_backoff_strategy = "exponential"
timeout_seconds = 300

[providers.git]
allow_remote_resolution = true
retry_attempts = 2
retry_backoff_ms = 250
retry_backoff_strategy = "fixed"
timeout_seconds = 60

[providers.buildroot]
retry_attempts = 1
retry_backoff_ms = 0
retry_backoff_strategy = "fixed"
timeout_seconds = 900
local_jobs = 4
download_dir = ".gaia/cache/buildroot/dl"

[providers.buildroot.ccache]
enabled = true
dir = ".gaia/cache/buildroot/ccache"
```

For Buildroot, `download_dir` is passed as `BR2_DL_DIR` so source tarballs can be
shared across clean builds. Without `download_dir`, Gaia still passes
`BR2_DL_DIR=<workspace>/.gaia/cache/buildroot/dl` through the environment, so
downloads survive re-fetching the Buildroot source and are shared by every
build in the workspace. When `providers.buildroot.ccache.enabled = true`,
Gaia enables `BR2_CCACHE` and passes `BR2_CCACHE_DIR` when `dir` is set.
Cache directories outside the workspace are mounted into Docker builds.

Buildroot's output tree is cleaned when the effective `.config` changes. The
generated version header and settings that cannot change the build output
(`BR2_DL_DIR`, `BR2_CCACHE_DIR`, `BR2_JLEVEL`, download mirrors) are ignored
for that comparison. Squashfs tuning (`BR2_TARGET_ROOTFS_SQUASHFS4_*`, block
size, padding) is ignored as well, because root filesystem images are
regenerated on every `make`; switching compression never forces a rebuild.

Gaia delivers the image feed (installs, stage files, env sets, services)
through Buildroot itself: it stages the feed next to the output tree and
appends a generated script to `BR2_ROOTFS_POST_BUILD_SCRIPT` on the `make`
command line (`.config` is not modified). Post-build scripts run after
package installation, stripping and rootfs overlays, so a single `make` packs
every image once with the feed included. Feed paths removed since the last
run are deleted from `target/` before `make`. If Buildroot does not run the
script, Gaia falls back to applying the feed after `make` and refreshing the
images.

#### Shared Buildroot trees

```toml
[providers.buildroot]
shared_output = true
# optional; default ".gaia/cache/buildroot/shared" under the workspace root
shared_output_dir = "/var/cache/gaia/buildroot-shared"
```

By default every build compiles its own Buildroot output tree under
`${workspace.build_dir}/image/buildroot-output`. With `shared_output = true`,
builds whose Buildroot inputs are identical compile packages once, in a tree
at `<shared_output_dir>/<key>`. The key is a digest of:

- the Buildroot source identity (the resolved commit or content digest the
  source provider recorded),
- `defconfig`, `defconfig_path` and `config_fragments` (paths and content),
- `config_overrides`,
- the external tree and package override directories,
- `ccache.enabled` and the Docker image.

The build name, build directory and image feed are not part of the key, so
`helios-base-os-cm5` and `helios-full-cm5` share one tree when only their
feeds differ. Changes inside external trees or package override directories
are handled in place, like a private tree.

Each build keeps a view at its usual output path: `build`, `host`, `staging`
and `per-package` link into the shared tree, `.config` is a copy, and
`target/` and `images/` are private copy-on-write clones (`cp --reflink=auto`,
falling back to a plain copy on filesystems without reflinks). The feed is
applied only to the private target, and each root filesystem image is packed
from it with the fakeroot script Buildroot generated for the shared tree, then
the post-image scripts run against the private `images/`. After the first
full `make`, the shared tree only runs `make target-finalize`, so each build
packs its images once. No file from one build's feed reaches the shared tree or
another build's image.

A file lock (`<key>.lock`) serializes builds that use the same tree; builds
with different keys run in parallel. When a build moves to a new key (for
example after a config change), it releases the old tree, which is deleted
once no build uses it. Turning `shared_output` off again replaces the view
with a private tree on the next run.

Limitations: initramfs, UBI, ISO9660, AXFS, cloop, OCI and YAFFS2 root
filesystems depend on more than the target tree and are rejected in shared
mode. Setting a different squashfs compression per build gives those builds
separate trees, because the key includes `config_overrides`.

#### Faster compression for development builds

Buildroot `config_overrides` can be set from presets and `--set` with
`image.buildroot.config_overrides.<SYMBOL>`. Keep XZ in the base config for
releases and switch to zstd (or lz4) in a development preset:

```toml
[image]
kind = "buildroot"
config_overrides = [["BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "y"]]

[presets.dev]
overrides = [
  ["image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "n"],
  ["image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_ZSTD", "y"],
]
```

```sh
gaia run configs/builds/helios.toml --preset dev
# or, for one run:
gaia run configs/builds/helios.toml \
  --set image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_XZ=n \
  --set image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_ZSTD=y
```

Set the old choice to `n` explicitly so Kconfig does not keep it. Switching
compression does not clean the Buildroot output tree; only the root filesystem
image is regenerated.

Retry strategies:
- `fixed`
- `exponential`

## Provenance

```toml
[provenance.identity]
project = "gaia-image-builder"
vendor = "Prometheus Dynamics"
channel = "release"
labels = [
  ["branch", "${build.branch}"],
]
```

## Workspace

```toml
[workspace]
root_dir = "."
build_dir = ".gaia/build/my-build"
out_dir = ".gaia/out/my-build"

[[workspace.named_paths]]
alias = "assets"
path = "assets"
kind = "host"

[[workspace.named_paths]]
alias = "generated"
path = "generated"
kind = "logical"
```

Fields:
- `root_dir`
  Logical repo/workspace root for path resolution.
- `build_dir`
  Mutable build workspace.
- `out_dir`
  Mutable published output location.

Named path kinds:
- `host`
  Real host filesystem path.
- `logical`
  Logical alias used as a semantic reference.

## Clean

```toml
[clean]
default = "dist"

[clean.profiles.dist]
description = "Remove generated build outputs and package cache"
build = true
out = true
paths = [
  ".cache/gaia",
  "@generated",
]

[clean.profiles.outputs]
out = true
```

Fields:
- `default`
  Optional profile used by `gaia clean --target configured` and by bare
  `gaia clean` when no explicit target or path is passed.
- `clean.profiles.<name>.description`
  Optional human-facing profile description.
- `clean.profiles.<name>.build`
  Include `workspace.build_dir`.
- `clean.profiles.<name>.out`
  Include `workspace.out_dir`.
- `clean.profiles.<name>.paths`
  Additional paths to remove. Paths use the same workspace resolution as other
  Gaia paths, including `@alias/...`.

Bare `gaia clean` removes `workspace.build_dir` and `workspace.out_dir` when no
default clean profile is configured.

## Sources

```toml
[[sources]]
id = "workspace-root"
kind = "path"
path = "${workspace.root_dir}"
refresh = "never"
pin = "locked"

[[sources]]
id = "buildroot-upstream"
kind = "git"
repo = "https://github.com/buildroot/buildroot.git"
tag = "2025.11"
update = true
refresh = "always"
pin = "floating"

[[sources]]
id = "seed-rootfs"
kind = "archive"
path = "@assets/rootfs/base-rootfs.tar"
strip_components = 0
refresh = "never"
pin = "locked"

[[sources]]
id = "tool-cache"
kind = "download"
url = "https://example.invalid/tool.tar.xz"
sha256 = "abc123"
output_path = "${workspace.build_dir}/downloads/tool.tar.xz"
refresh = "auto"
pin = "locked"
```

Source kinds:
- `git`
- `path`
- `archive`
- `download`

Policies:
- `refresh = "auto" | "always" | "never"`
- `pin = "floating" | "locked"`

`pin = "locked"` on its own only means "do not re-fetch once materialized":
each machine keeps whatever commit it fetched first. To pin git sources to
the same commit everywhere, use a lockfile.

### Git Source Lockfile

`gaia lock <build.toml>` writes `<build>.gaia.lock` next to the build
entrypoint (for example `configs/builds/cm5.toml` →
`configs/builds/cm5.gaia.lock`). Commit it alongside the build config.

```toml
# Generated by `gaia lock`. Commit this file to pin git sources.
version = 1

[[git]]
source = "orion"
repo = "https://github.com/example/orion.git"
ref = "branch:main"
commit = "3f2c9e0d4b1a7c8e6f5d4c3b2a1908f7e6d5c4b3"
```

Each entry is keyed by the source id plus its `repo` and `ref` (`branch:<name>`,
`tag:<name>`, or `head:HEAD`). When an entry matches the configured source:
- materialization clones the repository and checks out exactly that commit
  (detached), instead of the branch or tag tip; remote sources skip the
  `ls-remote` resolution step,
- the source counts as pinned, so `refresh = "auto"` no longer re-materializes
  a remote branch on every run,
- the locked commit is part of the source fingerprint, so moving the lock
  (`gaia lock --update`) re-materializes the source and rebuilds what depends
  on it, while new upstream commits do not,
- the source state records `lock_mode=locked` and `locked_commit_sha`.

Validation warns with `git_lock_stale` when an entry exists for a source whose
`repo` or ref changed; the stale entry is ignored (the source floats) until
`gaia lock` rewrites it. An unreadable lockfile is a validation error
(`git_lockfile_invalid`). Sources with an explicit `rev` are already pinned and
get no entry. Without a lockfile, git sources behave exactly as before.

Validation also warns with `git_source_ref_divergence` when two sources point
at the same repository (ignoring a trailing `.git`) at different refs.

### Download Cache

Download sources with a `sha256` are stored after verification in a
content-addressed cache, `<workspace>/.gaia/cache/downloads/sha256/<sha>`,
shared by every build in the workspace. Re-materializing the source copies
the cached file instead of downloading it again. Downloads without a
`sha256` are always fetched. The file is hashed once per download, both to
verify it and to record `output_sha256` in the source state. Remove the cache
with `gaia clean <build> --all-caches`.

## Artifacts

```toml
[[artifacts]]
id = "helios-api"
kind = "rust"
package = "helios-api"
source = "workspace-root"
profile = "${build.profile}"
dependencies = ["helios-common"]
install_name = "helios-api"
install_class = "binary"
install_dest_hint = "/usr/bin/helios-api"
output_path = "${workspace.out_dir}/artifacts/helios-api"

[[artifacts]]
id = "orion-node"
kind = "java"
build_target = "build/libs/orion-node.jar"
source = "workspace-root"
output_path = "${workspace.out_dir}/artifacts/orion-node.jar"

[[artifacts]]
id = "camera-jni"
kind = "java"
build_target = "build/libs/camera-jni-linuxarm64.jar"
build_command = ["tools/build_arm64_jni.sh"]
build_env = [
  ["SYSROOT_DIR", "${workspace.build_dir}/image/buildroot-output/host/aarch64-buildroot-linux-gnu/sysroot"],
  ["MAVEN_LOCAL_REPO", "${workspace.build_dir}/m2"],
]
after_image_prepare = true
source = "camera-jni-source"
output_path = "${workspace.out_dir}/artifacts/camera-jni-linuxarm64.jar"

[[artifacts]]
id = "frontend-package"
kind = "node"
package_dir = "frontend"
source = "workspace-root"
output_path = "${workspace.out_dir}/artifacts/frontend.tgz"

[[artifacts]]
id = "python-wheel"
kind = "python"
package_dir = "sdk/python"
source = "workspace-root"
output_path = "${workspace.out_dir}/artifacts/sdk.whl"

[[artifacts]]
id = "heliosctl"
kind = "go"
package = "./cmd/heliosctl"
source = "workspace-root"
output_path = "${workspace.out_dir}/artifacts/heliosctl"
```

Artifact common fields:
- `id`
- `kind`
- `source`
- `profile`
- `dependencies`
- `after_image_prepare`
- `install_name`
- `install_class`
- `install_dest_hint`
- `output_path`

Artifact kinds:
- `rust`
  - `package`
  - `target_name`
  - `emit_directory`
  - `features` (list, passed as `--features a,b`; supports interpolation)
  - `no_default_features` (bool, passed as `--no-default-features`)
  - `all_features` (bool, passed as `--all-features`; cannot be combined with
    `features` or `no_default_features`)

  Feature flags are part of the artifact's identity: changing them rebuilds the
  artifact, and non-default flags are recorded in its backend state. Only
  artifacts with identical flags are batched into one cargo invocation.

  ```toml
  [[artifacts]]
  id = "orion-node"
  kind = "rust"
  package = "orion-node"
  no_default_features = true
  features = ["metrics"]
  output_path = "out/orion-node"
  ```
- `java`
  - `build_target`
  - `build_args`
  - `build_command`
  - `build_env`
- `node`
  - `package_dir`
- `python`
  - `package_dir`
- `go`
  - `package`

Java artifacts run Maven or Gradle automatically when `build_command` is omitted. Use `build_args` to replace the default Maven/Gradle arguments while keeping tool detection, or use `build_command` for a source-local command such as a wrapper script. `build_env` adds environment variables for either mode. `after_image_prepare = true` schedules the artifact after Buildroot prepare, which is useful when the artifact needs the generated Buildroot sysroot before the final image feed is assembled.

### Artifact Execution

Artifacts run on the host unless `[execution.docker]` or the artifact's own
`execution` table selects Docker:

```toml
[[artifacts]]
id = "helios-engine"
kind = "rust"
package = "helios-engine"
source = "workspace-root"
output_path = "${workspace.out_dir}/artifacts/helios-engine"

[artifacts.execution]
backend = "docker"          # or "host"

[artifacts.execution.docker]
# Either reference an existing image ...
image = "helios-cross-rust194"
# ... or let Gaia build it from a Dockerfile (paths are workspace paths,
# `@alias/...` works).
dockerfile = "docker/cross-rust/Dockerfile"
context = "docker/cross-rust"   # optional, defaults to the Dockerfile's directory
```

With `dockerfile`, Gaia hashes the Dockerfile and every file in the context
(skipping `.git`, `.gaia`, `target` and `node_modules`) and runs the artifact
in `gaia-local/<name>:<hash>`, where `<name>` is the repository part of
`image` when set, otherwise the artifact id. Before the artifact builds, Gaia
checks for that tag with `docker image inspect` and runs
`docker build -f <dockerfile> -t <tag> <context>` only when it is missing. The
hash is part of the artifact fingerprint, so editing the Dockerfile or its
context rebuilds the image and the artifact. The artifact state records
`execution_backend_image` (the tag), `execution_backend_image_id`,
`execution_backend_image_hash` and `execution_backend_image_dockerfile`. Keep
the context small: it is hashed at every plan. A clean machine only needs
Docker and the repository to build the image.

Install classes:
- `binary`
- `library`
- `archive`
- `config`
- `service`
- `data`

## Install

```toml
[[install]]
id = "install-helios-api"
artifact = "helios-api"
dest = "/usr/bin/helios-api"
replace = true
mode = 493
owner = "root"
group = "root"
```

## Stage

```toml
[[stage.files]]
id = "motd"
src = "@assets/etc/motd"
dest = "/etc/motd"
origin = "static-asset"

[[stage.env_sets]]
id = "runtime-env"
name = "runtime"
entries = [
  ["GAIA_MODE", "release"],
  ["HELIOS_TARGET", "${build.target}"],
]

[[stage.services]]
id = "helios-api"
name = "helios-api.service"
unit_path = "@assets/systemd/helios-api.service"
```

Stage file origins:
- `static-asset`
- `generated`
- `provider-emitted`

## Image

### Buildroot

```toml
[image]
kind = "buildroot"
defconfig = "raspberrypi_defconfig"
external_tree = "@assets/buildroot"
external_tree_mode = "required"

[image.feed]
install_entries = ["install-helios-api"]
stage_files = ["motd"]
stage_env_sets = ["runtime-env"]
stage_services = ["helios-api"]

[[image.expected_images]]
name = "sdcard.img"
format = "raw"
required = true

[image.output]
collect_dir = "${workspace.out_dir}/images"
archive_name = "${build.name}-${build.version}.img.xz"
emit_report = true
```

Buildroot fields:
- `defconfig`
- `external_tree`
- `external_tree_mode = "auto" | "required" | "disabled"`
- `expected_images[]`

Expected image formats:
- `tar`
- `cpio`
- `ext2`
- `ext3`
- `ext4`
- `ubifs`
- `ubi`
- `jffs2`
- `romfs`
- `cramfs`
- `cloop`
- `f2fs`
- `btrfs`
- `squashfs`
- `raw`
- `kernel`
- `erofs`

### Starting Point

```toml
[image]
kind = "starting-point"
rootfs_path = "${workspace.build_dir}/seed-rootfs"
rootfs_validation_mode = "require-directory"
output_mode = "copy-and-archive"
```

Starting-point fields:
- `rootfs_path`
- `rootfs_validation_mode`
- `output_mode`

Validation modes:
- `require-exists`
- `require-directory`
- `require-file`
- `allow-missing`

Output modes:
- `copy-rootfs`
- `archive-only`
- `copy-and-archive`

### Image Feed

The image feed declares which install/stage domains are part of the final image contract:
- `install_entries`
- `stage_files`
- `stage_env_sets`
- `stage_services`

If omitted, the current compiler auto-feeds all entries in the corresponding domain.

### Image Assembly

Image assembly runs after the image provider has produced its base outputs. It creates named work trees, stages selected files, runs typed transforms, and can package trees into supported filesystem artifacts.

```toml
[image.assembly]
work_dir = "${workspace.build_dir}/assembly"
out_dir = "$provider.images"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.files]]
tree = "boot"
src = "@assets/board/config.txt"
dest = "config.txt"

[[image.assembly.files]]
tree = "boot"
src_glob = "$provider.images/*.dtb"
dest = "."
optional = true

[[image.assembly.files]]
tree = "initramfs"
src = "@assets/initramfs/init"
dest = "init"
mode = "0755"

[[image.assembly.transforms]]
kind = "gzip"
src = "$provider.images/Image"
dest = "$assembly.tree.boot/kernel.img"
deterministic = true

[[image.assembly.transforms]]
kind = "compile-dts"
src = "@assets/overlays/example-overlay.dts"
dest = "$assembly.tree.boot/overlays/example-overlay.dtbo"

[[image.assembly.busybox_initramfs]]
tree = "initramfs"
busybox = "$provider.target/bin/busybox"
include_runtime_libs = false
applets = ["sh", "mount", "mkdir", "switch_root"]

[[image.assembly.filesystems]]
id = "bootfs"
kind = "vfat"
source_tree = "boot"
output = "$provider.images/boot.vfat"
size = "32M"

[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-gzip"
source_tree = "initramfs"
output = "$assembly.tree.boot/initramfs"
deterministic = true

[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "mbr"
signature = "0x48454c49"

[[image.assembly.disks.partitions]]
name = "boot"
type_alias = "fat32-lba"
bootable = true
image = "$provider.images/boot.vfat"

[[image.assembly.disks.partitions]]
name = "rootfs"
type = "0x83"
image = "$provider.images/rootfs.squashfs"
```

Assembly roots:
- `$provider.images`: provider image output directory.
- `$provider.target`: provider target root tree, when the provider exposes one.
- `$provider.host`: provider host tools directory, when the provider exposes one.
- `$provider.staging`: provider staging/sysroot directory, when the provider exposes one.
- `$assembly.work`: configured assembly work directory.
- `$assembly.out`: configured assembly output directory.
- `$assembly.tree.<id>`: path for a declared assembly tree.
- `@assets/...`: workspace asset path.

Assembly behavior:
- Declared `trees` are cleaned and recreated before staging.
- File entries use exactly one of `src` or `src_glob`.
- Glob staging is deterministic and currently supports simple non-recursive `*` filename patterns.
- `optional = true` skips missing sources without failing the build and records the skip in runtime state.
- `mode = "0755"` applies Unix file modes on Unix hosts.
- Transform kinds currently implemented are `copy`, `gzip`, and `compile-dts`.
- `gzip` uses stable name and timestamp behavior for deterministic output.
- `compile-dts` resolves `dtc` from provider host tools first, then host `PATH`.
- `busybox_initramfs` copies the configured BusyBox binary to `bin/busybox`, creates requested applet symlinks in `bin`, and can copy `ldd`-reported runtime libraries when `include_runtime_libs = true`.
- Runtime library discovery is intended for Linux-style hosts and target binaries that can be inspected by host `ldd`; static BusyBox output skips library copying.
- Filesystem kinds currently implemented are `vfat`, `cpio`, and `cpio-gzip`.
- `vfat` uses `mtools` (`mformat` and `mcopy`), resolving provider host tools first, then host `PATH`; `size` accepts byte values or `K`, `M`, and `G` suffixes.
- Raw MBR disk assembly writes up to four 1 MiB-aligned partitions sequentially from declared partition images.
- MBR partition `type` accepts raw `0xNN` values; `type_alias` currently supports `fat32-lba` and `linux`.
- Assembly runtime state is included in provenance and manifest reports with staged file, transform, and filesystem output sizes and digests.

Provider hooks and assembly:
- Keep provider-native post-image hooks for provider-specific behavior that must run inside the provider build environment.
- Prefer typed assembly for generic staging, compression, device-tree compilation, filesystem images, raw MBR disk layout, and BusyBox initramfs generation.
- During migration, provider hooks and typed assembly can coexist; assembly runs after provider image build and feed refresh so it can consume provider outputs.
- Gaia reports publish-directory hygiene warnings for transient directories and large unexpected files, but these warnings do not fail builds. Tune the policy with `[reporting.output_hygiene]`:

```toml
[reporting.output_hygiene]
large_file_threshold_bytes = 104857600
transient_dir_names = [".cache", "build", "buildroot-output", "sources", "target"]
```

Buildroot migration example:
- Old hook-oriented approach: set `BR2_ROOTFS_POST_IMAGE_SCRIPT="support/scripts/genimage.sh"` and `BR2_ROOTFS_POST_SCRIPT_ARGS="-c ${CONFIG_DIR}/genimage.cfg"` in the Buildroot defconfig to create `sdcard.img`.
- Typed assembly equivalent: leave Buildroot responsible for producing root filesystem artifacts, then declare an MBR disk in TOML:

```toml
[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "mbr"
signature_text = "GAIA"

[[image.assembly.disks.partitions]]
name = "rootfs"
type_alias = "linux"
image = "$provider.images/buildroot-output/images/rootfs.ext4"
```

## Checkpoints

```toml
[[checkpoints]]
id = "base-image"
backend = "local"
anchor = "image"
use_policy = "auto"
upload_policy = "off"

[[checkpoints]]
id = "after-api-stage"
backend = "local"
anchor = "stage-service:helios-api"
use_policy = "always"
upload_policy = "off"
```

Checkpoint fields:
- `id`
- `backend`
- `anchor`
- `use_policy`
- `upload_policy`

Checkpoint policies:
- `off`
- `auto`
- `always`

Supported anchor forms:
- `image`
- `install:<install-id>`
- `stage-file:<stage-file-id>`
- `stage-env:<stage-env-set-id>`
- `stage-service:<stage-service-id>`

Important:
- unknown anchors are rejected
- anchors outside the active image feed are rejected
- required/conditional checkpoints on disconnected anchors are rejected as impossible ordering

## Reporting

```toml
[reporting]
summary = true
provenance = true
manifest = true

[reporting.masking]
enabled = true
replacement = "***"
patterns = ["TOKEN", "SECRET", "PASSWORD", "API_KEY"]
```

## Template Files

See:
- [../examples/templates/full/README.md](../examples/templates/full/README.md)
- [../examples/templates/minimal-buildroot/build.toml](../examples/templates/minimal-buildroot/build.toml)
- [../examples/templates/minimal-starting-point/build.toml](../examples/templates/minimal-starting-point/build.toml)
