# Gaia Docs

This is the rebuilt documentation set for the current typed Gaia system.

## Read In This Order

1. [configuration.md](configuration.md)
2. [cli.md](cli.md)
3. [execution.md](execution.md)
4. [providers.md](providers.md)
5. [platform-support.md](platform-support.md)
6. [reporting-and-state.md](reporting-and-state.md)
7. [migration-from-legacy.md](migration-from-legacy.md)
8. [future-features.md](future-features.md)
9. [completion-checklist.md](completion-checklist.md)

## What Each Doc Covers

- [configuration.md](configuration.md)
  Full TOML surface for build metadata, inputs, presets, env, workspace, sources, artifacts, install, stage, image, checkpoints, reporting, and policy.

- [cli.md](cli.md)
  Actual supported commands and overrides:
  `resolve`, `validate`, `plan`, `run`, default `tui`, `--preset`, `--env-file`, `--env`, `--set`.

- [execution.md](execution.md)
  How Gaia turns config into a validated spec, then into a typed plan, then into execution with reuse, rollback, and cancellation.

- [providers.md](providers.md)
  Current provider model and what each provider actually does today.

- [platform-support.md](platform-support.md)
  Supported release host, Unix process assumptions, and privileged image-provider requirements.

- [reporting-and-state.md](reporting-and-state.md)
  Machine-readable output files, provider state files, runtime state files, and reuse-state behavior.

- [migration-from-legacy.md](migration-from-legacy.md)
  How to translate old Gaia trees that still think in `buildroot` / `program` / `stage` module buckets.

- [future-features.md](future-features.md)
  Future checkpoint reuse, CI multiplex builds, and first-class image/root filesystem formats to consider.

- [completion-checklist.md](completion-checklist.md)
  The concrete remaining work required before Gaia can be considered done as an OS-image build system.
