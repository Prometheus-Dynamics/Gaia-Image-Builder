# Configuration: Inputs, presets, variants and profiles

[Configuration index](configuration.md)

## Inputs

Inputs are declared at the top level, then selected through defaults, presets, or CLI `--set input.<name>=...`.

```toml
[inputs.target]
description = "Hardware target identifier"
kind = "string"
default = "evk"

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
- `choices_from.refresh`: `auto`, `always`, or `never`. With `auto` and no `cache_ttl_seconds`, a successful fetch is kept in memory for 60 seconds within one `gaia` process (variants and validate-then-plan share it); `always` fetches every time.
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
repo = "example-org/my-app"
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
[inputs.rel]
kind = "enum"
default = "2027"
choices = ["2026", "2027"]

[[variants]]
id = "rel2026"
description = "my-app 2026"
overrides = [
  ["input.rel", "2026"],
]
```

A variant can also apply presets, like `--preset` does for one variant. Its
presets replace the command line's `--preset` for that variant; its own
`overrides` then apply on top:

```toml
[presets.rel2026]
overrides = [
  ["input.rel", "2026"],
  ["input.myapp_rev", "0123456789abcdef0123456789abcdef01234567"],
]

[[variants]]
id = "rel2026"
preset = "rel2026"            # or presets = ["a", "b"], applied in order
```

Running the file builds the base config as variant `default` and then each
declared variant, in order. `gaia run --variant rel2026` builds only that one
(`--variant default` names the base; several ids may be given, comma
separated). Without `--variant` every variant is built.

Rules:
- Ids use `[a-z0-9][a-z0-9-]*`, are unique, and `default` is reserved.
- An override key must be one Gaia knows, or `input.<name>` for an input the
  build declares. As with `--set`, a misspelled key is an error.
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
- Variants that would write the same output file are an error. The paths
  compared are `collect_dir/archive_name` of the raw disk image, the `output`
  of each assembly disk, archive and filesystem, and `export_dir/<file name>`
  when exports go to a configured directory. An image with no `archive_name`
  is named from its display name and version. The error names the path and
  the variants. Give each variant its own `input` that changes the image name
  or version, or its own `image.output.archive_name`.
- Variants run one after another in one process. A failing variant stops the
  run; the variants before it keep their outputs.

## Profiles

One build file can make several device profiles from shared software layers.
A profile is a layer that an `imports` entry selects with `when`, usually by
`build.target`:

```toml
# build.toml
extends = "software.toml"                  # shared: sources, artifacts, installs
build_name = "profiles"
target = "raze"
imports = [
  { path = "profiles/raze.toml", when = { target = "raze" } },
  { path = "profiles/systemcore.toml", when = { target = "systemcore" } },
]
```

`--set build.target=systemcore` selects the second profile for `validate`,
`plan` and `run`. `examples/profiles` is a complete build of both: `raze` is a
Buildroot image, and `systemcore` is an ipk package built with that OS's
toolchain, with no image (`kind = "none"`). Its validation and plan are
checked by the examples test.

The shared `[image]` in `software.toml` is the Buildroot base both profiles
start from. The systemcore layer switches the kind to `none`, so that base's
kind-specific fields are dropped for that profile (see "Image kind switch").

[[variants]] also builds several images from one file, but the variants share
one Buildroot base: a variant that drops the base's source or image is an
error (see "Variants"). Use a `when = { target = ... }` layer per profile when
the profiles differ in their image.

### Patch entries

A layer changes an entry of an id-keyed table without repeating it. The entry
has no key that marks a full declaration, so it is a patch:

| table | declaring key (a patch lacks it) | patch example |
|---|---|---|
| `[[artifacts]]` | `kind` | `[[artifacts]] id = "helios-api"` `toolchain = "systemcore"` |
| `[[install]]` | `artifact` | `[[install]] id = "install-helios-cli"` `dest = "/usr/local/bin/helios-cli"` |
| `[[stage.files]]` | `src` or `content` | `[[stage.files]] id = "motd"` `mode = 0o600` |
| `[[stage.services]]` | `name` | `[[stage.services]] id = "helios"` `unit_path = "helios-2.service"` |

`[stage.env_sets]` has no patch form: its entries are full declarations.

Rules:
- Each key the patch sets replaces that key of the earlier entry. A key it
  leaves out keeps the earlier value.
- A table value (such as `execution`, or `execution.docker`) replaces the whole
  table, not its keys: a patch with `[artifacts.execution] backend = "host"`
  drops the earlier `execution.docker` too.
- A patch may not set `id` only: that is an error, `sets no field to change`.
- A patch applies to the entry declared by a layer before it. Within one file,
  its declarations come before its patches. The entrypoint is the last layer,
  so an entry the entrypoint declares cannot be patched by one of its imports:
  declare shared entries in an `extends` base, or in an import, instead.
- A patch whose id no earlier entry declares is an error, and names the layer
  and line:
  `artifact patch 'x' in profiles/systemcore.toml:12 has no earlier [[artifacts]] entry to patch; add kind to declare it`.
  The error says which key declares the table (`kind`, `artifact`, `src` or
  `name`).
- A patch is applied to the merged entry, so `${...}` tokens in the patched
  fields are interpolated like any other value.

The merge records each patch in the build's notes, which `validate` and `plan`
print with a `note:` prefix. Each note names the layer that set each field:

```text
note: artifact 'helios-api': toolchain set by profiles/systemcore.toml:24
note: install 'install-helios-cli': dest set by profiles/systemcore.toml:33
```

### Image kind switch

A later layer that sets `[image] kind` to a different kind than the merged base
replaces the kind-specific image fields instead of merging into them. Replaced:
the definition's own keys (the Buildroot `source`, `defconfig`,
`defconfig_path`, `config_fragments`, `config_overrides`, `external_tree`,
`external_tree_mode`, `expected_images`, `rootfs`; the starting point's
`source`, `source_path`, `rootfs_path`, `packages` and so on), and the feed,
`signing`, `checks`, `[[image.hardware]]` and `layers`. Kept, and still
merged: `[image.output]` and `[image.assembly]`. A layer that sets the same kind
merges as before, and a layer without `kind` merges under the base's kind.

The note names the layer and the replaced keys that the base set:

```text
note: image kind none (profiles/systemcore.toml) replaces buildroot fields: source, defconfig_path, external_tree_mode, expected_images, feed
```
