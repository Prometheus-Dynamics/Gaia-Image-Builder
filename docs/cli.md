# CLI

Current CLI commands:

```bash
gaia --help
gaia --version
gaia resolve <build.toml>
gaia validate <build.toml>
gaia plan <build.toml>
gaia preview <build.toml>
gaia clean <build.toml>
gaia lock <build.toml>
gaia cache <build.toml>
gaia cache stats [build.toml]
gaia run <build.toml> [--export <dir> | --no-export]
gaia pause [run]
gaia resume [run]
gaia cancel [run]
gaia status [run]
gaia tui [build.toml]
gaia config schema
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
`configs/builds/*.toml`. Short build names such as `base-os-evk` resolve through
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
  Apply explicit top-level override values. The key must name a setting gaia
  knows, or `input.<name>` for an input the build declares. Any other key
  fails the command, with a `did you mean` hint for the closest known key,
  rather than being ignored. Presets are held to the same rule.
- `--variant <id>[,<id>]`
  For `run`, `preview` and `plan` on a build that declares `[[variants]]`:
  build only the named variants (`default` is the base config). Without it,
  every variant is built. Using it on a build with no variants is an error.

## Running Part Of A Build

`run` and `plan` accept `--only <targets>` to execute a slice of the build graph
plus everything it depends on. Targets are comma-separated or repeated, and are
either a domain (`sources`, `artifacts`, `install`, `stage`, `image`,
`checkpoints`) or an operation id as printed by `gaia plan` (for example
`artifact:acme-engine`).

```bash
# build only the artifacts (and the sources they need)
gaia run configs/builds/evk.toml --only artifacts

# preview what a single artifact rebuild would execute
gaia plan configs/builds/evk.toml --only artifact:acme-engine
```

A partial run keeps the reuse state of operations it did not execute, so an
artifacts-only run does not force the next full run to rebuild the image.

## Forcing Rebuilds

`--rebuild <targets>` executes the named operations whatever the reuse state
says. Targets take the same forms as `--only` (a domain or an operation id),
and an id may end in `*` to match several (`--rebuild 'artifact:*'`). Each
forced operation shows the reason `rebuild_requested` in `gaia preview`, and
its dependents follow the normal rules: they execute as `dependency_rebuilt`,
and content-based reuse still skips any whose rebuilt inputs turn out
unchanged. An unknown id is an error that suggests the close matches.

`--rebuild image:prepare` on a build with no prepare step (a plan without
`image:prepare`, such as a build with no variants and no artifact after
`image:prepare`) is accepted and means `--rebuild image:build`: that build does
its whole Buildroot make in `image:build`. Use `--rebuild image:build` directly
there; it is enough on its own to force the make.

Staged files and services are reused by the content they deliver: a unit file
referenced through `@assets/...` is hashed by its bytes, not its timestamp, so
editing it reruns its stage operation and the image build, and a rebuilt
artifact with new bytes reruns the install and the image build instead of
being cut off as unchanged.

`--rebuild-package <package>[,...]` is for Buildroot images. The named packages
are uninstalled and their build directories removed before `make`, they are
not restored from the package cache in that run, and the cache entry is stored
again from the new build. It also makes `image:prepare` and `image:build`
execute. `gaia preview` lists the packages it would clean. Only the named
packages are cleaned, not the packages that depend on them.

```bash
# rebuild the jar and everything that consumes it
gaia run configs/builds/evk.toml --rebuild artifact:myapp-jar

# rebuild one Buildroot package from source, showing what that would clean
gaia preview configs/builds/evk.toml --rebuild-package libcamera
```

`gaia resume` does not repeat either flag: a resumed run should not redo the
work that already finished.

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
The last line is the verdict, so `gaia validate build.toml | tail -1` tells a
pass from a failure:

```text
validate: ok (0 error(s), 1 warning(s))
validate: FAILED (2 error(s), 0 warning(s))
```

The exit code is unchanged: `0` when valid, `2` for validation errors. A build
config that does not resolve at all fails with `validate: FAILED (the build
config did not resolve)` on stderr, after the error, and exit code `1`.

### `plan`

Prints selection/overview context, then:
- operation count
- optionality highlights
- the operations that will execute and why
- an estimate from the durations recorded by earlier runs: the critical path
  (longest dependency chain), total work, and any operations with no timing yet
- runtime domain summaries

### `preview`

Shows what `gaia run` would do, without changing anything: the same plan as
`gaia plan`, then for each operation whether it runs or is reused, and why.
`gaia run <build.toml> --dry-run` is the same command.

For a Buildroot image that would run, it adds:
- the work dir: the build dir, RAM, or another directory; whether a RAM tree
  is created, kept, or whether a disk tree would be moved into RAM (which
  discards it), or RAM is too small and the build falls back to disk
- the config: what the defconfig, fragments, overrides and cache settings
  change against the last build
- host tools: which ones come from the system and which are built
- the clean a run would make, from the same decision the run acts on: nothing,
  finalize only, the packages rebuilt and uninstalled with reasons, or a full
  clean with its reasons
- the package cache: which packages would be restored, built, or built because
  the image reads their sources, and which cache entries would be evicted
- every path the run would remove or move aside, and the leftovers of an
  earlier clean that would be purged

The config steps run on a scratch copy of the tree's state files in the
temporary directory, so the real output tree is not touched. A preview cannot
see files a `make` creates later, the post-image steps, or the output of
assembly; those are not previewed. It may create the empty download and cache
directories a run would create.

The last line is the verdict, for example:

```text
preview: no clean, 3 packages rebuilt (mesa3d, openjdk, qt6base), 412 deleted paths
preview: FULL CLEAN of /path/to/build/image/buildroot-output (reason: effective config changed (no snapshot of the previously built config))
```

Options:
- `--json` prints the whole report as JSON, including every deletion path.
- `--fail-on-clean` exits with `3` when a run would clean the whole tree or
  delete anything other than leftovers of an earlier clean. Use it in scripts
  and CI to stop before a surprising clean.
- `--only`, `--set`, `--env` and `--preset` apply as for `run` and `plan`.

Pin bumps: when an earlier run left a reuse state, the line after the header
says what the changes invalidate. A direct operation has an input of its own
that changed; a cascaded one runs only because a dependency runs. A changed
git pin reports its rev, and a dependent says which dependency runs:

```text
preview of 'app-rev': 4 operation(s), 4 would execute
preview: pin/input changes invalidate 2 operation(s) (2 direct, 0 cascaded): source:upstream, image:build
run   resolve-build: plan_resolution_required: build resolution always executes for a fresh plan
run   source:upstream: operation_fingerprint_mismatch: operation 'source:upstream' will execute because its rev changed: 3175ad9 -> e565dcf
run   image:build: materialized_output_missing: operation 'image:build' will execute because its expected materialized outputs are missing
run   report:emit: report_emission_required: report emission always runs at the end of a plan
```

A dependent of a re-materialized git source reads `because source:upstream runs
(operation_fingerprint_mismatch)`, naming the dependency's own reason; with several
dependencies it lists the first three and counts the rest.
It is reused when the new checkout has the same files (same tree digest), which
only the run can check, so the preview lists it as cascaded. Without an earlier
run the line reads `no earlier run recorded: N operation(s) run`.

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
  - shared cargo target dirs under the user cache (`<cache>/cargo-target/<key>`)
    whose packages were all built from source directories that no longer
    exist (the `gaia-owners` claims name them). A dir with no claims, or
    with a claim whose source still exists, is kept,
  - `.<source>.gaia-preserved` stashes left in `build_dir/sources` by an
    interrupted re-clone,
  - Buildroot `target.refresh` work trees under
    `build_dir/image/buildroot-output/build/buildroot-fs/*/`.

  Gaia reports the size of each removed cache path and the total freed.
- `--unused-mirrors` (with `--target caches`)
  Also remove git mirrors under `.gaia/cache/git` that no git source of this
  build uses, and no cargo git dependency (`cargo_git_sources`) of this
  build maps to. Mirrors are shared by the whole workspace, so this prunes a
  mirror another build still needs (it is re-created on that build's next
  fetch). Use it only when this build is the workspace's only build.
- `--all-caches`
  Also remove the shared workspace caches wholesale: every git mirror under
  `.gaia/cache/git`, `.gaia/cache/downloads`, `.gaia/cache/buildroot/dl`,
  `.gaia/docker-cache` (cargo registry/git and sccache for Docker artifact
  builds), `.gaia/cache/import-sources` (git import checkouts) and
  `.gaia/docker-home` (the tool homes of Docker builds). They refill on the
  next build, at the cost of re-downloading.
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
gaia clean configs/builds/evk.toml --target caches --dry-run
gaia clean configs/builds/evk.toml --all-caches
```

### `lock`

Resolves the commit each git source's `branch`, `tag` (or `HEAD`) points at
and writes it to the build's lockfile, `<build>.gaia.lock` next to the build
entrypoint (`configs/builds/evk.toml` locks into
`configs/builds/evk.gaia.lock`). Commit the lockfile. See
[Git Source Lockfile](configuration.md#git-source-lockfile).

```bash
gaia lock configs/builds/evk.toml                    # add missing entries, keep existing ones
gaia lock configs/builds/evk.toml --update           # re-resolve every git source
gaia lock configs/builds/evk.toml --update worker    # re-resolve one source (comma-separate several)
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
- the critical path (see [Run profile](#run-profile))
- the slowest operations and their durations
- failure policy
- rollback summary
- failure-class summary
- checkpoint built/reused counts
- report file paths and sizes

#### Exporting the image

`gaia run <build.toml> --export <dir>` copies the run's primary image output
into `<dir>` after a successful run (no errors, not cancelled). The primary
output is the image archive the report lists, or, when the build leaves its
output as a collect directory, the disk images inside it. Each copy gets a
versioned name:

- a name that already carries a version number (such as `v2027.0.0` or `9.9.9`) is kept, so
  `myapp-full-myboard-dev-v2027.0.0-alpha-2-94-gc9548d91.img.xz` stays as it is;
- otherwise the name becomes `<build-name>-<version>.<ext>`, or
  `<build-name>-<UTC run time>.<ext>` when the build has no version.

An existing file with different content is never overwritten: the copy takes
`-1`, `-2`, ... before the extension. A file with identical content is reused.
Each exported file prints one line:

```text
exported: /home/me/images/myapp-full-myboard-dev-v2027.0.0-alpha-2-94-gc9548d91.img.xz  sha256 <hex>  412345678 bytes
```

A failed or cancelled run exports nothing and says so with
`export: skipped, the run did not succeed; nothing was exported`. A copy that
fails exits `1`.

The directory can also come from the build config, so `--export` is not needed
on every run:

- `[image.output] export_dir = "<dir>"` for one build;
- `[workspace] export_dir = "<dir>"` for every build in the workspace, used
  when the build sets none.

Both are also `--set image.output.export_dir=<dir>` and
`--set workspace.export_dir=<dir>`. An absolute path is used as written, `~/`
expands to the home directory, and any other relative path is taken from the
workspace root. An empty value is an error. Precedence, highest first:
`--export <dir>`, then the build's `export_dir`, then the workspace's.
`--export` paths are relative to the current directory. `--no-export` skips
any configured export for that run; combining it with `--export` is a usage
error.

#### Run profile

After a run, the summary prints the run's actual critical path, from the
recorded start and finish time of each operation and the plan's dependencies:

```text
critical path 9m40s: source:corelib 5s → artifact:acme-api 3m18s → image:prepare 21s → image:build 4m12s → image:assembly 58s
```

It is the chain ending at the operation that finished last; each step back
takes the dependency that finished last, the one the operation waited for.
Reused operations take no time and are passed through to their own
dependencies. The total runs from the start of the first operation on the path
to the end of the last, without the time those operations were paused; gaps
on the path are time spent waiting for a free job slot. A path of more than
nine operations shows the first and last four. The summary report
(`<build>.summary.json`) has it as `critical_path`, and each entry of
`operation_timings` has `start_ms` (milliseconds since execution began, pauses
included) besides `duration_ms` and `paused_ms`. `gaia plan` shows the
*estimated* critical path from earlier runs' durations.

Each run also writes `<build>.trace.json` beside the reports: the run as
[Chrome trace events](https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU),
to open in `chrome://tracing` or [Perfetto](https://ui.perfetto.dev). Each
operation is one slice, on the first row free when it started, so parallel
operations sit on separate rows. Its args hold the status, the duration, the
time paused, whether it is on the critical path, and the steps it timed. The
assembly operation's steps (filesystems, disks, transforms, ...) are slices
inside it; only their durations are recorded, so they are laid end to end from
the operation's start, and steps under 100 ms are not reported.

#### CI output

With `--ci`, or when the `CI` environment variable is set to anything but
empty, `0` or `false` (GitHub Actions, GitLab CI and most CI services set
`CI=true`), `gaia run` prints no live progress lines or heartbeats. Each
operation's output is kept until the operation finishes, then printed on stderr
as one GitHub Actions log group, so the log folds per operation and parallel
operations never interleave:

```text
run: 42 operation(s); CI output, one log group per operation
source:corelib: reused
::group::artifact:acme-api: done in 3m18s
...the operation's output...
::endgroup::
::group::image:build: failed: make exited with status 2 in 4m12s
...
::endgroup::
::error title=gaia image:build failed::make exited with status 2
```

An operation that wrote nothing is one line, without a group. A failure is
repeated as an `::error::` annotation. Output lines that look like workflow
commands (`::...`) are wrapped in `::stop-commands::` so a build log cannot
issue one. The last 5,000 lines of each operation are kept. The live status
files (`gaia status`) and the final summary are unchanged; `GAIA_RUN_PROGRESS=quiet`
still turns console output off entirely.

#### Resuming a failed or cancelled run

When a `gaia run` fails or is cancelled, its last line is the command that
picks the run back up, with the same build path and the same `--preset`,
`--env-file`, `--env`, `--set` and `--only` arguments:

```text
resume: gaia run configs/heli.toml --preset ci --set workspace.out_dir=/srv/out  # finished work is reused
```

Finished work is reused by the next run, so the resume line repeats the
invocation rather than starting over. When a rollback unwound completed
operations (`policy.failure.rollback_completed`), the comment says those run
again instead. Secret values in `--env` and `--set` are masked as the report
shows them (`***`); the line then says to enter them again.

### `prefetch`

`gaia prefetch <build.toml>` (the same as `gaia run <build.toml> --prefetch`)
downloads what a build needs and stops before anything is built:

- every source is materialized: git mirrors and checkouts, archives and
  downloads, as a run would fetch them;
- for Buildroot, the tree is configured exactly as a run configures it
  (defconfig, fragments, `config_overrides`, the cache settings), then
  `make source` downloads the source of every enabled package into the
  download cache (`BR2_DL_DIR`). Nothing is extracted, built or installed, and
  no image is written.

The output names the source files the run added, and the size of the download
cache. `--json` works as for `run`. A prefetch with `--offline`, `--only`,
`--rebuild`, `--rebuild-package`, `--export` or `--dry-run` is a usage error,
and so is a build with `[[variants]]`. A prefetch records no reuse state: the
next run plans its operations again as usual, and the Buildroot base is built
then, never taken from the prefetch.

Prefetch does not yet fetch the git dependencies of cargo artifacts
(`cargo_git_sources`): the first artifact build does that, so an offline build
of such an artifact needs that build to have run once online.

### Offline builds

`gaia run <build.toml> --offline` (or `[execution] offline = true` in the
build config) builds without network access. Each provider then uses only
what is already local:

- Buildroot: `BR2_PRIMARY_SITE` is the download cache (a `file://` URL),
  `BR2_PRIMARY_SITE_ONLY=y` and `BR2_BACKUP_SITE` is empty, so a source missing
  from the cache fails the make instead of reaching the network. The failure
  names the package and suggests `gaia prefetch`. These settings say only where
  files come from: they do not change the Buildroot output fingerprint or the
  package cache keys, so an offline build reuses what an online one built.
  Turning offline on or off reruns the configuration steps once.
- Git sources: a remote checkout comes from the cached mirror. No `ls-remote`
  and no fetch run. A floating branch or tag resolves to the commit the mirror
  holds as of its last fetch; a locked or `rev` commit the mirror lacks is an
  error that names it. A remote source with no mirror is an error. Plan-time
  ref resolution and the opt-in `dynamic_inputs` git lookups are skipped
  offline, so use a `gaia lock` or prefetch a build online before going offline.
- Cargo artifacts: `CARGO_NET_OFFLINE=true`, so cargo uses only the registry and
  git caches already on the machine.
- Docker: the execution image is run with `--pull=never`, so a missing image
  fails the run instead of being pulled. Docker-backed Buildroot, starting-point
  and source commands are not yet covered by this flag.

To prepare an offline build, run `gaia prefetch <build.toml>` on a connected
machine, then `gaia run <build.toml> --offline` on the offline one, with the
same workspace and cache directories (see `docs/configuration.md`, Download
Cache).

### `cache`

`gaia cache` lists, prunes and clears the Buildroot package cache (see
[Buildroot package cache](configuration.md)). `gaia cache stats` lists every
cache Gaia keeps for the build with its path, disk usage and file count, then
the total:

```text
git-mirrors                  1.2 GiB      18342 files  /work/.gaia/cache/git
downloads                        -                    /work/.gaia/cache/downloads  (not present)
buildroot-dl                 3.4 GiB       1210 files  /home/me/.cache/gaia/buildroot/dl
buildroot-packages (project) 612.0 MiB     40211 files  /work/.gaia/cache/buildroot/packages
ccache                       9.8 GiB     201113 files  /home/me/.cache/gaia/buildroot/ccache
cargo-target/3f9c2a1b4d5e   2.1 GiB      30022 files  /home/me/.cache/gaia/cargo-target/3f9c2a1b4d5e
total: 17.1 GiB in 290898 files, 11 of 16 caches present (hard links counted once)
```

The caches: git mirrors, downloads and import-source checkouts under
`<workspace>/.gaia/cache`; the Buildroot download dir (configured, else under
the user cache root, and the workspace fallback); the package cache levels;
shared Buildroot output trees; the compiler cache; `.gaia/docker-cache` and
`.gaia/docker-home`; and under the user cache root the shared
`cargo-target/<key>` dirs, `gradle-home` and `java-home`, and the user-wide
Buildroot defaults (`buildroot-dl (user)`, `buildroot-packages (user)`,
`ccache (user)`) when this build's config points elsewhere, since other builds
use them. Usage is counted like
`du`: allocated blocks of files and directories, a file with several hard
links once (also across caches, for the total), symlinks counted as links and
never followed. A cache inside another listed one is left out of the total.
Without a build config that resolves (and none named), it reports the
user-wide caches and the current directory's workspace caches.

### `config schema`

`gaia config schema` prints the JSON Schema of a build file to standard
output, without reading any build config. It is written from the keys the
validation checks (see [Unknown keys](configuration.md)), so an editor
completes and flags the same keys `gaia validate` does. Save it and point
taplo or VS Code's Even Better TOML at it; the configuration reference shows
both settings:

```bash
gaia config schema > gaia-build.schema.json
```

### `pause`, `resume`, `cancel`

Signal a running `gaia run`, from another terminal or a script. With no
argument, the one live run on the system is used, from any directory. When
several are live, the command lists them and exits with status 1; name one
with its number from `gaia status`, its build name or id, or its build config
path:

```bash
gaia pause              # the only live run
gaia pause 2            # run 2 of `gaia status`
gaia cancel evk         # by build name
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
gaia status evk               # by build name or id
gaia status configs/builds/evk.toml --follow  # a build config, refreshed every second
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
gaia tui configs/builds/evk.toml      # open one build directly
```

Without an explicit build, the TUI opens the build picker when more than one
entrypoint is found and goes straight to setup when there is only one.

The picker starts with a **Builds on this system** section, from anywhere on the
system: every `gaia run` in the registry (see [`status`](#status)), live ones
first and runs that ended in the last 24 hours dimmed. Selecting one attaches to
its monitor through the status file the run registered; no build config is
resolved. Outside a project, `gaia tui` opens the picker with only those builds. `j`/`k` move between the running
builds and the build configs of the directory, `Enter` attaches or opens, and
`r` rescans both.

Runs started from the setup screen of the TUI itself are not registered and
do not appear to other processes; start them with `gaia run`.

When a build already has a `gaia run` in progress (started from another terminal
or a script), the TUI attaches to it instead of offering to start one, and the
picker marks such builds with a progress bar and their elapsed time. The attached
monitor is the run dashboard (see Monitor below), refreshed every second from
the run's status file. `p` pauses or resumes the run (`r` resumes it), and `c`
cancels it (press `c` or `y` again to confirm). `q` leaves the monitor and the
build keeps running; `b` returns to the picker.

Every screen has the same frame: a header line with the screen's name and the
build or run it is about, the screen's body, and a footer with its key hints.
`?` lists the keys of the screen you are on. Below 40x8 the TUI only says that
the terminal is too small.

Screens:
- **Start** (the picker) lists the builds on the system as cards: the running
  ones with a progress bar, operations done, elapsed time and an estimate, and
  the recent ones that ended in the last day with their outcome. Below them, the
  build configs of the directory show their target and profile and the outcome,
  duration and age of their last run.
- **Setup** is a form on the left (branch, target, profile, inputs, variant on
  builds that declare `[[variants]]`, jobs) with a validation badge on each
  value, and a plan preview on the right: how many operations run and how many
  are reused, why they run, the recorded time of what runs (marked `~`), the
  validation outcome, and the Start button. The Start button says why it cannot
  start. `Left`/`Right` change a value; `d` opens the details.
- **Details** has five tabs, `Left`/`Right`, `Tab`, or `1`-`5`: **Plan** (a table
  of each operation, RUN or REUSE, with its reason and last duration; `f` filters
  to run or reuse), **Validation** (errors, warnings and plan problems, grouped),
  **Reports** (the last run's outcome, its slowest operations and image steps as
  bars, its images and errors), **Spec** (the resolved build in sections) and
  **Events** (the run's output: `/` searches, `o` shows one operation, `F`
  follows the newest line). `Esc` returns to setup.
- **Monitor** is the run dashboard, also shown for an attached run. It has a
  header (state, variant, elapsed time, ETA), an overall progress gauge weighted
  by how long each operation took last time, and an operations table: each
  operation's state, time, expected time, the difference from it, and why it
  runs. The selection follows the running operation until you move it (`f`
  filters: all, running, failed, slow; `s` sorts: plan order, duration, delta).
  `Enter` opens an operation's details: why it runs, its time, its packages and
  step times, its error, and the end of its output. The five chart tabs
  (`Left`/`Right`, `Tab`, or `1`-`5`) are Progress (this run's progress against
  the last run's: a braille line for this run, a dotted line for the last run,
  and a marker for now), Resources (CPU, memory, tmpfs, load and disk), Steps and
  Packages (the image build's step times and Buildroot packages), and Summary,
  which is last and shows the outcome, slowest operations, images and errors
  once the run is over. The output pane follows the newest line (`F` holds it in
  place), filters to the selected operation (`l`), searches (`/`), and resizes
  (`+`/`-`). On a narrow terminal the table and the charts take turns (`v`).

Press `?` on any screen for the key list. `q` or `Ctrl+C` quits; while a
build is running, the first press asks for confirmation, the second cancels the
build and exits once it stops, and a third exits immediately. `Esc` from the
monitor returns to setup without stopping the build, and `m` goes back.

Logs keep the most recent 5,000 lines per operation in the TUI.

## JSON Output

`--json` makes `run`, `preview` (`run --dry-run`), `plan`, `status`,
`validate` and `cache` print one JSON document on stdout instead of their
text output. Progress, logs and warnings stay on stderr, so
`gaia run build.toml --json > result.json` captures only the document. The
exit code is the same as without the flag. `--follow` does not combine with
`--json`, and other commands reject it.

Every document is an object with:

| Field | Meaning |
| --- | --- |
| `schema_version` | `1`. Fields are only added within a version; a rename or removal bumps it. |
| `command` | `run`, `preview`, `plan`, `status`, `validate` or `cache` |
| `exit_code` | the process exit code |
| `status` | per command, below; `error` when the command could not run |
| `error` | with `status: "error"`: the message (also printed on stderr) |

Per command:

- `run`: `status` is `succeeded`, `failed` or `cancelled`. `build`,
  `run_duration_ms`, `summary` (the object written to `<build>.summary.json`:
  counts, `operation_timings`, `critical_path`, `sizes`, `errors`, ...),
  `report_files` (`kind`, `path`, `bytes`; `kind` is `summary`, `selection`,
  `provenance`, `manifest`, `rebuild-reasons` or `trace`),
  `validation_diagnostics`, `plan_diagnostics`, `execution_errors`
  (`operation_id`, `code`, `message`, `output_tail`), `post_build_output`,
  `finish` (the export and resume lines) and `export_failed`.
  A build with variants has `header` and `variants`, one object per variant
  with its `id`, `exit_code` and the fields above.
- `plan`: `status` is `ok`, or `diagnostics` when the plan has some. `build`,
  `operation_count`, `execute_count`, `operations` (`id`, `depends_on`,
  `optionality`, `execute`, `reason` `{code, message}` or null, `reused_from`),
  `estimate` (`executing_operations`, `timed_operations`, `total_work_ms`,
  `critical_path_ms`, `critical_path`, `untimed_operations`) and `diagnostics`
  (`code`, `message`).
- `validate`: `status` is `ok` or `failed`. `build`, `error_count`,
  `warning_count` and `diagnostics` (`severity` `error` or `warning`, `code`,
  `location`, `message`).
- `preview`: `status` is `ok`, or `tripped` when `--fail-on-clean` trips; the
  report fields as before (`build`, `verdict`, `invalidation`, `operations`,
  `images` with every deletion, ...).
- `status`: with no run named, `runs`; with one, `run`. A run has `number`
  (its position in the listing), `pid`, `build_id`, `build_name`,
  `display_name`, `build_config`, `build_dir`, `cwd`, `argv`, `started_at`
  (unix seconds), `state` (`running`, `paused`, `ended`, or `none` for a build
  dir with no run), `outcome` and `ended_at` for an ended run, `errors`, and
  `status`: the live status snapshot as in `.gaia-run.status.json` (or the
  final one). A build config no registered run uses gives `display_name`,
  `build_dir` and the state fields.
- `cache`: `action` says which. `list`: `entries` (`level`, `package`, `key`,
  `version`, `bytes`, `relocatable`, `legacy`, `paths`), `levels` (`level`,
  `dir`, `entries`, `bytes`) and `legacy` (`entries`, `bytes`). `remove` and
  `clear`: `dry_run`, `removed` and `total_bytes` (`clear` also has `target`).
  `stats`: `caches` (`name`, `path`, `exists`, `nested`, `bytes` allocated,
  `apparent_bytes`, `files`, `dirs`) and `total` (`bytes`, `apparent_bytes`,
  `files`, `dirs`).

```bash
gaia run build.toml --json | jq '.summary.critical_path.operations[].operation_id'
gaia cache stats --json | jq '.caches[] | select(.exists) | [.name, .bytes]'
```

## Exit Codes

Current behavior:
- success commands return `0`
- invalid arguments and config load failures return `1`
- validation failure returns a non-zero validation code
- execution failure returns a non-zero execution code
- a cancelled run (Ctrl-C, `gaia cancel`) returns `130`
- `gaia preview --fail-on-clean` returns `3` when a run would clean the whole
  tree or delete something other than leftovers of an earlier clean

The important practical distinction is:
- validation errors stop before planning/execution
- execution errors produce structured failure reports and may trigger rollback according to policy

## What Gaia Does Not Expose Yet

There is no public CLI for:
- custom checkpoint store management in the new rewrite
- interactive config authoring

The supported public path right now is `resolve`, `validate`, `plan`, `clean`,
`lock`, `run`, and `tui` in default builds.
