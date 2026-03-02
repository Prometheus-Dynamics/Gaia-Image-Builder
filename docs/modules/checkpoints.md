# Checkpoints

Source:

- `crates/gaia-image-builder/src/checkpoints.rs`

## Purpose

`[checkpoints]` adds restore/capture behavior to existing build flows without splitting profiles.

Current supported anchor:

- `buildroot.build`

This gives a reusable **base OS checkpoint** before final image post-processing.

## Config shape

- `[checkpoints]`
  - `enabled`
  - `default_use_policy`: `auto|off|required`
  - `default_upload_policy`: `off|on_success|always`
  - `trust_mode`: `verify|permissive`
  - `queue_file` (optional)
  - `[checkpoints.backends.{s3,http,ssh}]` maps
- `[[checkpoints.points]]`
  - `id`
  - `anchor_task`
  - `use_policy` (optional)
  - `upload_policy` (optional)
  - `fingerprint_from` (optional)
  - `backend` (optional, `kind:name` or unique backend name)

Environment-backed backend fields:

- S3 backend:
  - `bucket_env`, `region_env`, `prefix_env`, `endpoint_url_env`, `profile_env`
  - credential source env mapping:
    - `aws_access_key_id_env`
    - `aws_secret_access_key_env`
    - `aws_session_token_env`
    - `aws_shared_credentials_file_env`
    - `aws_config_file_env`
    - `aws_ca_bundle_env`
- HTTP backend:
  - `base_url_env`, `token_env`
- SSH backend:
  - `target_env`, `port_env`, `identity_file_env`, `known_hosts_file_env`
  - transport options: `port`, `identity_file`, `known_hosts_file`, `strict_host_key_checking`

## Runtime behavior (`buildroot.build`)

- restore path:
  - `checkpoints.restore.buildroot-build` attempts restore for anchor `buildroot.build`
  - if local checkpoint is missing and `backend` is set, Gaia probes remote and downloads `manifest.json` + `payload.tar`
  - on hit, `buildroot.configure` and `buildroot.build` are skipped
  - `buildroot.collect` still runs, so stage/program changes still apply
- capture path:
  - `checkpoints.capture.buildroot-build` runs after `buildroot.build`
  - when restore is not used, Gaia captures checkpoint payload into `<build_dir>/checkpoints/...`
  - writes manifest and updates checkpoint index
- starting-point interaction:
  - when `buildroot.starting_point.enabled=true`, Buildroot compile is bypassed
  - restore/capture tasks become no-ops for `buildroot.build` anchor

## Invalidation

Default base checkpoint fingerprint paths include:

- `buildroot.version`
- `buildroot.defconfig`
- `buildroot.packages`
- `buildroot.package_versions`
- `buildroot.symbols`
- `buildroot.external`
- `buildroot.starting_point`
- relevant `buildroot.rpi.*` inputs

Changing these invalidates the base OS checkpoint.
Program/stage-only changes do not.

On misses, status now reports input-diff reasons (example: `inputs_changed:buildroot.packages`).

## CLI

- `gaia checkpoints status <build.toml>`
- `gaia checkpoints retry <build.toml> [--max N]`
- `gaia checkpoints list <build.toml> [--remote] [--id <checkpoint-id>]`

`status` includes:

- `exists` (local object exists)
- `remote_exists` (remote manifest check result when backend is configured)
- `will_download` (restore is expected to download from remote)
- upload queue state/reason fields

`list` includes:

- local fingerprints discovered under `<build_dir>/checkpoints/points/<id>/...`
- optional remote fingerprints from backend listing (`--remote`)

Store consistency:

- checkpoint index/queue/manifest writes use atomic file replacement
- upload/capture mutations acquire a checkpoint store lock (`<build_dir>/checkpoints/.store.lock`)

Remote backend fixture tests (Docker-based, ignored by default):

- `cargo test --test checkpoints_remote_backends -- --ignored`
