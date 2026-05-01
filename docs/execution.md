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

`execution.jobs` only limits Gaia scheduler concurrency. It does not get forwarded
to backend tools. Provider-local worker counts are configured separately through
provider policy, such as `providers.buildroot.local_jobs` for Buildroot `make -j`.
Keep `execution.jobs` and provider-local jobs separate to avoid accidentally
running N Gaia operations where each backend also starts N workers.

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
