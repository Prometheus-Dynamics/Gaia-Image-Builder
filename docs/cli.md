# CLI

Current CLI commands:

```bash
gaia --help
gaia --version
gaia resolve <build.toml>
gaia validate <build.toml>
gaia plan <build.toml>
gaia clean <build.toml>
gaia run <build.toml>
gaia tui <build.toml>
```

If no command is provided, Gaia treats the first positional argument as a build path and defaults to `run`.

The installed `gaia` binary includes terminal UI support by default. Use
`--no-default-features` when building or installing if you need a lean binary
without terminal UI dependencies.

```bash
cargo run -p gaia -- tui <build.toml>
```

## Shared Modifiers

All build-oriented commands support:

```bash
--preset <name>
--env-file <path>
--env KEY=VALUE
--set key=value
```

Semantics:
- `--preset`
  Select a named preset.
- `--env-file`
  Add one more env file at resolve time.
- `--env`
  Add one or more runtime env overrides.
- `--set`
  Apply explicit top-level override values.

Examples:

```bash
gaia resolve examples/default-workspace/configs/default.toml --preset ci
gaia validate examples/default-workspace/configs/default.toml --env GAIA_MODE=release
gaia plan examples/default-workspace/configs/default.toml --set build.version=v2026.2.0
gaia clean examples/default-workspace/configs/default.toml --target all
gaia run examples/default-workspace/configs/default.toml --preset release --env-file secrets.env --set workspace.out_dir=.gaia/examples/default-workspace/out-release
```

## Command Output

### `resolve`

Prints high-level resolved build context:
- selected build file
- preset
- selected inputs
- env files
- env overrides
- explicit overrides
- precedence order
- backend/runtime overview
- failure policy

### `validate`

Prints the same selection/overview context, then validation counts and diagnostics.

### `plan`

Prints selection/overview context, then:
- operation count
- optionality highlights
- runtime domain summaries

### `clean`

Resolves the build config and removes configured files or directories without
planning or running a build.

Built-in targets:
- `--target build`
  Remove `workspace.build_dir`.
- `--target out` or `--target outputs`
  Remove `workspace.out_dir`.
- `--target all`
  Remove both build and output directories.
- `--target configured`
  Use the profile named by `clean.default`.

Other options:
- `--profile <name>`
  Apply a named `[clean.profiles.<name>]` profile.
- `--path <path>`
  Add an explicit workspace-relative, absolute, or `@alias/...` path.
- `--dry-run`
  Print what would be removed without deleting anything.

When no clean profile, target, or explicit path is provided, Gaia removes
`workspace.build_dir` and `workspace.out_dir`.

### `run`

Prints selection/overview context, then:
- execution summary
- failure policy
- rollback summary
- failure-class summary
- checkpoint built/reused counts
- report file paths and sizes

### `tui`

Starts the interactive terminal UI for the current build.

This command is available in default `gaia` builds. If the binary is built with
`--no-default-features`, Gaia returns a clear command failure for `tui`.

Current TUI behavior:
- `Overview` tab for resolved build shape, provider/runtime overview, and failure policy
- `Validation` tab for typed validation diagnostics
- `Plan` tab for operation ordering, optionality, and parallelism shape
- `Run` tab for the latest in-TUI execution summary, runtime overview, errors, and report paths

Current controls:
- `q` quit
- `Tab` / `Left` / `Right` switch tabs
- `Up` / `Down` scroll
- `p` refresh resolve/validate/plan state
- `r` execute the current build and update the `Run` tab

## Exit Codes

Current behavior:
- success commands return `0`
- validation failure returns a non-zero validation code
- execution failure returns a non-zero execution code

The important practical distinction is:
- validation errors stop before planning/execution
- execution errors produce structured failure reports and may trigger rollback according to policy

## What Gaia Does Not Expose Yet

There is no public CLI for:
- custom checkpoint store management in the new rewrite
- interactive config authoring

The supported public path right now is `resolve`, `validate`, `plan`, `clean`,
`run`, and `tui` in default builds.
