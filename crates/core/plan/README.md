# `gaia-plan`

Planning layer that turns a resolved spec into an execution plan.

## Purpose

- derive executable operations from typed build intent

## Owns

- operation graph generation
- ordering and dependency structure
- future reuse decisions
- future checkpoint-aware planning

## Does Not Own

- raw config access
- command execution
- final reporting

## Depends On

- `gaia-spec`
- source provider catalogs
- artifact provider catalogs
- image provider catalogs

## Current Task List

- [x] Add `src/graph/` for plan graph types and edge management.
- [x] Add `src/operations/` for typed operation definitions.
- [x] Replace `Vec<String>` operations with a typed operation enum or struct hierarchy.
- [x] Define operation ids and dependency refs.
- [x] Derive source-materialization operations from `SourceSpec`.
- [x] Derive artifact-build/materialization operations from `ArtifactSpec`.
- [x] Derive install and stage operations once those domains exist in `gaia-spec`.
- [x] Derive image operations from `ImageSpec`.
- [x] Add provider planning hooks so provider crates contribute operations rather than the planner matching everything itself.
- [x] Add explicit graph validation for cycles and missing dependency nodes.
- [x] Split stage planning into file/env/service operations instead of one coarse render step.
- [x] Surface plan diagnostics explicitly in `gaia-app` or a future `gaia validate-plan` command.
