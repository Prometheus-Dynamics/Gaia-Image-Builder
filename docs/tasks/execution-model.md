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

## Checkpoint-aware execution (single-flow)

When `[checkpoints]` is enabled and a point anchors at `buildroot.build`:

- Gaia runs restore/capture orchestration tasks around Buildroot compile steps.
- on restore hit, `buildroot.configure` and `buildroot.build` short-circuit.
- `buildroot.collect` still executes, so current stage/program content is reflected in final images.

## Starting-point execution (Buildroot bypass)

When `buildroot.starting_point.enabled=true`:

- `buildroot.fetch`, `buildroot.configure`, and `buildroot.build` short-circuit.
- `buildroot.collect` imports from `buildroot.starting_point.rootfs_dir`, `rootfs_tar`, or `image`.
- stage/program outputs are still applied in the same flow.
- optional `buildroot.starting_point.packages` reconciliation can detect package manager/version from imported rootfs metadata and plan/execute install/remove commands.
- checkpoint restore/capture around `buildroot.build` are skipped as no-ops in this mode.

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
