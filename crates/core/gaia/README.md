# `gaia`

Thin binary entrypoint for the Gaia workspace.

## Purpose

- compile to the user-facing executable
- hand off control to `gaia-app`

## Owns

- process entrypoint
- top-level boot wiring

## Does Not Own

- config loading
- build spec modeling
- planning
- execution logic
- reporting logic

## Depends On

- `gaia-app`

## Current Task List

- [x] Keep [src/main.rs](./src/main.rs) as a single handoff into `gaia_app::run()`.
- [x] Add explicit process exit code handling once `gaia-app` returns structured command results.
- [x] Add top-level logging/bootstrap wiring only after the app layer defines a stable startup contract.
- [x] Do not add CLI parsing, config loading, or planning logic in this crate.
