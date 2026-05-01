# `gaia-exec`

Execution runtime for planned Gaia operations.

## Purpose

- execute the plan produced by `gaia-plan`

## Owns

- runtime execution entrypoints
- future process management
- future filesystem operations
- future cancellation and event emission

## Does Not Own

- raw config loading
- spec design
- planning policy
- provider catalog design
- final reporting presentation

## Depends On

- `gaia-spec`
- `gaia-plan`
- source provider catalogs
- artifact provider catalogs
- image provider catalogs

## Current Task List

- [x] Add `src/runtime/` for execution context and runtime state.
- [x] Add `src/operations/` for operation handler traits and handler dispatch.
- [x] Add `src/process/` for child process execution.
- [x] Add `src/fs/` for filesystem mutation helpers.
- [x] Replace plan execution over placeholder strings with typed operation execution.
- [x] Define runtime events for operation start, log, success, and failure.
- [x] Define structured execution errors instead of ad hoc strings.
- [x] Keep this crate free of raw config access as the runtime expands.
