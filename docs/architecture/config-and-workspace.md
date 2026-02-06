# Config And Workspace Model

## Config merge semantics

Implemented in `crates/gaia-image-builder/src/config/mod.rs`:

- Root-level `extends = "..."` loads one parent file first.
- `imports = ["...", ...]` can appear at root or nested table levels.
- Import merge order is left-to-right; local table values override imported values.
- `imports` keys are removed after inlining.
- Import cycles are detected and rejected.

## Module detection

Each module checks for a specific table path:

- `core`: always active.
- `buildroot`: active if `[buildroot]` exists.
- `buildroot.rpi`: active if `[buildroot.rpi]` exists.
- `stage`: active if `[stage]` exists.
- `program.*`: active per subsection (`[program.rust]`, `[program.java]`, etc.).

## Workspace paths

Defined in `crates/gaia-image-builder/src/workspace.rs`.

`[workspace]` controls:

- `root_dir`
- `build_dir`
- `out_dir`
- `clean` (`none|build|out|all`)
- `paths` aliases

Path resolution order for config fields:

1. `@alias/...` (including built-ins `@root`, `@build`, `@out`)
2. absolute path
3. relative path under `workspace.root_dir`

## Build templates

Certain buildroot output fields support templates via `modules::util::expand_build_template`:

- `{build}`: stem of the selected build file
- `{version}`: value from `[build].version`

If `{version}` is used without `[build].version`, Gaia returns an error.
