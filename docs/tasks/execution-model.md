# Task Execution Model

## Task shape

A task has:

- `id`
- `label`
- `module`
- `phase`
- `after` (dependencies)
- `provides` (dependency tokens)

Defined in `crates/gaia-image-builder/src/planner/mod.rs`.

## Dependency resolution

`after` entries may reference:

- a task id (`buildroot.fetch`)
- a provided token (`artifacts:rust`)
- optional dependency (`stage:done?`)

Optional dependencies are skipped when not present.

## Stage barrier

During `Plan::finalize_default`, Gaia adds `core.barrier.stage`:

- depends on every task that provides a token starting with `stage:`
- provides `stage:done`

This gives one global sync point for later tasks (notably `buildroot.collect`).

## Execution

From `crates/gaia-image-builder/src/executor/mod.rs`:

- `execute_plan`: sequential execution.
- `execute_plan_parallel`: concurrent scheduling constrained by dependencies.
- `--dry-run`: task bodies are not executed; commands are logged.
- cancellation: active process groups are terminated.

## Core init behavior

`core.init` initializes workspace paths and performs cleanup according to `[workspace].clean`.
All other tasks call `workspace_paths_or_init()`, which avoids repeating cleanup.

## HeliOS planning workflow

For a HeliOS-style build, use:
```bash
cargo run -- plan configs/builds/HeliOS-cm5.toml
cargo run -- plan configs/builds/HeliOS-cm5.toml --dot
```

How to read the output:

- if `program.install.stage` is present, it means at least one artifact producer (`rust/java/custom`) is configured
- if `buildroot.rpi.*` tasks are present, `[buildroot.rpi]` is active for board prep
- `buildroot.collect` depending on `stage:done?` means stage content is synchronized before final image generation
