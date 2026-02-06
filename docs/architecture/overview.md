# Architecture Overview

Gaia is a config-driven task orchestrator for OS image builds.

## End-to-end flow

1. CLI entrypoint loads a build TOML (`plan`, `run`, `resolve`, `tui`) in `crates/gaia-image-builder/src/main.rs`.
2. Config loader resolves `extends` + `imports` into one merged document in `crates/gaia-image-builder/src/config/mod.rs`.
3. Built-in modules are discovered from `crates/gaia-image-builder/src/modules/mod.rs`.
4. Each detected module contributes tasks into a `Plan` (`crates/gaia-image-builder/src/planner/mod.rs`).
5. Plan finalization injects `core.barrier.stage` to synchronize all `stage:*` providers.
6. Executor resolves task dependencies and runs tasks sequentially or in parallel (`crates/gaia-image-builder/src/executor/mod.rs`).
7. Modules emit runtime artifacts/manifests under the workspace `build/` and `out/` trees.

## Core subsystems

- `config/`: TOML merge engine (`extends`, `imports`).
- `workspace/`: root/build/out path resolution, alias expansion, cleanup mode.
- `planner/`: DAG ordering with dependency tokens.
- `executor/`: runtime task engine, logging, dry-run, cancellation, parallel scheduling.
- `modules/`: domain logic (`buildroot`, `program`, `stage`, `buildroot.rpi`, `core`).

## Build pipelines

Gaia composes three functional pipelines:

- `buildroot`: fetch/configure/build/collect OS artifacts.
- `program`: build or ingest app artifacts (rust/java/custom), then install into stage overlay.
- `stage`: render rootfs overlay files/env/systemd units.

`buildroot.collect` runs after `stage:done?` so image generation happens after stage overlay is complete.
