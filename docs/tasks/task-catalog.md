# Built-in Task Catalog

## Core

- `core.init`
  - module: `core`
  - provides: `core:initialized`
- `core.barrier.stage`
  - module: `core`
  - provides: `stage:done`

## Program

- `program.lint.run`
  - provides: `program:linted`
- `program.rust.artifacts`
  - provides: `artifacts:rust`
- `program.java.artifacts`
  - provides: `artifacts:java`
- `program.custom.artifacts`
  - provides: `artifacts:custom`
- `program.install.stage`
  - provides: `stage:program-install`

Condition note:

- artifact/install conditions (`enabled_if` / `disabled_if`) are resolved before planning.
- if a builder has zero selected artifacts, its `program.*.artifacts` task is omitted.
- if `program.install` has zero selected items, `program.install.stage` is omitted.

## Stage

- `stage.render`
  - provides: `stage:content`, `stage:services`

## Buildroot target overlay

- `buildroot.rpi.validate`
  - provides: `buildroot:target-validated`
- `buildroot.rpi.prepare`
  - provides: `buildroot:target-prepared`

## Buildroot pipeline

- `buildroot.fetch`
  - provides: `buildroot:source`
- `buildroot.configure`
  - provides: `buildroot:config`
- `buildroot.build`
  - provides: `buildroot:artifacts`
- `buildroot.collect`
  - provides: `artifacts:rootfs`

Checkpoint note:

- with `[checkpoints]` anchored at `buildroot.build`, `buildroot.configure` and `buildroot.build` may be skipped on restore hit, while `buildroot.collect` still runs.

Starting point note:

- with `[buildroot.starting_point]` enabled, `buildroot.fetch/configure/build` remain in plan but short-circuit, and `buildroot.collect` sources artifacts from the configured external rootfs input (`rootfs_dir`, `rootfs_tar`, or `image`).

## Checkpoints (when configured)

- `checkpoints.restore.buildroot-build`
  - provides: `checkpoints:buildroot-build-restored`
- `checkpoints.capture.buildroot-build`
  - captures checkpoint payload/manifest after `buildroot.build` when restore was not used

## Typical order

A common full run order is:

1. `core.init`
2. optional `program.*` build tasks
3. optional `program.install.stage`
4. `stage.render`
5. `buildroot.rpi.validate/prepare` (if configured)
6. `buildroot.fetch`
7. `buildroot.configure`
8. `buildroot.build`
9. `core.barrier.stage`
10. `buildroot.collect`

In starting-point mode:

1. `core.init`
2. optional `program.*` build tasks
3. optional `program.install.stage`
4. `stage.render`
5. `buildroot.fetch` (skipped)
6. `buildroot.configure` (skipped)
7. `buildroot.build` (skipped)
8. `core.barrier.stage`
9. `buildroot.collect` (collects from starting point)

## HeliOS-cm5 trigger map

Example build file:

- `configs/builds/HeliOS-cm5.toml`

Why each task appears:

- `core.init`
  - always present (`core` module is always detected)
- `program.rust.artifacts`
  - `[program.rust]` imported from `configs/modules/program/rust.toml`
- `program.java.artifacts`
  - `[program.java]` imported from `configs/modules/program/java.toml`
- `program.custom.artifacts`
  - `[program.custom]` imported from `configs/modules/program/custom.toml`
- `program.install.stage`
  - `[program.install]` imported from `configs/modules/program/install.toml`
- `stage.render`
  - `[stage]` imported from stage module files
- `buildroot.rpi.validate` and `buildroot.rpi.prepare`
  - `[buildroot.rpi]` imported from `configs/platforms/pi/*.toml`
- `buildroot.fetch/configure/build/collect`
  - `[buildroot]` imported from distro and platform stack
- `core.barrier.stage`
  - automatically injected by `Plan::finalize_default`
