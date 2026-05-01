# `gaia-validate`

Validation layer for resolved Gaia build specs.

## Purpose

- validate typed specs before expensive work begins

## Owns

- structural validation
- semantic validation
- warnings and errors against `ResolvedBuildSpec`

## Does Not Own

- config loading
- planning
- execution
- provider implementation details

## Depends On

- `gaia-spec`

## Current Task List

- [x] Define diagnostic types with severity, code, message, and optional location.
- [x] Validate duplicate ids in every spec domain.
- [x] Validate source refs used by artifacts.
- [x] Validate artifact refs used by install definitions.
- [x] Validate provider payloads against selected provider kinds.
- [x] Validate unsupported combinations across image, stage, and install domains.
- [x] Return multiple diagnostics in one run instead of short-circuiting on first error.
- [x] Expose a report type that `gaia-app` can print directly in a future `validate` command.
- [x] Add validation for artifact dependency cycles.
- [x] Add provider-aware validation hooks for rules that belong to specific providers.
