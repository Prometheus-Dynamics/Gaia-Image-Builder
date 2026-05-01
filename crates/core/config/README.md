# `gaia-config`

Configuration loading and resolution layer.

## Purpose

- load build configuration sources
- resolve config into the canonical Gaia spec

## Owns

- config ingestion
- future merge rules
- future interpolation
- future env-file handling

## Does Not Own

- planning
- execution
- reporting
- provider behavior

## Output

- `ResolvedBuildSpec`

## Depends On

- `gaia-spec`

## Current Task List

- [x] Create `src/load/` for raw file loading and build-root discovery.
- [x] Create `src/merge/` for `extends` and `imports`.
- [x] Create `src/interpolate/` for variable expansion and template resolution.
- [x] Create `src/env/` for env-file loading and precedence resolution.
- [x] Remove legacy compatibility adapters; old configs must be translated before use.
- [x] Define internal parse structs that are separate from `gaia-spec` types.
- [x] Replace the placeholder `resolve_config()` implementation with a config pipeline and config-to-spec compilation.
- [x] Compile `workspace`, `source`, `artifact`, `install`, `stage`, `image`, `checkpoints`, and `reporting` into `ResolvedBuildSpec`.
- [x] Ensure no downstream crate needs raw config structs after this crate finishes.
- [x] Replace the sample loader with actual TOML-backed file loading.
- [x] Implement real `extends` and `imports` merge semantics.
- [x] Implement real interpolation and layered env precedence.
