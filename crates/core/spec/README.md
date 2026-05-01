# `gaia-spec`

Canonical typed model for Gaia builds.

## Purpose

- define the internal source of truth for a build

## Owns

- `ResolvedBuildSpec`
- typed domain models
- shared identifiers and references between domains

## Current Domains

- `source`
- `artifact`
- `image`
- `inputs`
- `workspace`
- `install`
- `stage`
- `checkpoints`
- `reporting`

## Planned Domains

- deeper policy and identity refinements

## Does Not Own

- raw TOML loading
- planning policy
- process execution
- console reporting

## Consumers

- `gaia-config`
- `gaia-validate`
- `gaia-plan`
- `gaia-exec`
- `gaia-report`
- provider crates

## Current Task List

- [x] Replace enum-only source definitions with typed payload structs for git, path, archive, and download.
- [x] Replace enum-only artifact definitions with typed payload structs for Rust, Java, Node, Python, and Go.
- [x] Replace enum-only image definitions with typed payload structs for Buildroot and starting-point providers.
- [x] Add `workspace.rs` with `WorkspaceSpec`.
- [x] Add `install.rs` with `InstallSpec`.
- [x] Add `stage.rs` with `StageSpec`.
- [x] Add `checkpoints.rs` with `CheckpointSpec`.
- [x] Add `reporting.rs` with `ReportingSpec`.
- [x] Introduce stable ids and refs instead of open `String` references where domain links exist.
- [x] Make `ResolvedBuildSpec` the only canonical build model used outside `gaia-config`.
- [x] Add richer typed policy/config sections for future presets, interpolation metadata, and provenance identity.
