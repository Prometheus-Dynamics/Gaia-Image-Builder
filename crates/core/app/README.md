# `gaia-app`

Application-layer orchestration crate.

## Purpose

- coordinate the user-facing flow from input to result
- connect config, validation, planning, execution, and reporting layers

## Owns

- CLI command orchestration
- future TUI orchestration
- high-level application flow

## Does Not Own

- raw config merge logic
- canonical spec definitions
- execution internals
- provider implementations

## Depends On

- `gaia-config`
- `gaia-spec`
- `gaia-validate`
- `gaia-plan`
- `gaia-exec`
- `gaia-report`
- provider catalog crates

## Current Task List

- [x] Create `src/cli/mod.rs` for argument parsing and command selection.
- [x] Create `src/commands/resolve.rs`, `validate.rs`, `plan.rs`, and `run.rs`.
- [x] Define a shared command result type so all commands return structured success/failure information.
- [x] Route `resolve` through `gaia-config` only.
- [x] Route `validate` through `gaia-config` then `gaia-validate`.
- [x] Route `plan` through `gaia-config`, `gaia-validate`, then `gaia-plan`.
- [x] Route `run` through `gaia-config`, `gaia-validate`, `gaia-plan`, `gaia-exec`, then `gaia-report`.
- [x] Add a future `src/tui/` only after the command layer is stable.
- [x] Keep this crate orchestration-only; push implementation into lower layers.
