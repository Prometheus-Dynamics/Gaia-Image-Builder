# Gaia Image Builder

Gaia Image Builder is a Rust workspace for composing OS images from typed TOML build definitions.

Current Gaia is organized around canonical build domains:
- `sources`
- `artifacts`
- `install`
- `stage`
- `image`
- `checkpoints`
- `clean`
- `reporting`
- `policy`

The rewrite is no longer the old `buildroot` / `program` / `stage` bucket model. Legacy config compatibility was explicitly revoked. If you have an older Gaia tree, migrate it to the new typed config model described in [docs/migration-from-legacy.md](docs/migration-from-legacy.md).

## Install

```bash
cargo install --git https://github.com/Prometheus-Dynamics/Gaia-Image-Builder gaia
```

## Commands

```bash
# show the installed Gaia version
gaia --version

# resolve the final typed build
cargo run -q -p gaia -- resolve examples/default-workspace/configs/default.toml

# validate it
cargo run -q -p gaia -- validate examples/default-workspace/configs/default.toml

# inspect the execution plan
cargo run -q -p gaia -- plan examples/default-workspace/configs/default.toml

# remove build/output directories without running a build
cargo run -q -p gaia -- clean examples/default-workspace/configs/default.toml --target all

# use the interactive terminal UI
cargo run -q -p gaia --features tui -- tui examples/default-workspace/configs/default.toml

# execute it
cargo run -q -p gaia -- run examples/default-workspace/configs/default.toml
```

Supported CLI modifiers:
- `--preset <name>`
- `--env-file <path>`
- `--env KEY=VALUE`
- `--set key=value`

## Where To Start

- [docs/README.md](docs/README.md): documentation index
- [docs/configuration.md](docs/configuration.md): full config model
- [docs/cli.md](docs/cli.md): command-line behavior
- [docs/execution.md](docs/execution.md): planning, execution, rollback, cancellation, reuse
- [docs/providers.md](docs/providers.md): source, artifact, and image providers
- [docs/reporting-and-state.md](docs/reporting-and-state.md): reports, runtime state, provider state, reuse state
- [docs/migration-from-legacy.md](docs/migration-from-legacy.md): how to translate old Gaia configs
- [docs/completion-checklist.md](docs/completion-checklist.md): concrete remaining work before Gaia is actually done
- [examples/README.md](examples/README.md): commented template configs

## Repository Reality

This repo now contains:
- active rewrite crates under `crates/`
- a default workspace fixture under `examples/default-workspace/`
- `examples/templates/` for user-facing config templates

The active remaining work is tracked in [docs/completion-checklist.md](docs/completion-checklist.md).
