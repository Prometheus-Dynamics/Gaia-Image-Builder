# Gaia Docs

This is the rebuilt documentation set for the current typed Gaia system.

## Read In This Order

1. [configuration.md](configuration.md)
2. [cli.md](cli.md)
3. [execution.md](execution.md)
4. [providers.md](providers.md)
5. [platform-support.md](platform-support.md)
6. [reporting-and-state.md](reporting-and-state.md)
7. [ci.md](ci.md)
8. [migration-from-legacy.md](migration-from-legacy.md)
9. [future-features.md](future-features.md)

## What Each Doc Covers

- [configuration.md](configuration.md)
  The build file: loading, conditions, top-level fields, interpolation and secrets, with one page per part:
  [inputs and variants](configuration-inputs.md), [policy, workspace and clean](configuration-policy.md),
  [sources and toolchains](configuration-sources.md), [artifacts, install and stage](configuration-artifacts.md),
  [image](configuration-image.md), [image assembly](configuration-assembly.md) and
  [checkpoints, registry and reporting](configuration-checkpoints.md).

- [reference/config-keys.md](reference/config-keys.md)
  Every table and key a build file takes, generated from the layout Gaia validates against (`gaia config keys`).

- [cli.md](cli.md)
  Actual supported commands and overrides:
  `resolve`, `validate`, `plan`, `run`, default `tui`, `--preset`, `--env-file`, `--env`, `--set`.

- [ci.md](ci.md)
  Gaia on CI runners: the GitHub Action, annotations and the job summary, fast per-push jobs, caches, secrets, self-hosted runners.

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
