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
gaia cache <build.toml>
gaia run <build.toml>
gaia pause [run]
gaia resume [run]
gaia cancel [run]
gaia status [run]
gaia tui [build.toml]
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
- while a Buildroot `make` runs, its own progress first: packages built out of
  the config's packages, the packages being built, and an estimate of the
  time left (from how long each remaining package took when it was last
  built, kept in `<output>/.gaia-package-durations` across cleans, scaled by
  this run's pace), followed by the operation count, e.g.
  `run [######----------]  41% 120/290 packages eta ~35m00s building=linux,mesa3d,openjdk +1 | ops 15/130 ...`

Set `GAIA_RUN_PROGRESS=quiet` to disable live progress output for scripts that
only want the final summary.

Diagnostic logging goes to stderr. By default only `WARN` and `ERROR` events
are printed, one line each, without span context and without ANSI colors when
stderr is not a terminal (`NO_COLOR` also disables colors). To see more, set
`RUST_LOG` to a `tracing` filter, for example:

```bash
RUST_LOG=info gaia run configs/build.toml     # INFO events too (e.g. operation reused/succeeded)
RUST_LOG=gaia_exec=debug gaia run configs/build.toml
```

The `tui` subcommand writes its logs nowhere, so they do not disturb the screen.

After execution completes, prints:
- execution summary
- skipped operations (with `policy.failure.keep_going`)
- the slowest operations and their durations
- failure policy
- rollback summary
- failure-class summary
- checkpoint built/reused counts
- report file paths and sizes

### `pause`, `resume`, `cancel`

Signal a running `gaia run`, from another terminal or a script. With no
argument, the one live run on the system is used, from any directory. When
several are live, the command lists them and exits with status 1; name one
with its number from `gaia status`, its build name or id, or its build config
path:

```bash
gaia pause              # the only live run
gaia pause 2            # run 2 of `gaia status`
gaia cancel cm5         # by build name
```

The run is found through the registry (see [`status`](#status)); a build config
no registered run uses falls back to the pid the run keeps in
`<build_dir>/.gaia-run.pid`. The signals:

- `gaia pause` is Ctrl-Z in the running `gaia run`: every running command is
  stopped (its process group gets SIGSTOP, its docker container is paused)
  and Gaia itself stops. No timeout runs while paused, and operation times
  exclude the pause (`paused_ms` in `summary.json`, `paused` in the run
  summary).
- `gaia resume` is `fg`: the commands continue where they were.
- `gaia cancel` is Ctrl-C: the build stops, keeping its work (see
  [Cancellation](execution.md#cancellation)). It continues a paused run first.

The next `gaia run` with the same inputs says it resumes, reuses the finished
operations and continues the Buildroot build where it stopped.

### `status`

Shows what the `gaia run`s on this system are doing now, from any directory,
another terminal or a script. With no argument it lists every run, one line
each: name, pid, elapsed time, `PAUSED` when paused, operations finished out of
the total, Buildroot packages with the estimated time left, and the current
operation. Live runs come first; a run that ended in the last 24 hours follows,
with its outcome:

```bash
gaia status                   # every run: live, and ended in the last 24 hours
gaia status 2                 # run 2 of that list
gaia status cm5               # by build name or id
gaia status configs/builds/cm5.toml --follow  # a build config, refreshed every second
```

A number, a build name or id, or a build config path selects one run and shows
its detail. Names are matched against the registry first; a build config that no
registered run uses is resolved, and its build dir is read as before. `--follow`
refreshes every second: for one run until it ends, for the list until no run is
live.

Every `gaia run` registers itself in a per-user registry while it runs: one
JSON file per run in `$GAIA_RUNS_DIR`, else `$XDG_RUNTIME_DIR/gaia/runs`, else
`${XDG_STATE_HOME:-~/.local/state}/gaia/runs`. It records the pid, build id and
name, the absolute build config path, the working directory, the command line,
the build dir and the paths of the run's status files. The entry is removed when
the run ends, so a paused run keeps it. A run that ended is kept for 24 hours
under `ended/`, pointing at its final snapshot. Entries of runs that are gone
(a dead pid, or a pid since used by another program) are removed when the
registry is read.

While a run executes, `gaia run` keeps `<build_dir>/.gaia-run.status.json`
up to date (at most once a second, and at every operation start and finish),
also with `GAIA_RUN_PROGRESS=quiet`.

A live run shows its elapsed time, whether it is paused, the operations
finished out of the total (done, reused, failed, cancelled, skipped), the
Buildroot packages with their estimated time left while a `make` runs, the
running operations with their elapsed time and last output line, and the last
10 log lines. Without a live run, it says so and prints how the last run ended
(`completed`, `failed` or `cancelled`), from `<build_dir>/.gaia-run.last.json`.
`--follow` only applies to `status`. Like `pause`, `resume` and `cancel`, it
needs a Unix system.

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

The picker starts with a **Running builds** section, from anywhere on the
system: every `gaia run` in the registry (see [`status`](#status)), live ones
first and runs that ended in the last 24 hours dimmed. Selecting one attaches to
its monitor through the status file the run registered; no build config is
resolved. Outside a project, `gaia tui` opens the picker with only the running
builds, and says so when there are none. `j`/`k` move between the running
builds and the build configs of the directory, `Enter` attaches or opens, and
`r` rescans both.

Runs started from the setup screen of the TUI itself are not registered and
do not appear to other processes; start them with `gaia run`.

When a build already has a `gaia run` in progress (started from another terminal
or a script), the TUI attaches to it instead of offering to start one, and the
picker marks such builds, for example `● running 12m, 83% packages`. The attached
monitor shows the same summary as `gaia status`, refreshed every second, and the
recent output (`PgUp`/`PgDn` scroll, `End` follows). `p` pauses the run, `r`
resumes it, and `c` cancels it (press `c` or `y` again to confirm). `q` leaves
the monitor and the build keeps running; `b` returns to the picker.

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
- a cancelled run (Ctrl-C, `gaia cancel`) returns `130`

The important practical distinction is:
- validation errors stop before planning/execution
- execution errors produce structured failure reports and may trigger rollback according to policy

## What Gaia Does Not Expose Yet

There is no public CLI for:
- custom checkpoint store management in the new rewrite
- interactive config authoring

The supported public path right now is `resolve`, `validate`, `plan`, `clean`,
`lock`, `run`, and `tui` in default builds.
