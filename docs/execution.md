# Execution Model

Gaia runs in five logical phases:

1. Load raw TOML config
2. Merge, interpolate, and compile into `ResolvedBuildSpec`
3. Validate the resolved spec
4. Plan a typed execution graph
5. Execute the graph and emit reports

## Planning

The planner emits typed operations for:
- sources
- artifacts
- installs
- stage files
- stage env sets
- stage services
- image build
- checkpoints
- report emission

Each operation carries:
- stable `OperationId`
- typed kind
- dependency ids
- optionality
- parallelism metadata
- reuse decision
- fingerprint

## Optionality

Operations are explicitly labeled:
- `required`
- `conditional`
- `best-effort`

Current important case:
- checkpoint optionality is derived from `use_policy` and `upload_policy`

Plan validation rejects required operations that depend on best-effort ones.

## Parallelism

Operations are also labeled with parallelism metadata.

Current planner intent:
- sources: parallelizable
- artifacts: parallelizable
- installs/stage: exclusive runtime domain
- image: exclusive
- checkpoints: exclusive
- report: exclusive

The executor uses a synchronous scoped-thread scheduler rather than an async runtime.
That is intentional: Gaia's hot path is external subprocess and filesystem
orchestration, not high-volume socket I/O. Each runnable operation is executed on a
scoped worker thread, and the main executor loop schedules ready operations,
receives completion events over channels, and applies rollback/cancellation
decisions.

`execution.jobs` limits Gaia scheduler concurrency. It is not forwarded to
backend tools as-is. Provider-local worker counts are configured separately
through provider policy, such as `providers.buildroot.local_jobs` for Buildroot
`make -j`.

To avoid running N heavy operations where each backend also starts one worker
per core, the scheduler gives each CPU-heavy operation (artifact builds and
Buildroot prepare/build) a job budget when others run alongside it: the
available cores divided by the number of heavy operations expected to run at
once (those already running, the one starting, and ready ones that free job
slots allow). The budget is exported as `CARGO_BUILD_JOBS`, `MAKEFLAGS=-jN` and
`CMAKE_BUILD_PARALLEL_LEVEL` (never overriding values you set) and used for
Buildroot's `make -j` when `local_jobs = 0`. An operation that runs alone gets
no budget, so single-operation behavior is unchanged. The split is static per
operation: an operation keeps the budget it started with.

Nested Rust artifacts that share a workspace, target, profile, feature flags
and backend are started as one scheduling unit and built with a single
`cargo build -p a -p b ...` (see `providers.rust.batch_builds`). Each artifact
still reports its own events, result, state and timing.

Streamed build output goes to the live sink (console progress, TUI) as it
arrives. Only a bounded tail (`execution.output_retention.failure_tail_lines`)
is kept per operation and reported for failures; successful operations do not
re-emit their log, so long builds use constant memory.

This keeps provider code straightforward:
- external tools use blocking `std::process::Command`
- subprocess stdout/stderr are drained by `gaia-process`
- timeouts and cancellation are enforced in the shared process runner
- process groups are terminated on Unix so spawned descendants do not survive a timeout

Async should only be introduced if Gaia gains a concrete I/O multiplexing need
that scoped threads and bounded process readers do not solve.

## Reuse

Reuse is not just “did this operation run before”.

Gaia compares:
- whole-spec fingerprint
- per-operation fingerprint
- backend/tool signatures
- persisted provider state files
- persisted runtime state files
- output signatures

A reused operation must still have matching state and expected materialized outputs.

## Cancellation

Executor supports cancellation-aware execution.

Outcome tracks:
- `cancelled`
- `cancelled_operation_id`

Cancellation is separate from failure.

Cancellation is propagated through a shared `ProcessCancelCheck`. Providers pass
that check into command helpers, which allows long-running subprocesses to be
terminated without waiting for the backend tool to exit on its own. The executor
stops scheduling new operations after cancellation or first failure, then waits
for running operations to finish cleanup before recording the cancelled outcome.

## Failure Handling

Failure policy is typed:

```toml
[failure]
rollback_on_error = true
preserve_failed_outputs = false
rollback_domains = ["artifacts", "images"]
```

Behavior:
- when `rollback_on_error = true`, Gaia rolls back completed current-run outputs
- when `preserve_failed_outputs = true`, the failed op’s partial outputs are kept
- `rollback_domains` restrict which completed domains get cleaned up
- when `rollback_on_error = false`, Gaia leaves current-run outputs in place

By default the first failure stops every running sibling. With
`keep_going = true`, independent operations keep running to completion;
operations that depend on a failed one are skipped (a `Skipped` event and
`skipped_operation_ids` in the run summary), and the run still fails. Finished
work is not rolled back under `keep_going`: it is recorded in the reuse state
so the next run reuses it, and only the failed operations' own partial outputs
are cleaned per the policy above. Cancellation still rolls back as usual.

## Timing

Every operation's wall-clock duration is recorded in the run outcome and in
`summary.json` (`operation_timings`), and `gaia run` prints the slowest
operations. The last duration of each operation that executed is saved in the
reuse state (`dur=<operation id>;<ms>` lines). `gaia plan` and the TUI Plan
panel use these to estimate the next run: the total work of the operations that
will execute and the critical path, the dependency chain with the largest
summed duration (a lower bound on wall-clock time). Operations without a
recorded duration are listed as untimed.

## Failure Classification

Execution failures are classified into stable buckets:
- `MissingSpec`
- `MissingProvider`
- `ToolStart`
- `Timeout`
- `OutputMissing`
- `BackendCommand`
- `PolicyBlocked`
- `RuntimeState`
- `Unknown`

These classes appear in reports and CLI output.

## Checkpoints

Checkpoint anchors are typed and validated.

Current allowed anchor domains:
- `image`
- `install:<id>`
- `stage-file:<id>`
- `stage-env:<id>`
- `stage-service:<id>`

Current semantic rules:
- anchor target must exist
- anchor target must be part of the active image feed when anchoring to install/stage domains
- required/conditional checkpoints cannot anchor outside the image dependency chain

## What Is Real vs Placeholder

Real today:
- source materialization
- artifact builds for Rust, Go, Java, Node, Python
- starting-point image assembly
- Buildroot invocation when environment is correctly prepared

Not turnkey today:
- generic OS image builds with zero backend/environment setup
- fully mature Buildroot contract modeling for every real-world board flow
