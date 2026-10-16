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

The image `kind` (`kind = "buildroot"` or `kind = "starting-point"`) may be set
by the build file or by any layer it extends. A layer that only adds
`[image.assembly]` or `[image.rootfs]` tables can leave `kind` out; it merges
under the kind the build file sets. A build that has an `[image]` table and in
which no file sets `kind` fails with an error that names `image.kind`.

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

## When Conditions

A `when` table selects an import, an artifact, an install, or a stage item.
Its keys all have to match (AND); `all`, `any` and `not` combine nested
conditions:

- `target`, `profile`, `branch`: the build's value (`${build.*}` is allowed
  in the expected value of an import's `when`)
- `image_kind`: `buildroot` or `starting-point`
- `input = { name = "value", ... }`: each named input must have that value,
  compared as a string. A boolean or integer is written as it reads in TOML
  (`input = { flag = true }` matches the input value `"true"`). The names
  must be inputs some file of the build declares; any other name is an
  error.

Inputs are the values the build selects: the declared defaults, then the
selected preset's `input` overrides (the variant's `preset`/`presets` for a
variant), then `--set`, then a variant's overrides. Import `when` conditions
see the entrypoint's selection, also from files they import, so
`{ path = "styx.toml", when = { input = { backend = "styx" } } }` follows
`--set input.backend=styx`, a preset, or the variant that sets it. The presets
that set those inputs must be defined in the entrypoint file.

```toml
imports = [
  { path = "raze/photonvision-styx.toml", when = { input = { backend = "styx" } } },
]

[[artifacts]]
id = "styx-helper"
# ...
when = { input = { backend = "styx" } }
```

Each variant resolves its own imports, so two variants with different
`backend` values get different layers.

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

## Variants

One build file can make several images from one shared base. A variant is
a set of overrides, in the same `key = value` form as `--set`:

```toml
[inputs.pv]
kind = "enum"
default = "2027"
choices = ["2026", "2027"]

[[variants]]
id = "pv2026"
description = "PhotonVision 2026"
overrides = [
  ["input.pv", "2026"],
]
```

A variant can also apply presets, like `--preset` does for one variant. Its
presets replace the command line's `--preset` for that variant; its own
`overrides` then apply on top:

```toml
[presets.pv2026]
overrides = [
  ["input.pv", "2026"],
  ["input.raze_photonvision_rev", "d11961bd119e2c22101fa9cfe7a7b01f8436aabc"],
]

[[variants]]
id = "pv2026"
preset = "pv2026"            # or presets = ["a", "b"], applied in order
```

Running the file builds the base config as variant `default` and then each
declared variant, in order. `gaia run --variant pv2026` builds only that one
(`--variant default` names the base; several ids may be given, comma
separated). Without `--variant` every variant is built.

Rules:
- Ids use `[a-z0-9][a-z0-9-]*`, are unique, and `default` is reserved.
- An override key must be one Gaia knows, or `input.<name>` for an input the
  build declares. Unlike `--set`, a misspelled key is an error.
- Override order, last wins: the base file and the selected preset (or the
  variant's `preset`/`presets`), the variant's overrides, then the command
  line's `--set`. A `--set` therefore applies to every variant and beats
  their overrides.
- A `preset` or `presets` entry must name a preset the build defines.

What the variants share:
- An operation that is the same in every variant, and depends only on shared
  operations, runs once. In the first variant that needs it, and later
  variants reuse it. That covers sources and the Buildroot base
  (`image:prepare`), and any artifact whose inputs do not change.
- A source a variant checks out differently (its rev, say) is checked out
  per variant, under `build/sources/variants/<id>/<source>`. Everything that
  reads the source sees the variant's checkout, including
  `${source.<id>.path}` and `${source.<id>.commit}`. Artifacts built from it
  run per variant; artifacts built from the other sources stay shared.
- Everything else runs per variant. Each variant gets its own artifact
  outputs (`variants/<id>/` next to the configured path), its own install and
  stage records, its own image build, its own default image collect dir
  (`<out>/images/buildroot/<id>`) and its own reports (`<build>@<id>.*`).

Limits:
- A source that the Buildroot base (`image:prepare`) or the image reads must
  be the same in every variant, and so must the Buildroot base itself. A
  difference there is an error; it belongs in a separate build.
- A shared operation records the build's identity (version, target, profile,
  branch) in its state. A variant that changes that identity therefore makes
  the shared operations rebuild in each variant, instead of being reused.
  Keep identity the same across variants when the shared work should be
  reused.
- Variants that would publish the same image file (same collect dir, export
  dir, image name and version, or assembly output) are an error. Give each a
  different `input` that changes the image name or version, or its own
  `image.output.archive_name`.
- Variants run one after another in one process. A failing variant stops the
  run; the variants before it keep their outputs.

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
rollback_completed = false
preserve_failed_outputs = false
rollback_domains = ["sources", "artifacts", "installs", "stage", "images", "checkpoints"]
keep_going = false
```

Meaning:
- `rollback_on_error` (default `true`)
  On failure or cancellation, clean the failed operation's own partial outputs.
  Completed operations' outputs are kept and recorded for reuse. Set `false` to
  leave all current-run outputs in place.
- `rollback_completed` (default `false`)
  Also unwind every completed current-run output on failure or cancellation,
  within `rollback_domains`. This was the default before finished work was kept.
  Only meaningful when `rollback_on_error = true`. Override with
  `--set policy.failure.rollback_completed=true`.
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

Rust, Git and Java have extra specialized fields:
- Rust: `allow_nested_build`, `batch_builds`, `shared_target_dir`
- Git: `allow_remote_resolution`
- Java: `gradle_home` (see [Java and Gradle](#java-and-gradle))

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

`shared_target_dir` (default `false`) builds the nested cargo artifacts of
every source into one cargo target directory per toolchain, target triple,
profile and execution backend, under the user cache:
`<user cache>/cargo-target/<key>` (`$GAIA_CACHE_DIR`, else
`$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`). Sources built with the same
toolchain then reuse each other's compiled dependencies. By default each
source builds into `<source>/.gaia/cargo-target` and compiles its dependencies
again. Docker builds mount the shared directory at its own path. The build log
names the directory in use.

Cargo identifies a package by name and version, so two sources cannot share
one directory when they have a local (path or workspace) package with the same
name and version in different directories: cargo would reuse the first
package's outputs for the second. A source whose local packages collide with
packages already built from another directory keeps its own directory, and the
build log says why. Registry and git packages are shared without this check.
Gaia lists a source's packages with `cargo metadata`, run on the host; if that
fails, the source keeps its own directory.

Notes: concurrent builds in one shared directory wait for cargo's build lock,
and that wait counts toward `timeout_seconds`. The directory is not cleaned up
automatically; delete it under the user cache to reclaim space. Build outputs
and recorded artifact state do not depend on the directory, so switching the
setting does not invalidate recorded artifacts. It is opt-in for now.

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
shared_target_dir = true  # opt-in; see above
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
  It also splits the `make`: Gaia builds up to `target-finalize`, digests
  everything the filesystem image step reads (the finalized target tree by
  content, the images packages installed such as the kernel and device
  trees, the whole `.config`, the post-image and fakeroot scripts, users and
  device tables and the directories they are in, Buildroot's `fs/` and
  `support/scripts`, and the host packages built), and runs the rootfs
  images and post-image script only when that digest differs from the one
  recorded with the current images, or an expected image is missing. A
  rebuild where nothing those read changed skips EROFS, ext4, squashfs and
  genimage entirely.
- Turning `ccache.enabled` or `parallel_packages` on or off changes the
  toolchain wrapper or the tree layout, so the next build cleans the output
  tree once.

#### Work directory

```toml
[providers.buildroot]
work_dir = "disk"                 # "disk" (default), "ram", or a directory
ram_budget = "60G"                # most RAM a "ram" tree may use
keep_ram_tree = true              # keep the tree after a build for fast rebuilds
```

`work_dir = "ram"` builds the output tree on tmpfs under `/dev/shm`, which
makes compiles and tree writes much faster. The tree is kept for rebuilds
unless `keep_ram_tree = false`, and the build falls back to disk when RAM is
short (`ram_budget` caps the RAM the tree may take).

#### Host tools

```toml
[providers.buildroot.host_tools]
default = "build"                 # policy for tools not listed below
ccache = "system,build"           # per tool
pkgconf = "system,build"
```

Each policy is a comma-separated, non-empty list of steps tried in order:
`system` uses the build environment's own tool, `build` uses the tool
Buildroot builds for itself, and `fail` stops the build with an error naming
the tool and what was required. `"system,build"` uses the system tool when
one is usable and builds it otherwise; `"build,system"` prefers the built
tool. A `--set` value with an unknown step is rejected. In the TOML file, an
invalid policy compiles to `fail` (the build stops rather than silently
choosing a default), and tool names other than `ccache` and `pkgconf` are
ignored.

`system` currently applies to `ccache` and `pkgconf`. For `ccache`, Gaia uses
the build environment's `ccache` 4.x through a wrapper in `HOST_DIR/bin` that
points it at `BR2_CCACHE_DIR`, and removes Buildroot's `host-ccache` and its
`host-zstd`, `host-hiredis`, `host-xxhash` and `host-blake3` dependencies.
The decision and the system tool's version enter the package cache keys, so
switching a tool between `system` and `build` does not reuse packages built
the other way.

For `pkgconf`, Gaia uses the build environment's `pkgconf` 1.8 or newer and
keeps Buildroot's own `pkg-config` wrapper (the same install and `sed` steps,
with the static or shared variant), so the wrapper's environment and flags
are unchanged. Only `HOST_DIR/bin/pkgconf`, the binary the wrapper runs, is
replaced by a script that executes the system `pkgconf`; `host-pkgconf`'s
configure, build and other installed files are skipped. The system's
`pkg.m4` is copied into `HOST_DIR/share/aclocal` when present, so
`PKG_CHECK_MODULES` in host autotools packages still works. Two things differ
from Buildroot's own tool: Buildroot's pkgconf 2.3.0 carries a patch that
applies the sysroot only to a few variables (`includedir`, `libdir`, ...),
which the system pkgconf does not do, so a `pkg-config --variable` of another
path can come out prefixed with the staging directory; and the system's
`pkgconf` may differ in its defaults. The version floor is the oldest one
whose options the wrapper uses (`--keep-system-libs`, `--static`) and the
build image's 1.8.1 has.

#### Package cache

```toml
[providers.buildroot]
parallel_packages = true          # required

[providers.buildroot.package_cache]
enabled = true
# level = "system"                # default level for packages: "system" | "project"
# system_dir = "/srv/buildroot-packages"   # default: see below
# project_dir = ".gaia/cache/buildroot/packages"
# project_packages = ["photonvision*", "my-app"]  # kept in this project only
# system_packages = []            # shared even when level = "project"
# max_size = "100G"               # per level; unset: see "Size budget" below
```

Built packages are cached by content, so a wiped output tree or a second
image with the same kernel, Mesa or libcamera restores them instead of
compiling. After a
successful `make`, each package built in that run is stored as a
directory: the files it added to its per-package directories (everything
that is not a hard link to the same path in a direct dependency's trees, so
also what it installs outside its install steps, such as an extracted
external toolchain), its image files, file lists, kconfig `.config` and
stamps. Files are cloned with `cp --reflink=auto`: when the cache is on the
build's filesystem and that supports reflinks (btrfs, XFS), storing and
restoring copy no file data.

The cache has two levels:

- the **system level**, shared by every project of the user:
  `system_dir`, by default `<user cache root>/buildroot/packages` (the user
  cache root is `$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else
  `~/.cache/gaia`). When that default is on another filesystem than the
  build, it would copy every file, so its packages go to the project level
  instead (with a note in the run output); set `system_dir` to a directory on
  the build disk to share packages between projects.
- the **project level**: `project_dir`, by default
  `<workspace>/.gaia/cache/buildroot/packages`.

Packages are stored at `level` (default `system`), except those matching
`project_packages` (stored at the project level, for the project's own,
frequently changing packages) or `system_packages` (stored at the system
level when `level = "project"`). Restores look in the project level first,
then the system level. `dir` is accepted as an alias of `system_dir`.

**Size budget.** Each level is kept at its `max_size` (least recently used
packages are evicted first, until the level is at nine tenths of it). Unset,
a level's budget is computed when a run starts, from the free space `F` on
the filesystem holding that level and the bytes `C` the level already holds:

    budget = clamp(F / 2 + C, 2 GiB, 40 GiB)

so an existing cache is not over budget merely for being there, and a nearly
full disk gets a small cache. When the free space cannot be read, the budget
is 40 GiB. Eviction runs at the start of a run (when a lowered `max_size` or
a large cache is over its budget) and after packages are stored; each
eviction is reported in the run output with its count and size. A `max_size`
that is set is only warned about when it is larger than the free space; the
default never warns.

`gaia cache` manages the cache entry by entry, so one bad package never
means wiping everything:

```sh
gaia cache build.toml                          # list entries, largest first, with totals per level
gaia cache build.toml --level project --package 'mesa*'
gaia cache build.toml --remove mesa3d          # every cached build of a package
gaia cache build.toml --remove linux@ab12cd    # one build, by key prefix
gaia cache build.toml --clear project          # a whole level (or system, or ccache)
gaia cache build.toml --remove mesa3d --dry-run
```

To build without the cache for one run, pass
`--set policy.providers.buildroot.package_cache.enabled=false`.

The keys ignore Gaia's own patches to Buildroot (such as the reflink copy of
per-package directories), so a build with and without them shares entries.

Before the next `make`, packages not yet built are restored,
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
built. Before each `make`, the stamps of every built package whose key is
unchanged are made newer than its inputs, so sources copied again (newer
files, same content) neither rebuild packages nor break restored kconfig
packages, which have no sources to redo a step from. In text files that hold the output tree's path, it is rewritten on
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
- A setting that belongs to no package (an external tree's own option, for
  example) is looked up in what reads it:
  - Buildroot's infrastructure (`Makefile`, `package/Makefile.in`,
    `package/pkg-*.mk`, `toolchain/`, `arch/`, `system/system.mk`), an
    architecture or CPU choice, or an `external.mk` using it for something
    Gaia cannot follow (an `include`, a rule, a global variable such as
    `TARGET_CFLAGS`): the whole output tree is cleaned.
  - Package `.mk` files, or `<PKG>_*` variables and hooks an `external.mk`
    sets from it (directly or through its own variables): those packages
    are rebuilt.
  - Only target finalization: `BR2_ROOTFS_OVERLAY`,
    `BR2_ROOTFS_POST_BUILD_SCRIPT`, the hostname, issue and root password,
    or an `external.mk` feeding it to `PACKAGES_USERS`,
    `PACKAGES_PERMISSIONS_TABLE`, `PACKAGES_DEVICES_TABLE` or
    `TARGET_FINALIZE_HOOKS`. No package is rebuilt; with per-package
    directories `target/` is removed and reassembled from them, so files of
    a removed overlay go too (without them, the tree is cleaned).
  - Nothing at all: no package is rebuilt.

  Each decision is logged with its reason, for example
  `BR2_RAZE_LEMNOS_USERS_TABLE unset -> "/x/lemnos-users.table": only read
  when finalizing the target, no package rebuilt`.
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

### Java and Gradle

`[providers.java] gradle_home` chooses where Gradle's user home (its
dependency and wrapper caches) lives for Java artifacts built in Docker:

- `"workspace"` (default): `<workspace>/.gaia/docker-home/.gradle`, kept per
  workspace.
- `"user-cache"`: `<user cache root>/gradle-home`, in the per-user Gaia cache
  (`$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`) and
  shared by every workspace of the user. Gaia mounts it into the container at
  the same path.

In `user-cache` mode, Docker builds also get a persistent `HOME`
(`<user cache root>/java-home`, mounted at the same path), so the caches of
tools the build runs under `HOME` (pnpm's store, npm, Python virtualenvs)
survive the `--rm` container. A build whose `build_env` sets its own
`GRADLE_USER_HOME` or `HOME` outside `.gaia/docker-home` keeps it; one that
sets none, or one inside `.gaia/docker-home`, gets the user-cache paths. Host
builds keep their own homes. The first build in `user-cache` mode is cold:
Gradle and the other tools download their caches again.

```toml
[providers.java]
gradle_home = "user-cache"
```

Deprecated: the environment variable `GAIA_GRADLE_HOME=workspace|user-cache`
still overrides `gradle_home` when it is set, so existing setups keep working
during the transition. Any other value fails the build. Set
`[providers.java] gradle_home` (or `--set policy.providers.java.gradle_home=...`)
instead.

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
- `export_dir`
  Optional default directory for `gaia run --export` in every build of the
  workspace. A build's own `image.output.export_dir` takes precedence. See
  [Image output](#image) and [`gaia run --export`](cli.md#exporting-the-image).

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

#### Git source credentials

Git sources run their git commands (mirror clone and fetch, `ls-remote`,
clone, checkout) on the host with your own environment: git config,
credential helpers (for example `gh auth setup-git`), the ssh agent and
`GIT_*` variables. This holds even when the build runs in docker, which is
why a private repository works with `[policy.execution] docker`. Builds
still run in docker; only git runs on the host.

- `git_backend = "host" | "execution"` on a `kind = "git"` source. The default
  is `"host"`. `"execution"` runs the git commands through the execution
  backend (docker, when configured) as older versions did.
- If `git` is not installed on the host, a `"host"` source falls back to the
  execution backend and the build log says so.
- Git runs with `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never`: a
  missing credential fails at once with the source id and the redacted URL,
  and the error suggests a credential helper or an ssh key. It does not
  prompt or hang. ssh keys that need a passphrase must be loaded in ssh-agent.
- Credentials in a URL (`https://user:token@host/...`) are never written by
  Gaia. Logs, errors, `.gaia-source-state.txt`, the source marker, the
  provenance and manifest reports and `<build>.gaia.lock` show them as
  `https://***@host/...`. Prefer a credential helper over embedding a token;
  a token in the URL is still stored in the mirror's `.git/config` under
  `.gaia/cache/git` (the checkout's copy is redacted).
- The `git_backend` setting does not change source fingerprints: a build
  upgraded to this version keeps its materialized sources.

**Path source identity.** A path source inside a git work tree is identified by
the files git lists for it: tracked files and untracked files that are not
ignored, with `identity_ignore` and the built-in ignores applied on top. Ignored
build output such as `target/` or `node_modules/` is never read, so validating a
build on a large checkout stays fast. Outside a git work tree the whole directory
is walked as before. The identity of a git-backed path source changed once with
this rule, so its first build after the change may rebuild.

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
- `sources`
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
  - `cargo_git_sources` (table: repository URL as Cargo.lock records it →
    the id of a declared `kind = "git"` source; see
    [Private cargo git dependencies](#private-cargo-git-dependencies))

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

`sources = ["eidos"]` names sources the artifact reads besides its own
`source`. The artifact depends on each of them, so when one re-materializes
with changed content the artifact rebuilds, and the rebuild reason names the
source. A `${source.<id>.path}` or `${source.<id>.commit}` token in a Java
artifact's build fields adds its source automatically (the artifact depends on
it without listing it), so `sources` is for a source the artifact reads some
other way, such as files it opens inside that source. Each name must be a
declared source.

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

#### Private cargo git dependencies

A rust artifact whose workspace depends on a private git repository (`git =
"https://github.com/org/private.git"` in `Cargo.toml`) needs that repository's
credentials when cargo fetches it, and a docker container has none. Map the
dependency to a declared git source instead, and Gaia serves it from that
source's mirror, prepared on the host with your own git credentials:

```toml
[[sources]]
id = "eidos"
kind = "git"
repo = "https://github.com/Prometheus-Dynamics/Eidos.git"
branch = "main"

[[artifacts]]
id = "vision"
kind = "rust"
source = "workspace"
package = "vision"
cargo_git_sources = { "https://github.com/Prometheus-Dynamics/Eidos.git" = "eidos" }
output_path = "${workspace.out_dir}/artifacts/vision"
```

- The key is the repository URL as `Cargo.lock` records it. It also matches
  without `.git`, with a trailing `/`, and with the `git+` prefix or a `?branch=`
  query that the lock adds. The value is the id of a declared `kind = "git"` source.
- The artifact depends on each named source, so a change to it rebuilds the
  artifact. The source's branch or tag does not decide which commits build: the
  lock does. Cargo.toml and Cargo.lock are not changed.
- Before cargo runs, on the host: every commit the workspace's `Cargo.lock`
  pins for the mapped repository must be in the source's mirror under
  `.gaia/cache/git`. A commit the mirror lacks is fetched; if it still cannot be
  found the build fails and names the URL, the locked commit and the source.
  The commit is then kept reachable from `refs/heads/gaia-locked/<commit>` in the
  mirror, because cargo's git fetch only asks for branches. The mirror's refs are
  rewritten on every build.
- Cargo runs with `CARGO_NET_GIT_FETCH_WITH_CLI=true` and git URL rewriting
  (`url.<mirror>.insteadOf`), so it reads the dependency from the mirror, never
  from the network. In docker the mirror is mounted read-only at its own path,
  and the execution image needs `git` (the official rust images have it).
  Entries already in `GIT_CONFIG_COUNT` are kept.
- Each mapping must name a `kind = "git"` source. Two keys naming the same
  repository for different sources, or `cargo_git_sources` on a non-rust
  artifact, are validation errors.
- Rewrites match by prefix, so a locked repository whose URL starts with a
  mapped one (`https://h/a` and `https://h/a-core`) is a build error unless it
  is mapped too.

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
export_dir = "~/images/gaia"
emit_report = true
```

`export_dir` is where a successful `gaia run` copies the primary image (see
[Exporting the image](cli.md#exporting-the-image)). It is set per build here
and falls back to `[workspace] export_dir`. A path that is not absolute and
does not start with `~/` is taken from the workspace root; `~` and `~/...`
expand to the home directory. An empty value is an error. `gaia run --export
<dir>` overrides both, and `gaia run --no-export` skips the configured
export for one run. The `--set` keys are `image.output.export_dir` and
`workspace.export_dir`.

An `archive_name` ending in `.img`/`.raw` publishes the raw disk image
itself; `.img.xz`/`.raw.xz` compresses it with xz (smallest, slow) and
`.img.zst`/`.raw.zst` with zstd (several times faster to compress and
decompress, a little larger). Both use every core and give the same bytes
for any core count. A common split is zstd for development images and xz
for releases, for example
`archive_name = "${build.name}-${build.version}.img.${input.compression}"`.

When the assembly builds more than one disk, a raw disk archive needs
`disk = "<assembly disk id>"` in `[image.output]` to say which disk it holds
(the id must be an `[[image.assembly.disks]]` entry, and the archive name must
be a raw disk image). Without it the run fails and names the disks:

```toml
[image.output]
archive_name = "${build.name}-${build.version}.img.xz"
disk = "sdcard"
```

The run reports every image it publishes (the `image sizes` table and the
summary's `published_outputs`): the primary, the other disks written under the
image output dir, published filesystem images, and archives and transform
outputs written there. `gaia run --export` (see
[Exporting the image](cli.md#exporting-the-image)) copies the primary and those
outputs, except uncompressed raw disks other than the primary, which the
primary archive already holds; compress such a disk with an `xz` transform to
export it.

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

`${source.<id>.commit_short}` is the first 8 characters of that commit; a
`-dirty` suffix is kept, so a short token still shows an uncommitted tree.

Project identity: `${project.commit}` and `${project.describe}` describe the
git repository holding the build file. `${project.commit}` is the full `HEAD`
sha, suffixed `-dirty` when tracked files have uncommitted changes, and
`${project.describe}` is `git describe --tags --always --dirty`. They resolve
during normal interpolation, so they work anywhere `${build.version}` does,
including `version`, `archive_name`, env sets and archive manifest entries.
`${project.commit_short}` is the first 8 characters of `${project.commit}`,
with the `-dirty` suffix kept. Use them to make image versions unique per build:

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
materializes them; with build variants, `<build_dir>/sources/variants/<variant>/<id>`
for a source a variant checks out for itself). Like the commit token it works
in stage env set values and build labels; both tokens also work in Java
artifacts' `build_target`, `build_args`, `build_command` and `build_env`
(for example `build_env = [["EIDOS_PATH", "${source.eidos.path}"]]`) and in
Buildroot `config_overrides` values, so a setting can name a file inside a
source:

```toml
[image]
kind = "buildroot"
config_overrides = [
  ["BR2_ROOTFS_USERS_TABLES", "\"${source.orion.path}/packaging/buildroot/orion-users.table\""],
]
```

Inside `[image.assembly]` every string takes both source tokens: tree ids
and tree or file paths, file `src`, `dest` and `content`, `json` values (keys
are not substituted), archive outputs and members, disk and filesystem
outputs, and the rest. A token there resolves at config time like the others,
so a slot id can carry the commit:

```toml
[[image.assembly.trees]]
id = "slot-${source.app.commit_short}"
path = "$assembly.work/slot-${source.app.commit_short}"

[[image.assembly.files]]
tree = "slot-${source.app.commit_short}"
json = { rev = "${source.app.commit}" }
dest = "slot.json"
```

A `${source.<id>...}` token that stays unresolved there (an unknown source id,
or a git source with no commit yet) is reported by validation, like any other
unresolved token. Each declared source the assembly names is a dependency of
its `image:assembly` step: a changed file inside a path source (or a new
commit of a git source) reruns the assembly, and an unchanged source keeps it
reused.

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
- `file`: any plain file Buildroot writes. The name is not checked against a
  format (`flash-id` is accepted as is), and the defconfig need not enable a
  filesystem for it. Use it for images that Buildroot's post-image scripts
  produce under a name without a format suffix.

Buildroot run notes:
- Config steps are reused. The build operation repeats the prepare operation's
  defconfig, fragments, `config_overrides` and cache settings only when their
  inputs changed: the defconfig and fragment files, the overrides, the cache
  settings Gaia writes, the Buildroot version (`Makefile`) and every Kconfig
  file of the Buildroot and `BR2_EXTERNAL` trees (by content). The state is
  `.gaia-buildroot-config-inputs` in the output tree; a `.config` edited
  outside Gaia is rebuilt. The run message is `buildroot config unchanged
  since the last configuration; config steps skipped`.
- `target-finalize` is reused. Each `make` that reaches it copies every
  package's per-package tree into `target/` and patches the ELF files of
  `host/` (about 40 s on a PhotonVision tree, with nothing to build). After a
  prepare (or build) finalize, the build operation skips it when every
  package is installed, the config is unchanged and nothing has been built
  since; the images step then runs as before. The marker
  `.gaia-target-finalized` is removed when a make may build, and when the
  image feed is applied to `target/`. The run message is `skipped
  target-finalize: ...`.
- `SOURCE_DATE_EPOCH` is set for Buildroot's `make` and its post-build and
  post-image scripts: the caller's own `SOURCE_DATE_EPOCH` when the
  environment sets one, else the commit time of the workspace git `HEAD`
  (stable for a commit, changes with each commit; nothing is set outside a
  git workspace). It is not part of the package cache keys, so a package
  restored from the cache keeps the timestamps of the epoch it was built
  with until it is rebuilt.
- The package cache reports installed packages it cannot key (for example
  packages with local sources) instead of leaving them out silently.

#### Leaving packages out of the rootfs

```toml
[image.rootfs]
exclude_packages = ["libcamera", "libpisp", "opencv4"]   # Buildroot package names
```

One Buildroot tree can build every package while an image leaves some of them
out of its root filesystem, without a second tree. `image:build` removes the
files of the listed packages from `target/` before the image's post-build
scripts and before the filesystem images, so `rootfs.*` does not contain them.
Names are lowercase Buildroot package names; a name listed twice is an error.
`--set image.rootfs.exclude_packages=libcamera,libpisp` sets the list (a
variant can set it the same way). Layered configs concatenate their lists.
The image:prepare operation does not apply it.

Checks, before anything is removed:
- each name is a target package of the Buildroot tree: an unknown name and a
  `host-*` package are errors;
- no kept target package depends on an excluded one, directly or through
  other packages. The error names the chain, for example `app -> libcamera ->
  libpisp`. `skeleton` and the toolchain are not counted.

After the make, before the images:
- every removed path is gone, and no object left in `target/` needs a shared
  object name that only a removed file provides. The check reads each ELF
  object's `DT_NEEDED` and `DT_SONAME`.

Run notes:
- Only the files `build/packages-file-list.txt` lists for an excluded package
  are removed. A path a kept package also lists stays and is reported.
  Directories go when the removal empties them.
- The exclusion needs `policy.providers.buildroot.parallel_packages` (per-package
  directories): without them Buildroot copies the files back into `target/`
  on every finalize, and the build refuses. It is refused with shared
  Buildroot output, which one tree serves to every image.
- The checks do not see runtime loads that are not `DT_NEEDED` entries, such as
  a library opened by path with `dlopen` or a plugin directory. Leave those
  packages in, or remove what loads them too.
- The run message is `rootfs: left out N packages (...): M files, X MiB`;
  `gaia preview` shows the names, the checks on the last build's graph, and the
  files that build lists for them.

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
- `strip = true` on a tree strips every ELF file in it right before a filesystem or archive packs the tree (the steps that fill the tree have run by then). Executables and shared libraries lose their symbols (`strip --strip-unneeded`), kernel modules (`*.ko`) lose only their debug data (`--strip-debug`), and relocatable objects and non-ELF files are left alone. Only the copies in the tree change, never a source. Hard links are stripped once and stay linked, symlinks are left as they are, and a file that is already stripped is skipped. The run state and summary record the files stripped and the bytes saved per tree.
- `strip_tool = "<program>"` (under `[image.assembly]`) names the strip program every `strip = true` entry uses: a program name looked up in the provider's host tools and then on `PATH`, or a path (it may use `$provider.*` tokens). Unset, the tool is a `<triplet>-strip` in the provider's host tools matching the ELF machine (`aarch64-*-strip`, `arm-*-strip`, `x86_64-*-strip`), else `llvm-strip` on `PATH`, else a plain `strip` when the file is for the host's own machine. When none is found the build fails, naming the file and these options.
- File entries use exactly one of `src`, `src_glob`, `content` or `json`. `content` and `json` write a generated file (see [Generated content](#generated-content)).
- `exclude = ["dwc2.dtbo", "i2c*.dtbo"]` on a `src_glob` entry leaves out matches by file name (simple `*` globs allowed). Use it to copy a stock set except the names a later entry provides, so the result does not depend on copy order. It is refused on other entries.
- Glob staging is deterministic and currently supports simple non-recursive `*` filename patterns.
- A `dest` of `.` or ending in `/` is a directory the sources are copied into; any other `dest` is the file a single source is copied to. A `src_glob` that matches several files needs a directory `dest` (otherwise every match would land on one file) and fails without one.
- `optional = true` skips missing sources without failing the build and records the skip in runtime state.
- A copied file that is dynamically linked (it names a program interpreter or `DT_NEEDED` libraries) is checked against its tree. Once the steps that fill the tree have run, before a filesystem or archive packs it, each interpreter and library the file needs must be in the tree, looked up as [listed binaries](#listed-binaries) look them up. A missing one is a warning, not an error: the run output and the summary name the file and the libraries (`copy it with [[image.assembly.binaries]] to bring its libraries`). A tree may be an overlay on a full rootfs, which is why it is not an error. `allow_missing_libs = true` on the entry accepts the file without the warning. The check reads only ELF headers, never the host, and a static or non-ELF file is never checked. `allow_missing_libs` is refused on `content` and `json` entries.
- `mode = "0755"` applies Unix file modes on Unix hosts.
- `include = ["<name>", ...]` on a `src_glob` entry copies only the matches whose file name one of the names matches (simple `*` globs allowed); it is applied after `exclude`. An entry that matches no file fails the build, naming it, unless `optional = true`. It is refused on other entries. For example, twelve named overlays out of a directory of several hundred: `src_glob = "$provider.target/boot/overlays/*.dtbo"`, `include = ["vc4-kms-v3d.dtbo", "i2c*.dtbo"]`, `dest = "overlays/"`.
- `strip = true` on a file entry strips each copied ELF file (the `src`, or each `src_glob` match) with the strip tool described under the tree; a copied file that is not ELF is left alone. It is refused on `content` and `json` entries.
- Transform kinds currently implemented are `copy`, `gzip`, `zstd`, `xz`, `compile-dts`, and `verity` (see [dm-verity and ext4](#dm-verity-and-ext4)).
- `gzip` runs `gzip -n -c [-<level>] <src>`. `-n` leaves the name and the modification time out of the header, so the same input and level give the same bytes on every run. The optional `level` (1-9) is accepted on `gzip` transforms and defaults to gzip's own.
- `zstd` runs `zstd -q -c --no-progress -T0 [-<level>] <src>`, resolving `zstd` from provider host tools first, then host `PATH`; the tool path and version are recorded in assembly state. The optional `level` (1-19) is only accepted on `zstd` transforms and defaults to zstd's own default.
- `xz` runs `xz -T0 --block-size=24MiB -c [-<level>] <src>`, resolved like `zstd`. Its fixed block size makes the output the same for any core count. The optional `level` (0-9, xz's preset) is only accepted on `xz`, `zstd` and `gzip` transforms, as `gzip`'s and `zstd`'s own levels are. Use it to compress another disk into the output dir, for example `kind = "xz"`, `src = "$assembly.work/slots/<id>.img"`, `dest = "$assembly.out/<id>.img.xz"`.
- A transform, verity, archive or disk output written under `$assembly.out` is a published output of the run: it is listed in the summary and copied by `--export` (see `image.output.disk` above for raw disks).
- `compile-dts` resolves `dtc` from provider host tools first, then host `PATH`.
- `busybox_initramfs` copies the configured BusyBox binary to `bin/busybox` and creates the requested applet symlinks in `bin`. With `include_runtime_libs = true` it also copies the dynamic loader and the shared libraries the binary needs, from the target's sysroot (see the next bullet). Each entry takes:
  - `tree`, `busybox`, `applets`, `include_runtime_libs`: as above.
  - `sysroot` (optional): the target root the runtime libraries are read from, as a path template such as `$provider.target`. Unset, it is derived from `busybox`: the directory above its `bin/` or `sbin/`, or two levels up from `usr/bin/` or `usr/sbin/`. A dynamic binary whose sysroot cannot be derived fails the build, asking for `sysroot`.
- `binaries` copies a binary into a tree together with the runtime it needs (see [Listed binaries](#listed-binaries)).
- `kernel_modules` copies the named kernel modules, and the modules they depend on, from a kernel's `modules.dep` into a tree under `lib/modules/<kernel version>/` at the same relative paths. It also copies `modules.order`, `modules.builtin` and `modules.builtin.modinfo` when present, then runs `depmod -b <tree> <kernel version>`, so the tree's module index covers exactly the copied set. Each entry takes:
  - `tree`: the tree to fill.
  - `from`: the directory holding the `<kernel version>` directories, such as `$provider.target/lib/modules`.
  - `modules`: module names. `-` and `_` are the same, and a name may carry `.ko`, `.ko.xz`, `.ko.zst` and similar suffixes or a path.
  - `kernel_version` (optional): the version directory to use. Required when `from` holds more than one.
  - `depmod` (optional): a path template for `depmod`. Unset, it takes the provider's `sbin/depmod`, then the host's `depmod` from `PATH`; a missing depmod fails the build.
  - `metadata` (optional): the `modules.*` files to install, such as `["modules.dep", "modules.alias"]`. Each name must start with `modules.`. Unset, the default set is installed (`modules.order`, `modules.builtin` and `modules.builtin.modinfo`, when present). Set, only the listed files are installed, and each must exist in the kernel's module directory (a missing one fails the build). depmod still writes its indexes into the tree; the `modules.*` files it wrote that the list does not name are then removed, so `.bin` indexes and `modules.builtin.modinfo` are left out.

  A module the kernel has built in (listed in `modules.builtin`) is skipped and noted. A name that is neither in `modules.dep` nor built in fails the build, naming it. The step runs with the other tree-populating steps, before the filesystems that pack the tree.
- Runtime libraries are found from the binary's ELF headers, never from the host: the program interpreter, the `DT_NEEDED` names and the `DT_RUNPATH` (or `DT_RPATH`) entries are read from the file, and each name is looked up under the sysroot. The search order is the binary's RUNPATH/RPATH entries (`$ORIGIN` is expanded; an entry with another `$` token, or a relative path, is skipped), then `lib`, `lib64`, `usr/lib`, `usr/lib64`, the `<triplet>` directories present under `lib` and `usr/lib`, and the directories of `etc/ld.so.conf` and its `include` files. A library is used only when its class, byte order and machine match the binary, so a host library is never taken for a target one.
- Symlinks inside the sysroot are followed (an absolute link target means the sysroot's root). The tree gets each file the loader reaches as a real copy at its absolute path, and each symlink crossed on the way as a symlink with the same target text, so a soname such as `libc.so.6` resolves in the tree as it does on the target. The interpreter is placed at its exact path. A static binary copies nothing. A NEEDED name that is not found under the sysroot fails the build, and the error lists every missing name. The tree must not create a directory that the sysroot links (for example `lib` when the sysroot has `/lib -> usr/lib`), since the link cannot replace it.
- For a cross-compiled BusyBox taken from a Buildroot target tree, set `sysroot = "$provider.target"` (or leave it unset when the binary sits at `$provider.target/bin/busybox`). The reuse fingerprint covers the sysroot, the interpreter, every symlink and the digest of every library file, so a changed libc rebuilds the assembly.
- Filesystem kinds currently implemented are `vfat`, `cpio`, `cpio-gzip`, `cpio-zstd`, `cpio-xz`, and `ext4` (see [dm-verity and ext4](#dm-verity-and-ext4)).
- `cpio` kinds write a newc archive with uid and gid 0, entries in sorted order and zero timestamps, so the same tree packs to the same bytes. `cpio-zstd` compresses that archive with `zstd -q -c --no-progress -T0 -<compression_level>`, resolving `zstd` like the zstd transform; `compression_level` (1-19) is only accepted on `cpio-zstd` and defaults to 19.
- `cpio-xz` compresses the same newc archive with `xz -T1 --check=crc32 --lzma2=preset=<compression_level>,dict=<size> -c`. The CRC32 check is the one the kernel's XZ initramfs decoder reads. That decoder allocates the whole LZMA2 dictionary, so the dictionary is capped at 1 MiB (or the cpio's size rounded up to 4 KiB, when smaller) whatever the level: the 64 MiB dictionary of `-9` gives an initramfs the kernel cannot unpack. `compression_level` (0-9) is accepted on `cpio-xz` (default 6) and `cpio-zstd` (1-19, default 19 there). One thread writes one block, so the output is the same on every run.
- `publish = true` on a filesystem also copies its image to the image output dir (`$assembly.out`, the collect dir by default) under the output's file name, and the published image is reported like a raw disk: in the run summary, its image sizes, and the content digests. The name must not collide with another output or published copy. In a RAM work dir the image is built in RAM and copied to the image output dir when built.
- `vfat` uses `mtools` (`mformat` and `mcopy`), resolving provider host tools first, then host `PATH`; `size` accepts byte values or binary `K`, `M`, `G`, and `T` suffixes, or `size = "auto"`. Without `size` the image is 32M.
- `free = "256M"` (only with `size = "auto"`) leaves that much free space beyond the content estimate: the margin becomes the larger of the automatic margin and `free`, and the result is still rounded up to a whole MiB. Use it when files will be added to the image later.
- `size = "auto"` (vfat and ext4) sizes the image from its source tree; the vfat rule is: each file is counted rounded up to 4 KiB, each directory as 4 KiB, plus 1 MiB for the FAT tables and root directory, plus a margin of 5% (at least 1 MiB), rounded up to a whole MiB. The size depends only on the tree's content, so the same tree always gets the same size. The 4 KiB per entry is an estimate of the cluster size mtools picks: a tree that overruns it fails while copying, rather than being truncated.
- Raw MBR disk assembly writes 1 MiB-aligned partitions sequentially (see [Disk partitions](#disk-partitions)).
- MBR partition `type` accepts raw `0xNN` values; `type_alias` currently supports `fat32-lba` and `linux`.
- Assembly runtime state is included in provenance and manifest reports with staged file, transform, filesystem, disk, and archive output sizes and digests.
- Assembly order follows dependencies: a step that reads a path another step writes (the same file, a file in a directory it fills, or a glob it matches) runs after it, whatever its kind or position, so for example a transform compressing a filesystem built by the same assembly sees this run's image. Trees are prepared first; steps that do not depend on each other run in the order dirs, symlinks, files, BusyBox initramfs, kernel modules, transforms, filesystems, disks, archives. Steps that read each other's outputs in a cycle are rejected, by validation and at run time.
- Before the steps run, outputs of transforms, filesystems, disks and archives left from an earlier run are removed, so a step reading one before it is rebuilt fails instead of using a stale file.

#### Work directory and RAM

`[image.assembly] work_dir` takes a path (the intermediates' directory, as
above), `"disk"` (the build dir's `assembly` directory, the default) or
`"ram"`.

With `"ram"` the intermediates go to tmpfs under
`/dev/shm/gaia-<user>/<hash>/assembly`: the trees, filesystem images such as
`boot.vfat`, transform outputs under the work dir, and raw disk images. The
published outputs stay on disk. The archive is compressed from the RAM copy
of the raw disk. Every raw disk image is built in RAM and copied to its spec
path on disk when built; the copy is sparse, so the zeros of an unwritten
partition cost no disk writes. The RAM copies are removed when
the assembly ends, whether it succeeded or not.

In RAM mode a filesystem image such as `boot.vfat` is an intermediate and is
not left in the collect dir; use `"disk"` to keep it there, or set `publish = true`
on the filesystem to copy it to the image output dir.

When `work_dir` is unset it follows `[providers.buildroot] work_dir`: RAM when
that is `"ram"`, disk otherwise. A path, such as the PhotonVision project's
`${workspace.build_dir}/assembly`, keeps the intermediates on disk even when
the Buildroot tree is in RAM; set `work_dir = "ram"` to move them.

RAM is used only when the expected size of the intermediates fits in
`MemAvailable` and in the free space on `/dev/shm` with 4 GiB to spare. The
expected size is an upper bound from the spec: the disks' layouts, the
filesystems' `size` and the transform inputs. Otherwise the assembly runs on
disk and the run says why (`assembly work dir falls back to disk: ...`).

The assembly state records `work_dir.placement` (`ram` or `disk`),
`work_dir.path` and `work_dir.expected_bytes`.

#### Disk partitions

`[[image.assembly.disks.partitions]]` fields:
- `name`: label used in state and errors.
- `type` (raw `0xNN`) or `type_alias` (`fat32-lba`, `linux`); defaults to `linux` (`0x83`).
- `bootable`: sets the MBR active flag. Only allowed on primary partitions; validation rejects it on logical partitions (`assembly_partition_bootable_logical`).
- `image`: file written at the partition start. Optional when `size` is set.
- `size`: partition size in bytes or with a binary `K`/`M`/`G`/`T` suffix (`"16M"`, `"2G"`). Without it the partition is exactly as large as its image (rounded up to 512-byte sectors). With it the image must fit, otherwise assembly fails before the disk is written; the space after the image is left unwritten.
- `wipe = true`: only for empty partitions (no `image`). Writes zeros over the first 1 MiB of the partition (less if the partition is smaller) so filesystem signatures left by a previous flash do not survive when the image is written to a device. Validation rejects `wipe` together with `image`, since an image already overwrites the partition start.
- `materialize = false`: the partition is in the table at its full `size`, but nothing is written to it (no image, no wipe). It needs `size`; validation rejects `image` or `wipe` with it (`assembly_partition_unmaterialized_image`, `assembly_partition_unmaterialized_wipe`). Pair it with `truncate = "last-data"` to leave the partition past the end of the file so it is created on first boot. Defaults to `true`.

Layout:
- Partitions are placed in declaration order, each starting at the next `alignment_lba` boundary (default 2048 sectors) after `first_lba` (default 2048).
- With 4 or fewer partitions all of them are primary partitions p1-p4.
- With more than 4 partitions Gaia writes the standard sfdisk extended layout: partitions 1-3 are primary (p1-p3), MBR slot 4 becomes an extended partition (type `0x05`), and partitions 4.. become logical partitions p5, p6, ... Each logical partition has an EBR in the first sector of its aligned slot and its data starts at the next `alignment_lba` boundary. EBR entry 0 describes the logical partition relative to its EBR; entry 1 links the next EBR, relative to the extended partition start, and spans that EBR through the end of its logical partition. The extended entry spans from the first EBR to the end of the last logical partition.
- The disk file is created at its full size with unwritten ranges left as holes, so empty and padded partitions take no space on filesystems that support sparse files.
- Assembly state records each partition's `start_lba`, `sector_count`, image `bytes`, and for layouts with more than 4 partitions the kernel `number` and the `ebr_lba` of logical partitions; empty partitions record `empty=true` and `wipe_bytes`.

#### Truncated disks

`[[image.assembly.disks]]` options that make the raw file shorter than the disk:
- `truncate = "last-data"`: the file ends after the last written byte instead of at the end of the full-size layout. The end is the largest of: the partition table sectors (the MBR and every EBR), each materialized partition's image end (its start plus the image's size, not the partition size), and the full size of each `wipe = true` partition. The result is rounded up to `alignment_lba` (1 MiB by default) and capped at the full size. Unset (the default) keeps the full size. Only `mbr` is supported; `gpt` with `truncate` is a validation error, since a GPT backup header lives at the end of the full-size disk.
- The partition table still lists every partition at its full size. The bytes past the end are not written, so the partitions beyond it are blank until the disk is written to a device of its full size (or repaired with `sfdisk`/`partx` after the first boot).
- The size checks still apply against the partition size: an image larger than its partition fails before the disk is written.
- The RAM path, the archives (xz, zstd, gzip, raw) and the content digests all use the shorter file. The summary reports the raw and written sizes per image (see below).
- `ebr_placement = "default"` (the default) puts each EBR right before its logical partition, so a logical partition with no data at the end of the disk pulls its EBR, and so the file, a little further out. Nothing is cut off: the file always covers every EBR.
- `ebr_placement = "packed"`: the EBR chain of the extended partition is written in consecutive sectors at its start (EBR n at the extended start plus n sectors). Logical data starts after the chain, so with `truncate` the file ends after the last logical data and every logical partition is still described at its full size. Each EBR's first entry describes its logical partition relative to that EBR; its second entry links the next EBR, relative to the extended start. The planner errors if the chain leaves no room for the first logical partition. Only affects layouts with more than four partitions; use it with `truncate` when logical partitions are unmaterialized at the end of the disk.

Assembly results report, per published raw disk image (and the primary image), `raw_bytes` (file length) and `content_bytes` (bytes the filesystem allocates, from `st_blocks`, so holes are excluded). The run summary has them in `image_sizes` and adds a note such as `sdcard.img: 1.36 GB raw, 412.00 MB written`. Every published output's file size, and its change since the previous run, is in the summary's `sizes` list (see [Image sizes](reporting-and-state.md#image-sizes)).

#### Archives

`[[image.assembly.archives]]` builds a plain POSIX ustar archive after all disks, written by Gaia itself (no external `tar`):
- `id`: unique archive id.
- `output`: path template, like other assembly outputs; it can point into the image collect dir (`$provider.images/...`).
- `members`: ordered list. Each member has exactly one source: `src` (a path template; the file is copied, stored as `name`), `entries` (a generated `KEY=value` file, stored as `name`), `json` (a generated JSON document, see [Generated content](#generated-content)), `tree` or `src_dir` (every entry under an assembly tree or a directory), or `src_glob` (each file the pattern matches).
- `tree = "<tree id>"` and `src_dir = "<path template>"` archive every entry under that root, stored under `prefix` (empty, or a relative path ending in `/`). `name` may be left empty for these. Entries are walked in sorted order, a directory before its contents: regular files keep mode 0644, or 0755 when executable; directories are 0755; symbolic links are stored as links and not followed. Only regular files appear in the archive's state. A `tree` member reads its tree's root, so it runs after the steps that fill the tree, and its fingerprint covers the tree's content.
- `src_glob = "<pattern>"` archives each file the pattern matches under `name`, a directory prefix ending in `/`, followed by the file's own name.
- `exclude = [...]` on a `tree` or `src_glob` member leaves out files (not directories) by name, simple `*` globs allowed.
- Archive paths follow the ustar rule: a name, or a link target, longer than 100 bytes fails the build with an error naming it (the `prefix` field is not used to split names).
- `generated`: ordered list of generated members (`name`, `entries`). They are written **before** all `members`; to interleave generated and file members, put `entries` directly on a `members` item instead.
- `entries`: `[["KEY", "value"], ...]`, rendered as one `KEY=value` line per entry in order. Keys must match `[A-Za-z_][A-Za-z0-9_]*`. Values made only of `A-Z a-z 0-9 _ @ % + = : , . / -` are written as-is; anything else (including an empty value) is wrapped in single quotes with embedded `'` written as `'\''`, so the file can be sourced by `sh`. Values cannot contain newlines.
- Entry values may contain `${assembly.sha256:<path template>}`, replaced at assembly time by the lowercase hex sha256 of that file (an assembly output such as a transform result, or any other file). The file must exist when the archive is built. Normal config interpolation such as `${build.version}` runs first and may also appear inside the path (`${assembly.sha256:$assembly.out/${build.name}.img}`). Any other `${...}` left in a value is a validation error.
- Entry values may also contain `${assembly.size:<path template>}`, replaced by the size of that file in bytes.
- Entry values may also contain `${assembly.verity:<path template>:<field>}`, replaced by a field of a `verity` transform's output. The path must be the transform's `dest` written exactly as in the transform. The fields are `root_hash`, `salt`, `hash_offset`, `data_blocks`, `data_block_size` and `hash_block_size`; see [dm-verity and ext4](#dm-verity-and-ext4). A token that names no verity transform output is a validation error, and an unknown field name is rejected at validation too.

Archive output is deterministic: entries appear in the configured order with mtime 0, uid/gid 0, empty user and group names, and mode `0644`. Member names must be relative paths of at most 100 bytes (the ustar name field; prefix splitting is not used) without `.`/`..` components, and must be unique per archive. The archive is written to a temporary file and published atomically; state records `archives.N.output`, `bytes`, `sha256`, and per-member name, source, size, and sha256. Archive outputs are treated as expected outputs, so a bundle in the collect dir is not reported as a large unexpected file. Reuse fingerprints cover the archive config and the state of every member and digest input file.

#### Generated content

Three ways to write generated text or JSON into a tree or an archive. All three render `${assembly...}` tokens at assembly time and run after the steps whose outputs they read, so a file can name a transform's output that is declared after it.

- **Text files**: `[[image.assembly.files]]` with `content = "..."` (in place of `src`/`src_glob`) writes the text to `dest` in the tree. `mode` applies as for copied files; unset, a generated file is `0644` whatever the host's umask. `dest` must name a file, not a directory. Config interpolation such as `${build.name}` runs first. Any other `${...}` left in the text is an error, so a file that carries its own `${VAR}` (a shell script, say) belongs in `src` rather than `content`.
- **JSON documents**: `json = { ... }` on a file entry, or on an archive `members` or `generated` item (in place of `entries`/`src`). It is a TOML table whose values are strings, integers, booleans, arrays and tables. Floats are rejected. Keys keep the order the file wrote them, and the output is pretty-printed with two-space indentation and a trailing newline, so two runs write the same bytes.
- **Tokens** in any string value: `${assembly.sha256:<path>}` (hex digest), `${assembly.size:<path>}` (size in bytes) and `${assembly.verity:<path>:<field>}`. A string that is exactly one numeric token (`size`, or a verity `hash_offset`, `data_blocks`, `data_block_size` or `hash_block_size`) is written as a JSON number; any other token is written as text.
- **Tree digest map**: a JSON string that is exactly `${assembly.sha256_tree:<tree id or path template>}` is replaced by an object mapping each regular file's path under that tree (relative to its root, `/` separators, sorted by path) to its lowercase hex sha256. Symbolic links and directories are not listed. An empty tree gives `{}`. It is a whole JSON value only: in `content`, an entry or inside a longer string it is a validation error. The tree is read after the steps that fill it, so a file declared before its tree's files still digests all of them, and the map is a function of the bytes, so rewriting a file with the same bytes keeps the assembly reused. A file cannot digest the tree it is written into (`assembly_tree_digest_self`). For example, a boot tree's `slot.json` naming the payload:

  ```toml
  [[image.assembly.files]]
  tree = "boot"
  json = { boot = { files = "${assembly.sha256_tree:slots-payload}" } }
  dest = "slot.json"
  ```
- `${build.date}` is the build's time as RFC 3339 UTC (`2026-10-15T12:00:00Z`): `SOURCE_DATE_EPOCH` when the environment sets it, else the commit time of the build file's git `HEAD`. Outside both it stays unresolved and the assembly fails rather than write a wall-clock time.

A token that reads a work or out path no step writes is a validation error (`assembly_token_path_not_produced`), since nothing would produce it before the read. The rest of the rules are the archive rules above.

```toml
[[image.assembly.files]]
tree = "boot"
dest = "cmdline.txt"
content = "console=tty1 atlas.roothash=${assembly.verity:$assembly.work/slot/rootfs.erofs:root_hash} rootwait"

[[image.assembly.files]]
tree = "boot"
dest = "state.json"
json = { schema = 1, state = "trial", size = "${assembly.size:$assembly.work/slot/rootfs.erofs}" }
```

#### Listed binaries

`[[image.assembly.binaries]]` copies one binary from a target tree into a tree, with the runtime closure a dynamically linked binary needs: its interpreter and its shared libraries, found under a sysroot and copied to the same absolute paths in the tree (symlinks crossed on the way are recreated). A static binary is only copied. Each entry takes:
- `tree`: the tree to fill.
- `src`: the binary, a path template such as `$provider.buildroot_output/target/usr/sbin/e2fsck`.
- `dest`: where it goes in the tree, such as `usr/sbin/e2fsck`. It must be a file path inside the tree.
- `sysroot` (optional): the target root its libraries are read from. Unset, it is derived from `src`: the directory above its `bin/` or `sbin/` (or `usr/bin/`, `usr/sbin/`).
- `mode` (optional): octal, `0755` unless set.
- `strip` (optional): `true` strips the copied program and every copied runtime library in the tree. The source is never touched; a static binary's copy is stripped too.

Several binaries may share libraries: each library is copied once per binary, the later copy replacing the same file. Reuse fingerprints cover the binary and its closure, like BusyBox's.

A `[[image.assembly.files]]` entry that copies a dynamic binary without its libraries gets a warning naming them (see [Image Assembly](#image-assembly)); `binaries` is the entry that brings them. Libraries a `binaries` entry or another file entry brings into the tree count, whatever the entries' order.

#### dm-verity and ext4

Two kinds for images that a device mounts read-only or from a data partition.

**`verity`** (a transform) builds a dm-verity hash tree over `src`, a data image such as an EROFS root filesystem, with `veritysetup format`. `veritysetup` resolves from provider host tools first (`bin/`, `usr/bin/`, or the host dir), then host `PATH`; a missing tool fails the build with a clear error. Options, all for `kind = "verity"` only (other kinds reject them):
- `dest`: the output data image. The data is copied and zero-padded to whole `data_block_size` blocks. Unless `hash_dest` is set the hash tree is appended after the padded data, so `dest` is one file that holds the data and its tree.
- `hash_dest` (optional): write the hash tree to its own file instead; `dest` then holds the padded data alone.
- `data_block_size`, `hash_block_size`: powers of two from 512 to 4096, default 4096.
- `salt` (optional): hex, 1 to 256 bytes. Unset, the salt is the sha256 of `src`, so the same data always gets the same tree.
- `uuid` (optional): the superblock UUID. Unset, it is derived from the sha256 of `src`.

The tree uses sha256 and the default superblock. The layout is recorded for the consumer in the assembly state under `verity.<n>.*`, and archive entries reach it with `${assembly.verity:<dest>:<field>}`:

| Field | Meaning |
| --- | --- |
| `root_hash` | the root hash, lower-case hex |
| `salt` | the salt, lower-case hex |
| `hash_offset` | byte offset of the verity superblock in the hash device: the first hash-block boundary after the padded data when appended, `0` for a separate file |
| `data_blocks` | data blocks the tree covers |
| `data_block_size`, `hash_block_size` | the block sizes in bytes |

To open the tree, the hash device is `dest` when appended or `hash_dest` otherwise, and the root hash and hash offset are the recorded values: `veritysetup open <dest> <name> <hash device> <root_hash> --hash-offset=<hash_offset>`. The superblock records the block sizes, salt and UUID. Two runs over the same `src` give byte-identical output and the same root hash. The reproducibility rests on the salt and UUID being derived from the data, not on a random salt.

**`ext4`** (a filesystem) packs `source_tree` into an ext4 image with `mkfs.ext4 -d`, resolved like `veritysetup` (`mkfs.ext4` and `debugfs` from provider host tools, then `PATH`). Options, for `kind = "ext4"` only:
- `size` (required): a byte size with a binary suffix, or `"auto"`. The size must be a multiple of 4 KiB. Auto sizing counts each file and directory in whole 4 KiB blocks, the inode tables (two inodes per entry plus 64, 256 bytes each), 8 MiB of fixed overhead and the journal, plus a margin of 5% (at least 4 MiB), rounded up to a whole MiB.
- `label` (optional): at most 16 bytes, no `/`.
- `uuid` (optional): a UUID. Unset, it is derived from the filesystem `id`.
- `journal` (optional): `false` for no journal; the default is a journal of 1/64 of the image, between 4 and 128 MiB.

`mke2fs` copies the builder's owners and times into the image, so Gaia then sets every entry to root:root with timestamps of 0 (the epoch). File modes are kept. The fixed UUID, hash seed and time make two runs over the same tree byte-identical. Entry names containing `"`, `\` or a control character are rejected, because `debugfs` cannot set them. `publish = true` and the RAM work dir work as for the other filesystems, and `deterministic` must stay `true`.

##### Example: dynamic slot bundle

A read-only root slot: the EROFS root filesystem from the image provider (Buildroot's EROFS rootfs option, for example), a verity tree appended to it, and a bundle that carries the root's digest and the verity values the initramfs needs to open the tree:

```toml
[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/rootfs.verity.erofs"

[[image.assembly.archives]]
id = "slot"
output = "$provider.images/slot.pdupdate"

[[image.assembly.archives.generated]]
name = "manifest.env"
entries = [
  ["ROOTFS_SHA256", "${assembly.sha256:$provider.images/rootfs.erofs}"],
  ["VERITY_ROOT_HASH", "${assembly.verity:$assembly.work/rootfs.verity.erofs:root_hash}"],
  ["VERITY_SALT", "${assembly.verity:$assembly.work/rootfs.verity.erofs:salt}"],
  ["VERITY_HASH_OFFSET", "${assembly.verity:$assembly.work/rootfs.verity.erofs:hash_offset}"],
  ["VERITY_DATA_BLOCKS", "${assembly.verity:$assembly.work/rootfs.verity.erofs:data_blocks}"],
  ["VERITY_DATA_BLOCK_SIZE", "${assembly.verity:$assembly.work/rootfs.verity.erofs:data_block_size}"],
  ["VERITY_HASH_BLOCK_SIZE", "${assembly.verity:$assembly.work/rootfs.verity.erofs:hash_block_size}"],
]

[[image.assembly.archives.members]]
name = "rootfs.erofs"
src = "$assembly.work/rootfs.verity.erofs"
```

The bundle's `manifest.env` is the file the updater sources. The root slot is `rootfs.verity.erofs`; the initramfs opens it with the manifest's root hash and hash offset.

##### Example: a dynamic slot with its manifest, cmdline and tools

The same slot, with the root's verity values in a JSON manifest and in the kernel command line, a fragment that names the slot's trial state, and `e2fsck` copied into the initramfs with its libraries:

```toml
[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/slot/rootfs.erofs"

[[image.assembly.files]]
tree = "initramfs"
content = "atlas.roothash=${assembly.verity:$assembly.work/slot/rootfs.erofs:root_hash}\n"
dest = "cmdline.txt"

[[image.assembly.files]]
tree = "initramfs"
content = "trial=1\n"
dest = "default.txt"

[[image.assembly.files]]
tree = "initramfs"
dest = "state.json"
json = { schema = 1, id = "${input.slot_id}", version = "${build.version}", built = "${build.date}", source = "${project.describe}", image = { size = "${assembly.size:$assembly.work/slot/rootfs.erofs}", sha256 = "${assembly.sha256:$assembly.work/slot/rootfs.erofs}" }, verity = { root_hash = "${assembly.verity:$assembly.work/slot/rootfs.erofs:root_hash}", salt = "${assembly.verity:$assembly.work/slot/rootfs.erofs:salt}", hash_offset = "${assembly.verity:$assembly.work/slot/rootfs.erofs:hash_offset}", data_blocks = "${assembly.verity:$assembly.work/slot/rootfs.erofs:data_blocks}" } }

[[image.assembly.binaries]]
tree = "initramfs"
src = "$provider.buildroot_output/target/usr/sbin/e2fsck"
dest = "usr/sbin/e2fsck"
```

The `state.json` document is written in the order shown, and its `hash_offset` and `data_blocks` are JSON numbers.

##### Example: ext4 data partition

A data partition built from a tree of its own, sized from the tree and labelled, as the last partition of a disk:

```toml
[[image.assembly.filesystems]]
id = "data"
kind = "ext4"
source_tree = "data"
output = "$provider.images/data.ext4"
size = "auto"
label = "data"

[[image.assembly.disks.partitions]]
name = "data"
type_alias = "linux"
image = "$provider.images/data.ext4"
```

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
image = "$provider.buildroot_output/images/rootfs.ext4"
```

#### Example: flasher boot image with an initramfs

A flasher image boots a kernel with an initramfs that loads USB gadget modules
and runs the flashing script. The initramfs is packed as `cpio-zstd` and
placed in a FAT boot image, which is published as an image of the build:

```toml
build_name = "flasher"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "flasher_defconfig"

[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

# BusyBox and the kernel modules the gadget needs, with their dependencies.
[[image.assembly.busybox_initramfs]]
tree = "initramfs"
busybox = "$provider.target/bin/busybox"
include_runtime_libs = false
applets = ["sh", "mount", "mkdir", "modprobe", "switch_root"]

[[image.assembly.kernel_modules]]
tree = "initramfs"
from = "$provider.target/lib/modules"
modules = ["libcomposite", "usb-f-mass-storage"]

[[image.assembly.files]]
tree = "initramfs"
src = "@assets/flasher/init"
dest = "init"
mode = "0755"

# The initramfs, compressed with zstd level 19 (the default).
[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-zstd"
source_tree = "initramfs"
output = "$assembly.work/initramfs.cpio.zst"
compression_level = 19

# The boot tree: kernel, device tree, config.txt and the initramfs.
[[image.assembly.files]]
tree = "boot"
src = "$provider.images/Image"
dest = "Image"

[[image.assembly.files]]
tree = "boot"
src = "$provider.images/bcm2711-rpi-4-b.dtb"
dest = "bcm2711-rpi-4-b.dtb"

[[image.assembly.files]]
tree = "boot"
src = "@assets/flasher/config.txt"
dest = "config.txt"

[[image.assembly.files]]
tree = "boot"
src = "$assembly.work/initramfs.cpio.zst"
dest = "initramfs.cpio.zst"

# boot.img, published to $provider.images/boot.img.
[[image.assembly.filesystems]]
id = "boot"
kind = "vfat"
source_tree = "boot"
output = "$assembly.work/boot.img"
size = "64M"
publish = true
```

The boot tree's `config.txt` carries the line `initramfs initramfs.cpio.zst followkernel`, which tells the firmware to load the initramfs next to the kernel. The boot tree reads the `cpio-zstd` output, so it runs after the initramfs is packed.

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
