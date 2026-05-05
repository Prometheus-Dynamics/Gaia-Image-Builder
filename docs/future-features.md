# Future Features

This document tracks feature areas that are useful but not yet first-class in
Gaia. They are intentionally kept separate from the current configuration and
execution docs so the supported surface stays clear.

## Improved Checkpoint System

The checkpoint system should grow from runtime state tracking into a real
storage-backed reuse layer. The target workflow is multiplex image building:
many release images share the same expensive base, then install different
artifact versions, configuration sets, services, or target-specific overlays on
top.

The CI-oriented model should look like this:

1. Build or restore a base OS image checkpoint.
2. Fan out a matrix of final image builds that all consume that base.
3. Rebuild the base only when its inputs change.
4. Rebuild individual final images when their application artifacts, configs,
   presets, or release metadata change.
5. Publish final image outputs and reports as release artifacts.

This matters most in GitHub CI and similar systems where runners are often
ephemeral. Gaia's local reuse state is useful, but the expensive base needs a
portable checkpoint object that can survive between jobs and workflow runs.

Recommended checkpoint improvements:

- Add explicit restore and upload phases, rather than only recording that a
  checkpoint operation completed.
- Define checkpoint content contracts: what files, provider state, runtime
  state, output signatures, and metadata are included.
- Support remote checkpoint backends, starting with a simple filesystem backend
  and then CI-friendly storage such as GitHub Actions cache/artifacts or OCI
  registry blobs.
- Compute checkpoint keys from the anchored operation fingerprint plus relevant
  provider/tool versions, source identities, defconfigs, and image format
  contracts.
- Allow a checkpoint to be restored as an input to later operations, not only
  captured after an operation.
- Distinguish immutable content-addressed checkpoints from mutable aliases such
  as `latest-main-base`.
- Emit report data that explains whether each checkpoint was restored, missed,
  uploaded, skipped, or invalidated.
- Provide CLI commands for checkpoint inspection and maintenance, such as
  `checkpoint list`, `checkpoint restore`, `checkpoint upload`, and
  `checkpoint prune`.

The intended build graph should allow a base image checkpoint to sit between
the expensive OS construction phase and a family of final release images:

```text
base sources + base config + Buildroot/toolchain
  -> base image
  -> checkpoint:base-os
      -> release image: product-a stable
      -> release image: product-a beta
      -> release image: product-b stable
      -> release image: product-b debug
```

For GitHub Actions, this can initially be approximated by caching `.gaia/...`
state and outputs with carefully chosen cache keys. A first-class Gaia backend
would make that less brittle by moving cache identity, restore validation, and
upload policy into Gaia itself.

Open design questions:

- Whether checkpoints should store complete image directories, compressed
  archive blobs, mounted filesystem snapshots, or backend-specific state.
- How much of the workspace should be restored when a downstream build consumes
  a checkpoint.
- Whether downstream builds should be able to mutate a restored checkpoint in
  place or must copy it into a new image workspace.
- How checkpoint locking should work when several CI jobs discover the same
  missing base at once.
- How long mutable checkpoint aliases should be retained compared with
  content-addressed checkpoint blobs.
