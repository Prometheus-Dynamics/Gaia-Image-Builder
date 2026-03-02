# Program Modules

Sources:

- `crates/gaia-image-builder/src/modules/program/mod.rs`
- `.../program/lint.rs`
- `.../program/rust.rs`
- `.../program/java.rs`
- `.../program/custom.rs`
- `.../program/install.rs`

## Shared program config

`[program]` defines:

- `default_profile`
- `profiles` (target/toolchain/container/env)
- `checks` (reusable commands)
- `check_policy` (`required` or `warn`)

Artifact IDs must be globally unique across rust/java/custom.

## `program.lint`

Runs selected checks before artifact builds.
Provides `program:linted` token.

## Artifact builders (`rust`, `java`, `custom`)

Each builder:

- has `enabled`, `workspace_dir`, optional default `check_ids`
- emits one task (`*.artifacts`) that may process multiple artifacts
- supports artifact `mode`: `build`, `prebuilt`, `auto`
- supports `enabled_if` / `disabled_if` conditions per artifact
- writes artifact records to `build/artifacts/<id>.json`
- writes build state fingerprints to `build/artifacts/<id>.state.json`

Conditions are evaluated from resolved build inputs:

- `[inputs.options.<key>]` defines user-facing toggles/inputs
- CLI overrides use `--set key=value`
- conditions support:
  - `key` (truthy)
  - `!key` (falsey)
  - `key=value`
  - `key!=value`

Build inputs are also exported to program commands as env vars:

- `GAIA_INPUT_<KEY>` (uppercased, non-alnum converted to `_`)
- example: `pv_jar_source` -> `GAIA_INPUT_PV_JAR_SOURCE`

### Rust specifics

- If `build_command` is omitted, defaults to `cargo build` for declared `package`.
- Supports optional container builds via program profile `container_image`.
- Auto output inference for `bin`/`cdylib` kinds when `output_path` is omitted.

### Java/custom specifics

- `build_command` is required for build mode.
- `output_path` identifies produced file/dir.
- `program.custom` additionally supports `after_artifacts` for dependency ordering between custom artifacts.
  - supports optional dependency with `?` suffix (for example `"driver-jar?"`).

## `program.install`

Copies artifact outputs into stage rootfs paths (`/usr/bin/...`, etc.).

- `dest` must be absolute image path.
- can set mode/owner/group.
- supports `enabled_if` / `disabled_if` per install item.
- provides `stage:program-install`, which `stage.render` can depend on.

## Practical pattern

1. Build artifacts with `program.rust/java/custom`.
2. Install selected artifacts via `program.install`.
3. Let `stage.render` add configs/services that reference those binaries.

## HeliOS examples

HeliOS-style example layout:

- rust artifact catalog:
  - `configs/modules/program/rust.toml`
- frontend custom artifact:
  - `configs/modules/program/custom.toml`
- install mapping:
  - `configs/modules/program/install.toml`

Useful production patterns shown there:

- host profile for frontend + cross profile for runtime binaries
- plugin `cdylib` artifacts installed into `/usr/lib/helios/plugins/...`
- strict artifact IDs reused consistently in `program.install`
