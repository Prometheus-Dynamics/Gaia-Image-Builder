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
