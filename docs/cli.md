# CLI

Current CLI commands:

```bash
gaia --help
gaia --version
gaia resolve <build.toml>
gaia validate <build.toml>
gaia plan <build.toml>
gaia clean <build.toml>
gaia lock <build.toml>
gaia run <build.toml>
gaia tui <build.toml>
```

If no command is provided, Gaia treats the first positional argument as a build path and defaults to `run`.

Flags may appear before or after the build path. Gaia rejects unknown flags,
flags missing their value, malformed `KEY=VALUE` pairs, and extra positional
arguments with exit code `1` instead of silently ignoring them.

The installed `gaia` binary includes terminal UI support by default. Use
`--no-default-features` when building or installing if you need a lean binary
without terminal UI dependencies.

```bash
cargo run -p gaia -- tui <build.toml>
```

For projects with layered configs, put concrete entrypoints in
`configs/builds/*.toml`. Short build names such as `base-os-cm5` resolve through
that directory, and the TUI build picker lists those concrete entrypoints instead
of every imported layer. Running `gaia tui` without a build opens that picker
when project build entrypoints are available.

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

## Running Part Of A Build

`run` and `plan` accept `--only <targets>` to execute a slice of the build graph
plus everything it depends on. Targets are comma-separated or repeated, and are
either a domain (`sources`, `artifacts`, `install`, `stage`, `image`,
`checkpoints`) or an operation id as printed by `gaia plan` (for example
`artifact:helios-engine`).

```bash
# build only the artifacts (and the sources they need)
gaia run configs/builds/cm5.toml --only artifacts

# preview what a single artifact rebuild would execute
gaia plan configs/builds/cm5.toml --only artifact:helios-engine
```

A partial run keeps the reuse state of operations it did not execute, so an
artifacts-only run does not force the next full run to rebuild the image.

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
- the operations that will execute and why
- an estimate from the durations recorded by earlier runs: the critical path
  (longest dependency chain), total work, and any operations with no timing yet
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
- `--target caches`
  Prune cache leftovers without touching build or output directories:
  - git mirrors under `.gaia/cache/git` that no git source of this build uses
    (including mirrors left from the older one-mirror-per-ref layout; a
    mirror used only by another build in the same workspace is pruned too
    and re-created on its next fetch),
  - `.<source>.gaia-preserved` stashes left in `build_dir/sources` by an
    interrupted re-clone,
  - Buildroot `target.refresh` work trees under
    `build_dir/image/buildroot-output/build/buildroot-fs/*/`.

  Gaia reports the size of each removed cache path and the total freed.
- `--all-caches`
  Also remove the shared workspace caches wholesale: `.gaia/cache/git`,
  `.gaia/cache/downloads`, `.gaia/cache/buildroot/dl` and
  `.gaia/docker-cache` (cargo registry/git and sccache for Docker artifact
  builds). They refill on the next build, at the cost of re-downloading.
  Implies `--target caches`; on its own it does not clean build or output
  directories.

Other options:
- `--profile <name>`
  Apply a named `[clean.profiles.<name>]` profile.
- `--path <path>`
  Add an explicit workspace-relative, absolute, or `@alias/...` path.
- `--dry-run`
  Print what would be removed without deleting anything.

When no clean profile, target, or explicit path is provided, Gaia removes
`workspace.build_dir` and `workspace.out_dir`.

```bash
gaia clean configs/builds/cm5.toml --target caches --dry-run
gaia clean configs/builds/cm5.toml --all-caches
```

### `lock`

Resolves the commit each git source's `branch`, `tag` (or `HEAD`) points at
and writes it to the build's lockfile, `<build>.gaia.lock` next to the build
entrypoint (`configs/builds/cm5.toml` locks into
`configs/builds/cm5.gaia.lock`). Commit the lockfile. See
[Git Source Lockfile](configuration.md#git-source-lockfile).

```bash
gaia lock configs/builds/cm5.toml                    # add missing entries, keep existing ones
gaia lock configs/builds/cm5.toml --update           # re-resolve every git source
gaia lock configs/builds/cm5.toml --update orion     # re-resolve one source (comma-separate several)
```

- Without `--update`, entries that still match the source's repo and ref are
  kept as they are; missing and stale entries are resolved and written.
- `--update [source-id]` re-resolves the named sources, or all of them when
  no id follows. Put the build path before `--update` so the id is not taken
  as the build; `--update=<id>` works in any position.
- Entries for sources the build no longer has are dropped.
- Sources pinned with `rev` need no entry and are skipped.

Resolution uses `git ls-remote`, so it works for remote URLs, `file://` URLs
and local repository paths.

### `run`

Prints live execution progress while the plan runs, then:
- a compact progress bar
- completed/total operation counts and percentage
- running operation count
- elapsed time
- current operation id
- throttled heartbeat lines with the latest provider output for long-running commands

Set `GAIA_RUN_PROGRESS=quiet` to disable live progress output for scripts that
only want the final summary.

After execution completes, prints:
- execution summary
- skipped operations (with `policy.failure.keep_going`)
- the slowest operations and their durations
- failure policy
- rollback summary
- failure-class summary
- checkpoint built/reused counts
- report file paths and sizes

### `tui`

Starts the interactive terminal UI for the current build.

This command is available in default `gaia` builds. If the binary is built with
`--no-default-features`, Gaia returns a clear command failure for `tui`.

```bash
gaia tui                              # pick from configs/builds/*.toml
gaia tui --builds-dir path/to/builds  # pick from another directory
gaia tui configs/builds/cm5.toml      # open one build directly
```

Without an explicit build, the TUI opens the build picker when more than one
entrypoint is found and goes straight to setup when there is only one.

Screens:
- **Picker** lists build entrypoints.
- **Setup** edits inputs (enum inputs cycle with `Left`/`Right`, booleans
  toggle, others open a text field), branch, and parallel jobs, and shows
  Overview, Selection, Validation, Plan, Reports, and Spec panels. The Plan
  panel lists every operation with whether the next run will execute or reuse
  it, and why.
- **Monitor** shows progress, every operation with its live status, and
  Overview, Events, Logs, Reports, and Spec views. The selection follows the
  newest running operation until you move it by hand (`f` turns following back
  on). When a run fails, the monitor jumps to the failed operation's logs.

Press `?` on any screen for the key list. `q` or `Ctrl+C` quits; while a
build is running, the first press asks for confirmation, the second cancels the
build and exits once it stops, and a third exits immediately. `Esc` from the
monitor returns to setup without stopping the build, and `m` goes back.

Logs keep the most recent 5,000 lines per operation in the TUI.

## Exit Codes

Current behavior:
- success commands return `0`
- invalid arguments and config load failures return `1`
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
`lock`, `run`, and `tui` in default builds.
