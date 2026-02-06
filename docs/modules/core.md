# Core Module

Source: `crates/gaia-image-builder/src/modules/core.rs`

## Responsibilities

- Always active.
- Adds `core.init` task.
- Validates schema by rejecting unsupported top-level legacy tables.

## `core.init` runtime behavior

Source: `crates/gaia-image-builder/src/executor/mod.rs` (`core_init`)

- Reads `[workspace]` config.
- Logs resolved workspace settings.
- Applies cleanup policy (`none/build/out/all`).
- Initializes absolute paths and alias map.
- Stores resolved workspace paths in shared execution context.

Everything else in Gaia assumes `core.init` ran first.
