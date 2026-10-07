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
  repositories. An existing checkout is reused without fetching, and a commit
  already in the mirror is checked out without a network fetch, so use full
  commit shas for `rev`. Fetches and clones use `[providers.git]
  timeout_seconds` (the largest of the local config files, or
  `--set policy.providers.git.timeout_seconds=<n>`), 1800 seconds when it is
  not set.
- `when` behaves exactly as for local imports. An import whose `when` does not
  match is not loaded at all, whether it is local or from a source: nothing in
  it is fetched or token-resolved, so a layer selected for one target may use
  `@source:<id>` while other targets build without that source being
  reachable.
- A git source used only as an import source (by `source = ...` or
  `@source:<id>`) that no selected layer uses is left out of the plan; it is
  kept when an artifact or the image builds from it.
- Imports and `extends` inside a source-imported file resolve relative to that
  file, like local imports, and must stay inside the checkout: `..` or symlink
  escapes are rejected.
- The source stays a normal build source with the same id, materialized at
  the same commit.

Path tokens, rewritten when each file is loaded, at the start of a string value
or of any entry of a `:`-separated list such as
`external_tree = "@source:atlas/devices/raze/gaia/buildroot-external:raze/assets/buildroot"`:
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
gaia_version = ">=2.0.0"
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
- `gaia_version`
  Optional semver requirement on the Gaia binary (Cargo syntax: `">=2.1.0"`,
  `">=2.1, <3"`; a bare `"2.1.0"` means `^2.1.0`). It is checked on the raw
  TOML of every loaded file (including `extends` and imports) before any
  other parsing or validation, so an older binary fails with
  `this build requires gaia >=2.1.0, but gaia 2.0.0 is installed; upgrade with: cargo install ...`
  instead of misreading the file. The version only changes at releases;
  builds from `main` between releases report the last release. To require a
  setting added since then, use it: unknown keys fail validation.
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

A config file that is empty or whitespace-only (the entrypoint, an
`extends` parent or any import) fails to load: it is never intended, and is
usually a write that did not finish. A file that sets nothing (only
comments) is reported as a `config_layer_empty` warning.

```toml
[expect]
artifacts = ["helios-api", "helios-engine"]
installs = ["install-helios-api"]
sources = ["vision-plugin"]
```

`[expect]` lists ids the build must end up with, in any layer. Validation
fails (`config_expected_missing`) when one is missing, so a layer that
stops providing it (deleted, emptied, no longer imported, or its `when` no
longer matching) cannot silently drop it from the image. Lists from all
layers are combined.

Unknown keys fail validation (`config_unknown_key` errors, for example
`unknown key 'build_command' in '<file>': this gaia does not know it`), so a
typo, or a file written for a newer Gaia, never builds with the setting
silently missing.
The check covers top-level keys and the `workspace`, `product`,
`interpolation`, `clean`, `execution`, `failure`, `providers.*`,
`provenance`, `reporting`, `stage`, `install` and `checkpoints` tables. Tables
whose fields depend on a `kind` (`sources`, `artifacts`, `image`) and free-form
maps (`inputs`, `presets`, `env`) are not checked.

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
shared dependency features. Artifacts that must agree on features should use
a [Rust build group](#rust-build-groups) instead: groups always build together,
with or without `batch_builds`.

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
parallel_packages = true
# download_dir = "/srv/buildroot-dl"     # default: the user cache below

[providers.buildroot.ccache]
enabled = true
# dir = "/srv/buildroot-ccache"          # default: the user cache below
max_size = "50G"                         # default 50G
```

Downloads and the compiler cache are shared by every build of every project
of the user by default, so a second project, or a rebuild after the output
tree was wiped, reuses them. They live under the user cache root:
`$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`
(`buildroot/dl` and `buildroot/ccache`), or under `<workspace>/.gaia/cache`
when that cannot be created.

- `download_dir` is passed as `BR2_DL_DIR` (written to `.config` when set,
  through the environment otherwise), so source tarballs survive clean
  builds and re-fetching the Buildroot source.
- `ccache.enabled = true` sets `BR2_CCACHE=y`, `BR2_CCACHE_DIR` and
  `BR2_CCACHE_USE_BASEDIR=y` (paths relative to the output tree, so trees in
  other directories share cache entries), and sets `max_size` in the cache's
  `ccache.conf`. Buildroot keys entries on its toolchain, so builds with the
  same toolchain share them. The run summary reports how many of the run's
  compilations came from the cache:
  `buildroot ccache: 8123/9410 compilations from cache (86.3%)`.
- `parallel_packages = true` builds independent packages concurrently: it
  sets `BR2_PER_PACKAGE_DIRECTORIES=y` and runs the top-level `make` with
  `-j<local_jobs>` and a load limit of the same value (inherited by each
  package's own `make`, which still uses `BR2_JLEVEL` jobs). Buildroot marks
  per-package directories experimental; a few packages may not support them.
- Turning `ccache.enabled` or `parallel_packages` on or off changes the
  toolchain wrapper or the tree layout, so the next build cleans the output
  tree once.

#### Package cache

```toml
[providers.buildroot]
parallel_packages = true          # required

[providers.buildroot.package_cache]
enabled = true
# dir = "/srv/buildroot-packages" # default: <user cache root>/buildroot/packages
max_size = "100G"                 # default; least recently used packages go first
```

Built packages are cached by content and reused by every build of the user,
in any project, so a wiped output tree or a second image with the same
kernel, Mesa or libcamera restores them instead of compiling. After a
successful `make`, each package built in that run is archived: the files it
added to its per-package directories (everything that is not a hard link to
a dependency's file, so also what it installs outside its install steps,
such as an extracted external toolchain), its image files, file lists and
stamps. Before the next `make`, packages not yet built are restored,
dependencies first, when their key is cached and all their dependencies are
built or restored: as Buildroot's per-package preparation does, their
dependencies' trees are linked in, then their own files are added, and their
stamps are recreated in build order so `make` treats them as built.

A package's key covers:
- Buildroot's package infrastructure and the settings it references
  (architecture, toolchain, optimisation, hardening, init system);
- the package's `.mk` directory, `.hash` files, patches, version and
  download names;
- the values of the settings its `.mk` references, where a setting naming a
  file or directory counts by content, not path;
- the Docker image (or host compiler) it is built with;
- its dependencies' keys.

It does not depend on where the output tree is, the build name, or packages
the package does not use.

Packages built from a local directory (`SITE_METHOD = local`,
`<PKG>_OVERRIDE_SRCDIR`), and everything depending on them, are always
built. In text files that hold the output tree's path, it is rewritten on
restore. Packages whose binaries hold it (most host tools) are stored for
that path only: they are restored when the same build is rebuilt after a
wipe, and in other trees once they are built there, but not across trees at
different paths, which keeps their dependents building there too. Running
builds in Docker with the output tree at a fixed path lifts this. `linux` is
restored only when nothing still to be built needs its build tree (for
example out-of-tree kernel modules). The run summary reports
`buildroot package cache: <n> restored, <m> stored`.

Cache directories outside the workspace are mounted into Docker builds.

Buildroot never rebuilds a built package by itself, so Gaia compares the
effective `.config` and the package override trees with those the output tree
was last built from, and rebuilds as little as keeps it correct:

- Settings that cannot change what packages build are ignored: the version
  header, `BR2_DL_DIR`, `BR2_CCACHE_DIR`, `BR2_JLEVEL`, download mirrors,
  `BR2_EXTERNAL_*` (tree names, paths, versions), root filesystem image
  settings (`BR2_TARGET_ROOTFS_*`, post-image and fakeroot scripts, users and
  device tables) and import-source checkout directories in paths. Root
  filesystem images are regenerated on every `make`.
- Newly enabled packages, including new packages in override or external
  trees, simply build.
- A built package whose options, override directory contents or version
  changed is uninstalled (the files it installed in `target/`, `staging/`
  and `host/` that no other package also installed are removed) and its
  build directory is removed (`<pkg>-dirclean`), as is every package that
  depends on it, recursively.
- A package no longer enabled is uninstalled, and the packages that depended
  on it are rebuilt.
- A built package that gained or lost a dependency (for example kmod once xz
  is enabled) is rebuilt.
- Toolchain, architecture, libc, init system and other system-wide settings
  still clean the whole output tree (`make clean`), as does any change when
  Buildroot cannot report its package graph (`make show-info`, which Gaia
  runs only when something changed).

Both the clean and the rebuild log what caused them, for example
`buildroot rebuild of 3 package(s): libcamera, libcamera-apps, photonvision`
followed by `libcamera changed: BR2_PACKAGE_LIBCAMERA_PIPELINE_RPI_PISP unset
-> y`.

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
- `ccache.enabled`, `parallel_packages` (when on) and the Docker image.

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

#### Dropped config overrides

```toml
[providers.buildroot]
override_check = "error" # default; or "warn" / "off"
```

`olddefconfig` silently resets every symbol whose `depends on` is not met and
drops symbols that no longer exist, so a requested `BR2_PACKAGE_OPENJDK=y`
can vanish (for example when `BR2_PACKAGE_XORG7` is off) while validate and
plan pass. After the defconfig, fragments, `config_overrides` and cache
settings are applied (private and shared trees alike), Gaia compares every
`config_overrides` entry with the final `.config`:

- `y`, `m` or a value is **dropped** when the symbol is missing or
  `# KEY is not set`, and **changed** when it holds a different value;
- `n` (or `""`) is satisfied by a missing symbol or `# KEY is not set`;
- string quotes are optional, and `int`/`hex` values compare numerically;
- for repeated keys the last entry counts; `BR2_DL_DIR` and `BR2_CCACHE_DIR`
  are skipped because Gaia's cache policy rewrites them.

With `"error"` the image operation fails before the long `make`, listing each
symbol with the requested and final value and the hint
"usually an unmet `depends on`; check menuconfig for <KEY>". With `"warn"` the
build continues and each entry is reported as a warning: in the `gaia run`
output, in `summary.json` (`image_warnings`, counted in `warning_count`) and
on the image record of `manifest.json` (`warnings`). `"off"` skips the
comparison. `--set policy.providers.buildroot.override_check=warn` relaxes it
for one run.

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

  - `build_group` (string; see [Rust build groups](#rust-build-groups))

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

  Gaia collects `target/<triple>/<profile>/<target_name>` from the cargo
  target directory; `target_name` defaults to the file name of
  `output_path`. For a `cdylib` (or other library) set it to the file cargo
  writes, for example `target_name = "libvision_plugin.so"` for a package
  `vision-plugin`.
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

#### Rust build groups

Rust artifacts that name the same `build_group` are always built by one cargo
invocation, so cargo resolves features once for all of them. Use this when
binaries and plugins must agree on the features of shared crates, for example
an engine and the `cdylib` plugins it loads after checking an ABI or feature
fingerprint: separate `cargo build -p` runs can enable different features of
a shared dependency and produce incompatible builds.

For every member, Gaia runs `cargo build -p <each member package>` with the
sorted union of all members' `features`. The command is identical for each
member, whether the members are built together or one at a time (the second
build is then an incremental no-op), and members always share one batched
build without `[providers.rust] batch_builds`. Each member still collects its
own output (`package`, `target_name`, `output_path`) and has its own install
identity.

Members must share `source`, `target`, `profile`, `execution` and
`no_default_features`/`all_features`; validation reports the conflicting
members otherwise. `build_group` on a non-rust artifact is an error. A group
of one is allowed. Since one `--features` list serves every selected
package, prefer `package/feature` names so each feature is enabled on the
package that defines it.

```toml
[[artifacts]]
id = "helios-engine"
kind = "rust"
source = "helios"
package = "helios-engine"
profile = "release"
features = ["helios-engine/plugins"]
build_group = "helios"
install_name = "helios-engine"
install_dest_hint = "/usr/bin/helios-engine"
output_path = "${workspace.out_dir}/artifacts/helios-engine"

[[artifacts]]
id = "vision-plugin"
kind = "rust"
source = "helios"
package = "vision-plugin"          # [lib] crate-type = ["cdylib"]
target_name = "libvision_plugin.so"
profile = "release"
features = ["vision-plugin/simd"]
build_group = "helios"
install_name = "libvision_plugin.so"
install_class = "library"
install_dest_hint = "/usr/lib/helios/plugins/libvision_plugin.so"
output_path = "${workspace.out_dir}/artifacts/libvision_plugin.so"

[[install]]
id = "install-helios-engine"
artifact = "helios-engine"
dest = "/usr/bin/helios-engine"
mode = 493

[[install]]
id = "install-vision-plugin"
artifact = "vision-plugin"
dest = "/usr/lib/helios/plugins/libvision_plugin.so"
mode = 420
```

Both artifacts build with `cargo build -p helios-engine -p vision-plugin
--features helios-engine/plugins,vision-plugin/simd --release`.

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

Reuse fingerprints identify the toolchain an artifact was built with. Host
artifacts hash the host tool versions (`cargo`/`rustc`, `go`, `python3`,
`npm`/`node`, `mvn`/`gradle`). Docker artifacts never probe host tools (so a
host without Maven or Gradle no longer logs failed probes); they hash the
execution image instead:

- a Dockerfile-built image contributes its content hash, without calling
  Docker;
- a plain `image` (or `[execution.docker] image`) contributes its image id
  from `docker image inspect --format '{{.Id}}'`, probed once per process, so
  pulling or rebuilding a different image under the same tag rebuilds the
  artifact;
- an image that is not present locally contributes `image-missing:<tag>`.
  Planning does not fail; the first run after the image is pulled rebuilds the
  artifact once.

Moving to these signatures changes the fingerprint of every docker-backed
artifact once, so they rebuild on the first run after upgrading.

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
- `external_tree`: one or more `BR2_EXTERNAL` trees, `:`-separated as
  Buildroot expects (`"@source:atlas/devices/raze/gaia/buildroot-external:raze/assets/buildroot"`).
  Each entry is resolved on its own: `@self`, `@source:<id>` and `@alias`
  tokens may start any entry, and plain relative entries resolve against the
  workspace root. Every tree needs its own `external.desc` with a unique
  `name`. Gaia's generated package-override tree (`GAIA_GENERATED`) is
  appended automatically, so do not reuse that name. A later layer's
  `external_tree` replaces an earlier one rather than appending, so a layer
  that adds a tree must list the full combination.
- `external_tree_mode = "auto" | "required" | "disabled"`
- `expected_images[]`

Buildroot checks:
- `[providers.buildroot] override_check = "error" | "warn" | "off"` (default
  `"error"`): fails before `make` when `olddefconfig` dropped or changed a
  requested `config_overrides` entry.
- `[providers.buildroot] kernel_modules_check = "error" | "warn" | "off"`
  (default `"error"`): after `make`, fails when `target/lib/modules` holds
  fewer kernel modules than the kernel build's `modules.order` lists.
  Buildroot's `linux.mk` does not fail when `modules_install` stops part way,
  so without this an image can ship with most modules missing. Use `"warn"`
  if a post-build script removes modules on purpose.

Source commits: `${source.<id>.commit}` in stage env set values and build
labels becomes the exact commit the source builds from: a git source's
`gaia lock` commit or full-sha `rev`, an import source's checkout commit, or a
path source's `git rev-parse HEAD` (with `-dirty` for uncommitted tracked
changes). Use it to record versions in the image, for example:

```toml
[[stage.env_sets]]
id = "image-version"
name = "image-version"
entries = [
  ["DEVICE_PACKAGE_COMMIT", "${source.atlas.commit}"],
  ["ORION_COMMIT", "${source.orion.commit}"],
]
```

A git source that follows a branch or tag without a lock has no exact commit
before it is fetched, so its token is reported as unresolved: pin it with
`rev` or `gaia lock`.

Project identity: `${project.commit}` and `${project.describe}` describe the
git repository holding the build file. `${project.commit}` is the full `HEAD`
sha, suffixed `-dirty` when tracked files have uncommitted changes, and
`${project.describe}` is `git describe --tags --always --dirty`. They resolve
during normal interpolation, so they work anywhere `${build.version}` does,
including `version`, `archive_name`, env sets and archive manifest entries.
Use them to make image versions unique per build:

```toml
version = "${input.app_version}+${project.describe}"
```

Outside a git repository they stay unresolved and validation reports them.
Anything that embeds them changes with every commit. For example, putting
them in `version` gives artifacts a new build context on every commit, so
they rebuild each time. To keep artifact reuse, put the commit only into the
image's version records (env sets, os-release files, the update bundle
manifest) rather than `version`.

Source paths: `${source.<id>.path}` becomes the directory holding the
source's files: an import source's checkout (under
`.gaia/cache/import-sources/`), a path source's directory, or
`<build_dir>/sources/<id>` for git, archive and download sources (where Gaia
materializes them). Like the commit token it works in stage env set values
and build labels, and both tokens also work in Buildroot `config_overrides`
values, so a setting can name a file inside a source:

```toml
[image]
kind = "buildroot"
config_overrides = [
  ["BR2_ROOTFS_USERS_TABLES", "\"${source.orion.path}/packaging/buildroot/orion-users.table\""],
]
```

When an override value names a source's directory, the image operations
depend on that source, so it is materialized first. `BR2_ROOTFS_USERS_TABLES`,
`BR2_ROOTFS_DEVICE_TABLE` and `BR2_ROOTFS_STATIC_DEVICE_TABLE` are applied
when rootfs images are generated on every `make`, so changing them (including
an import source's checkout path changing with its `rev`) does not force a
full Buildroot clean.

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
- `xfs` (Buildroot 2026.08 or newer, `BR2_TARGET_ROOTFS_XFS`)

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
- Transform kinds currently implemented are `copy`, `gzip`, `zstd`, and `compile-dts`.
- `gzip` uses stable name and timestamp behavior for deterministic output.
- `zstd` runs `zstd -q -c --no-progress -T0 [-<level>] <src>`, resolving `zstd` from provider host tools first, then host `PATH`; the tool path and version are recorded in assembly state. The optional `level` (1-19) is only accepted on `zstd` transforms and defaults to zstd's own default.
- `compile-dts` resolves `dtc` from provider host tools first, then host `PATH`.
- `busybox_initramfs` copies the configured BusyBox binary to `bin/busybox`, creates requested applet symlinks in `bin`, and can copy `ldd`-reported runtime libraries when `include_runtime_libs = true`.
- Runtime library discovery is intended for Linux-style hosts and target binaries that can be inspected by host `ldd`; static BusyBox output skips library copying.
- Filesystem kinds currently implemented are `vfat`, `cpio`, and `cpio-gzip`.
- `vfat` uses `mtools` (`mformat` and `mcopy`), resolving provider host tools first, then host `PATH`; `size` accepts byte values or binary `K`, `M`, `G`, and `T` suffixes.
- Raw MBR disk assembly writes 1 MiB-aligned partitions sequentially (see [Disk partitions](#disk-partitions)).
- MBR partition `type` accepts raw `0xNN` values; `type_alias` currently supports `fat32-lba` and `linux`.
- Assembly runtime state is included in provenance and manifest reports with staged file, transform, filesystem, disk, and archive output sizes and digests.
- Assembly order follows dependencies: a step that reads a path another step writes (the same file, a file in a directory it fills, or a glob it matches) runs after it, whatever its kind or position, so for example a transform compressing a filesystem built by the same assembly sees this run's image. Trees are prepared first; steps that do not depend on each other run in the order dirs, symlinks, files, BusyBox initramfs, transforms, filesystems, disks, archives. Steps that read each other's outputs in a cycle are rejected, by validation and at run time.
- Before the steps run, outputs of transforms, filesystems, disks and archives left from an earlier run are removed, so a step reading one before it is rebuilt fails instead of using a stale file.
- Validation warns (`assembly_reads_later_output`) when a step reads what a step of a later kind or position produces: Gaia versions before dependency ordering ran it first, on the previous run's file.

#### Disk partitions

`[[image.assembly.disks.partitions]]` fields:
- `name`: label used in state and errors.
- `type` (raw `0xNN`) or `type_alias` (`fat32-lba`, `linux`); defaults to `linux` (`0x83`).
- `bootable`: sets the MBR active flag. Only allowed on primary partitions; validation rejects it on logical partitions (`assembly_partition_bootable_logical`).
- `image`: file written at the partition start. Optional when `size` is set.
- `size`: partition size in bytes or with a binary `K`/`M`/`G`/`T` suffix (`"16M"`, `"2G"`). Without it the partition is exactly as large as its image (rounded up to 512-byte sectors). With it the image must fit, otherwise assembly fails before the disk is written; the space after the image is left unwritten.
- `wipe = true`: only for empty partitions (no `image`). Writes zeros over the first 1 MiB of the partition (less if the partition is smaller) so filesystem signatures left by a previous flash do not survive when the image is written to a device. Validation rejects `wipe` together with `image`, since an image already overwrites the partition start.

Layout:
- Partitions are placed in declaration order, each starting at the next `alignment_lba` boundary (default 2048 sectors) after `first_lba` (default 2048).
- With 4 or fewer partitions all of them are primary partitions p1-p4.
- With more than 4 partitions Gaia writes the standard sfdisk extended layout: partitions 1-3 are primary (p1-p3), MBR slot 4 becomes an extended partition (type `0x05`), and partitions 4.. become logical partitions p5, p6, ... Each logical partition has an EBR in the first sector of its aligned slot and its data starts at the next `alignment_lba` boundary. EBR entry 0 describes the logical partition relative to its EBR; entry 1 links the next EBR, relative to the extended partition start, and spans that EBR through the end of its logical partition. The extended entry spans from the first EBR to the end of the last logical partition.
- The disk file is created at its full size with unwritten ranges left as holes, so empty and padded partitions take no space on filesystems that support sparse files.
- Assembly state records each partition's `start_lba`, `sector_count`, image `bytes`, and for layouts with more than 4 partitions the kernel `number` and the `ebr_lba` of logical partitions; empty partitions record `empty=true` and `wipe_bytes`.

#### Archives

`[[image.assembly.archives]]` builds a plain POSIX ustar archive after all disks, written by Gaia itself (no external `tar`):
- `id`: unique archive id.
- `output`: path template, like other assembly outputs; it can point into the image collect dir (`$provider.images/...`).
- `members`: ordered list. Each member has a `name` (path inside the archive) and exactly one of `src` (a path template; the file is copied) or `entries` (a generated file).
- `generated`: ordered list of generated members (`name`, `entries`). They are written **before** all `members`; to interleave generated and file members, put `entries` directly on a `members` item instead.
- `entries`: `[["KEY", "value"], ...]`, rendered as one `KEY=value` line per entry in order. Keys must match `[A-Za-z_][A-Za-z0-9_]*`. Values made only of `A-Z a-z 0-9 _ @ % + = : , . / -` are written as-is; anything else (including an empty value) is wrapped in single quotes with embedded `'` written as `'\''`, so the file can be sourced by `sh`. Values cannot contain newlines.
- Entry values may contain `${assembly.sha256:<path template>}`, replaced at assembly time by the lowercase hex sha256 of that file (an assembly output such as a transform result, or any other file). The file must exist when the archive is built. Normal config interpolation such as `${build.version}` runs first and may also appear inside the path (`${assembly.sha256:$assembly.out/${build.name}.img}`). Any other `${...}` left in a value is a validation error.

Archive output is deterministic: entries appear in the configured order with mtime 0, uid/gid 0, empty user and group names, and mode `0644`. Member names must be relative paths of at most 100 bytes (the ustar name field; prefix splitting is not used) without `.`/`..` components, and must be unique per archive. The archive is written to a temporary file and published atomically; state records `archives.N.output`, `bytes`, `sha256`, and per-member name, source, size, and sha256. Archive outputs are treated as expected outputs, so a bundle in the collect dir is not reported as a large unexpected file. Reuse fingerprints cover the archive config and the state of every member and digest input file.

#### Example: CM5 eMMC A/B layout with an update bundle

A Compute Module 5 eMMC image with a small autoboot partition, A/B boot and root slots, a data partition, and a `.pdupdate` bundle containing a manifest and zstd-compressed slot images:

| Partition | Content |
| --- | --- |
| p1 | 16M FAT holding only `autoboot.txt` |
| p2, p3 | boot A/B, 128M FAT each, both from `boot.vfat` |
| p4 | extended partition |
| p5, p6 | root A/B, 2G ext4 each; A from `rootfs.ext4`, B empty and wiped |
| p7 | `/data` ext4 from a small image |

```toml
build_name = "helios"
version = "1.2.3"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"

[[image.assembly.trees]]
id = "autoboot"
path = "$assembly.work/autoboot"

[[image.assembly.files]]
tree = "autoboot"
src = "@assets/autoboot.txt"
dest = "autoboot.txt"

[[image.assembly.filesystems]]
id = "autoboot"
kind = "vfat"
source_tree = "autoboot"
output = "$provider.images/autoboot.vfat"
size = "16M"

[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/boot.vfat"
dest = "$assembly.work/boot.vfat.zst"

[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/rootfs.ext4"
dest = "$assembly.work/rootfs.ext4.zst"
level = 19

[[image.assembly.disks]]
id = "emmc"
output = "$provider.images/emmc.img"
partition_table = "mbr"

[[image.assembly.disks.partitions]]
name = "autoboot"
type_alias = "fat32-lba"
bootable = true
image = "$provider.images/autoboot.vfat"
size = "16M"

[[image.assembly.disks.partitions]]
name = "boot-a"
type_alias = "fat32-lba"
image = "$provider.images/boot.vfat"
size = "128M"

[[image.assembly.disks.partitions]]
name = "boot-b"
type_alias = "fat32-lba"
image = "$provider.images/boot.vfat"
size = "128M"

[[image.assembly.disks.partitions]]
name = "rootfs-a"
type_alias = "linux"
image = "$provider.images/rootfs.ext4"
size = "2G"

[[image.assembly.disks.partitions]]
name = "rootfs-b"
type_alias = "linux"
size = "2G"
wipe = true

[[image.assembly.disks.partitions]]
name = "data"
type_alias = "linux"
image = "$provider.images/data.ext4"

[[image.assembly.archives]]
id = "update"
output = "$provider.images/${build.name}-${build.version}.pdupdate"

[[image.assembly.archives.members]]
name = "manifest.env"
entries = [
  ["MODEL", "cm5"],
  ["VERSION", "${build.version}"],
  ["OS", "HeliOS"],
  ["BOOT_SHA256", "${assembly.sha256:$assembly.work/boot.vfat.zst}"],
  ["ROOTFS_SHA256", "${assembly.sha256:$assembly.work/rootfs.ext4.zst}"],
]

[[image.assembly.archives.members]]
name = "boot.vfat.zst"
src = "$assembly.work/boot.vfat.zst"

[[image.assembly.archives.members]]
name = "rootfs.ext4.zst"
src = "$assembly.work/rootfs.ext4.zst"
```

The bundle `helios-1.2.3.pdupdate` is a tar of `manifest.env` (`MODEL`, `VERSION`, `OS`, `BOOT_SHA256`, `ROOTFS_SHA256`) followed by `boot.vfat.zst` and `rootfs.ext4.zst`, where each digest is the sha256 of the compressed member in the bundle.


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
