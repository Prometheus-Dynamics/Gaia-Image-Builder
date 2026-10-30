# Configuration

Gaia resolves one build file into one `ResolvedBuildSpec`. The raw TOML model is intentionally close to the canonical spec, but not identical. This document describes the TOML surface you can actually write today.

## Pages

This page covers loading, conditions, the top-level fields, interpolation and secrets. Each other part of the build file has its own page:

- [Inputs, presets, variants and profiles](configuration-inputs.md): [Inputs](configuration-inputs.md#inputs), [Presets](configuration-inputs.md#presets), [Variants](configuration-inputs.md#variants), [Profiles](configuration-inputs.md#profiles)
- [Policy, workspace and clean](configuration-policy.md): [Failure Policy](configuration-policy.md#failure-policy), [Execution Policy](configuration-policy.md#execution-policy), [Provider Execution Policy](configuration-policy.md#provider-execution-policy), [Provenance](configuration-policy.md#provenance), [Workspace](configuration-policy.md#workspace), [Clean](configuration-policy.md#clean)
- [Sources and toolchains](configuration-sources.md): [Sources](configuration-sources.md#sources), [Toolchains](configuration-sources.md#toolchains)
- [Artifacts, install and stage](configuration-artifacts.md): [Artifacts](configuration-artifacts.md#artifacts), [Install](configuration-artifacts.md#install), [Stage](configuration-artifacts.md#stage)
- [Image](configuration-image.md): [Image](configuration-image.md#image)
- [Image assembly](configuration-assembly.md): [Image Assembly](configuration-assembly.md#image-assembly), [Assembly checks](configuration-assembly.md#assembly-checks)
- [Checkpoints, registry and reporting](configuration-checkpoints.md): [Checkpoints](configuration-checkpoints.md#checkpoints), [Registry checkpoints](configuration-checkpoints.md#registry-checkpoints), [Canonical tree path](configuration-checkpoints.md#canonical-tree-path), [Reporting](configuration-checkpoints.md#reporting)
- [Key reference](reference/config-keys.md): every key Gaia accepts, generated from the config schema.

## File Loading

Gaia accepts:
- an explicit file path
- or a logical build name that resolves through repo-relative fallback locations configured by the app; in this repo the default fixture lives under `examples/default-workspace/configs/`

Config files can use:
- `extends = "base.toml"` for one base file
- `imports = ["a.toml", "b.toml"]` for additive fragments
- table imports with `when` for conditional fragments, for example
  `{ path = "full.toml", when = { profile = "full" } }`

The image `kind` (`kind = "buildroot"`, `kind = "starting-point"` or `kind = "none"`) may be set
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
- a patch entry (an `[[artifacts]]`, `[[install]]`, `[[stage.files]]` or
  `[[stage.services]]` entry without its declaring key) changes the earlier
  entry with its id; see "Patch entries" under "Profiles"
- a later `[image] kind` that differs from the merged base's replaces the
  kind-specific image fields; see "Image kind switch" under "Profiles"

### Imports From a Git Source

A layer can live in another repository and be imported from a git source,
for example shared device support kept next to the hardware it supports:

```toml
# in a local config file (the build entrypoint or a layer it imports by path)
imports = [
  "../layers/base-os.toml",
  { source = "bsp", path = "devices/myboard/gaia/device.toml", when = { target = "evk" } },
]

[[sources]]
id = "bsp"
kind = "git"
repo = "https://example.com/org/bsp"
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
  (see [Git Source Lockfile](configuration-sources.md#git-source-lockfile)). Otherwise resolution fails
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
`external_tree = "@source:bsp/devices/myboard/gaia/buildroot-external:myboard/assets/buildroot"`:
- `@self` / `@self/<rest>`: absolute directory of the file containing the
  value. This is the same for local files and source-imported files, so a
  layer written with `@self/overlays/myboard.dtbo` works from a checkout and from a
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
- `image_kind`: `buildroot`, `starting-point` or `none`
- `input = { name = "value", ... }`: each named input must have that value,
  compared as a string. A boolean or integer is written as it reads in TOML
  (`input = { flag = true }` matches the input value `"true"`). The names
  must be inputs some file of the build declares; any other name is an
  error.

Inputs are the values the build selects: the declared defaults, then the
selected preset's `input` overrides (the variant's `preset`/`presets` for a
variant), then `--set`, then a variant's overrides. Import `when` conditions
see the entrypoint's selection, also from files they import, so
`{ path = "sensor.toml", when = { input = { backend = "sensor" } } }` follows
`--set input.backend=sensor`, a preset, or the variant that sets it. The presets
that set those inputs must be defined in the entrypoint file.

```toml
imports = [
  { path = "myboard/myapp-sensor.toml", when = { input = { backend = "sensor" } } },
]

[[artifacts]]
id = "sensor-helper"
# ...
when = { input = { backend = "sensor" } }
```

Each variant resolves its own imports, so two variants with different
`backend` values get different layers.

## Top-Level Build Fields

Supported top-level fields:

```toml
gaia_version = ">=2.0.0"
build_name = "acme-evk"
display_name = "AcmeOS EVK"
version = "v2026.2.0"
description = "Release image for the EVK board."
branch = "main"
target = "evk"
profile = "release"
labels = [
  ["stack", "acme"],
  ["board", "evk"],
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
artifacts = ["acme-api", "acme-engine"]
installs = ["install-acme-api"]
sources = ["vision-plugin"]
```

`[expect]` lists ids the build must end up with, in any layer. Validation
fails (`config_expected_missing`) when one is missing, so a layer that
stops providing it (deleted, emptied, no longer imported, or its `when` no
longer matching) cannot silently drop it from the image. Lists from all
layers are combined.

Unknown keys fail validation (`config_unknown_key` errors, for example
`unknown key 'build_command' in 'build.toml:1:1': this gaia does not know it`),
so a typo, or a file written for a newer Gaia, never builds with the setting
silently missing. The message names the file, line and column of the key, and
a close match among the keys that table takes (`did you mean 'branch'?`).
The check covers top-level keys and the `workspace`, `product`,
`interpolation`, `clean`, `execution`, `failure`, `providers.*`,
`provenance`, `reporting`, `stage`, `install` and `checkpoints` tables.
`[[sources]]`, `[[artifacts]]` and `[image]` are checked by their `kind`: a
source takes the keys of its `kind` (`git`, `path`, `archive`, `download`), an
artifact those of its `kind` (`rust`, `java`, `node`, `python`, `go`), and the
image those of its `kind` (`buildroot`, `starting-point`, `none`), including the
`[image.rootfs]`, `[[image.expected_images]]` and `[image.packages]` tables.
A table with no `kind` (a layer that leaves it to the layer it extends) takes
the keys of every kind. Free-form maps (`inputs`, `presets`, `env`) are not
checked.

A parse error, or a value of the wrong type, is reported at the line and
column where it is: `failed to parse build config 'build.toml:4:12': ...`.

### Editor Support: JSON Schema

`gaia config schema` prints a JSON Schema of the build file, written from the
same key lists the check above uses, so an editor reports the same unknown
keys and completes the keys of each kind:

```sh
gaia config schema > gaia-build.schema.json
```

With taplo, point a rule at the file in `.taplo.toml`:

```toml
[[rule]]
include = ["**/build.toml", "configs/**/*.toml"]
[rule.schema]
path = "gaia-build.schema.json"
```

With VS Code's Even Better TOML, associate the file in `settings.json`:

```json
{
  "evenBetterToml.schema.associations": {
    "build\\.toml$": "./gaia-build.schema.json"
  }
}
```

The schema checks key names and the `kind` values. It does not check value
types, and it describes no keys in words: the reference is this document.

## Product Metadata

```toml
[product]
family = "acme"
name = "vision-system"
sku = "acme-evk-release"
```

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

A reference that leads back to a token it is already resolving is an error
that names the cycle. For example, `build_name = "x-${build.display_name}"`
with `display_name = "${build.name}"` fails with
`interpolation cycle: build.display_name -> build.name -> build.display_name`.

## Secrets

A `[secrets]` table names values a build reads without storing them: registry tokens, signing keys and API keys. Each entry names where its value comes from, and the value is read when it is used.

```toml
[secrets.ghcr]
command = ["gh", "auth", "token"]        # the standard output, trimmed

[secrets.signing]
file = "~/.config/gaia/secrets/signing-key"   # relative paths are taken from the workspace root
required = false                         # default true; false makes a missing source resolve to nothing

[secrets.ci]
env = "CI_TOKEN"                         # the value of an environment variable
```

Each entry names exactly one source. Validation rejects an entry with none or with two, an empty source, and a name that uses characters other than letters, digits, `_`, `-` and `.`. An unknown key in an entry is reported like any other unknown key.

- `env = "VAR"`: the variable's value, as it is.
- `file = "path"`: the file's contents without its trailing newline. `~` is the home directory. The file must not be readable by group or others: a file with any group or other permission is refused, and the error names the path and the fix, `chmod 600 <path>`. Gaia does not change the mode.
- `command = ["program", "arg", ...]`: the program and its arguments, run without a shell. Its standard output, trimmed, is the value. Its standard error is discarded, never echoed. A non-zero exit is an error that names the entry and the program.
- `required` (default `true`): a missing variable, a missing or unreadable file, or a failed command is an error when the secret is used, and the error names the entry and its source. With `required = false` the reference resolves to nothing instead: a signing key is then empty, and the step that needs it reports that; a token falls through to the next registry credential source; and a `${secret:NAME}` in `build_env` becomes empty.

Values are read lazily: when a signature is made or checked, when a registry request needs credentials, or when a build process starts. Loading the build file reads none of them. A command runs once per process.

**References.** A reference names an entry. There are three forms:

- `secret:NAME` as a whole value in a secret-capable field: `[image.signing] key`, and a checkpoint's `sign_key`, `verify_key`, `token` and `username`. The field receives the value, so for `minisign` and `cosign-blob` the secret holds the key path and for `gpg` the key id.
- `env:VAR` as a whole value, in the same fields. It works as before, and a `token` takes it too.
- `${secret:NAME}` inside a string, only where a build-time process environment is set: an artifact's `build_env` (`rust` and `java` artifacts). The resolved spec keeps the text `${secret:NAME}`, so fingerprints, reuse state, provenance and `gaia config show` see the reference and not the value. The value is substituted when the build process starts, and it goes only to that process. Anywhere else, `${secret:...}` is a validation error that names the places where it is allowed: the `build_env` of an artifact. A stage environment set, a file, an image field or an interpolation value that lands outside `build_env` is refused the same way.

A checkpoint's `token` must be a reference (`secret:NAME` or `env:VAR`). A literal token in the build file is a validation error, so the token never sits in a file that is committed.

**Environment references.** `${env:NAME}` still works and may hold a value that is not a secret. Its value is stored in the resolved spec, so when `NAME` contains `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY` or `KEY`, validation warns: use a `[secrets]` entry and `secret:` or `${secret:}` so the value is not stored.

**What is never stored.** A resolved value is never in the resolved spec, the fingerprints, the reuse state, provenance, the reports or `gaia config show`. Those hold the reference, the entry's name and its source (a variable name, a file path or a command).

**Masking.** Every resolved value of at least four characters is registered for masking, and each occurrence is replaced with `[reporting.masking] replacement` (`***` by default) in captured process output and operation logs, failure messages and output tails, and the report files (the run summary, provenance and the manifest). Masking applies even when `[reporting.masking] enabled = false`, because it is by value and not by key name. A value shorter than four characters is not masked. Masking finds the value as written: a build that transforms a secret (encodes or splits it) can still print a form that is not masked.

**Example: a GHCR registry cache.** The token comes from the GitHub CLI, or from a 0600 file instead:

```toml
[secrets.ghcr]
command = ["gh", "auth", "token"]
# or: file = "~/.config/gaia/secrets/ghcr-token"   (chmod 600)

[[checkpoints]]
id = "registry"
backend = "oci"
registry = "ghcr.io"
repository = "owner/gaia-cache"
verify_key = "/keys/gaia.pub"
sign_key = "/keys/gaia.sec"
token = "secret:ghcr"
username = "owner"                      # default "gaia"
use_policy = "auto"
upload_policy = "auto"
```

## Template Files

See:
- [../examples/templates/full/README.md](../examples/templates/full/README.md)
- [../examples/templates/minimal-buildroot/build.toml](../examples/templates/minimal-buildroot/build.toml)
- [../examples/templates/minimal-starting-point/build.toml](../examples/templates/minimal-starting-point/build.toml)
