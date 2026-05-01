# Providers

Gaia has three provider families:
- source providers
- artifact providers
- image providers

## Source Providers

Kinds:
- `git`
- `path`
- `archive`
- `download`

Current capabilities:
- `git`
  real clone / checkout / remote ref resolution
- `path`
  real source identity based on path content/state
- `archive`
  real extraction via `tar`
- `download`
  real download via `curl`

Source providers persist `.gaia-source-state.txt` and include backend-native fields such as:
- refresh policy
- pin policy
- selected ref type/value
- resolved commit sha
- checksum policy/source
- content identity mode

## Artifact Providers

Kinds:
- `rust`
- `go`
- `java`
- `node`
- `python`

Current execution paths:
- Rust: `cargo build`
- Go: `go build`
- Java: Maven/Gradle build + built-target resolution
- Node: `npm pack`
- Python: `python3 -m pip wheel`

Artifact providers persist `.gaia-state.txt` next to outputs and now standardize:
- `resolved_identifier_kind`
- `resolved_identifier`
- `produced_filename`
- `output_class`
- `build_tool`
- `build_tool_version`
- `output_sha256`
- `output_bytes`

Provider-specific examples:
- Rust
  - package
  - target
  - compiler tool/version
- Java
  - build target
  - maven vs gradle vs gradle wrapper
- Node
  - package dir
  - tarball output class
- Python
  - package dir
  - wheel output class
- Go
  - package
  - binary output class

## Image Providers

Kinds:
- `buildroot`
- `starting-point`

### Buildroot

Current reality:
- can run a real Buildroot build when a valid Buildroot tree and environment are present
- otherwise can fall back to materialized outputs for structural/testing flows

Typed Buildroot contract includes:
- `defconfig`
- `external_tree`
- `external_tree_mode`
- expected image list

### Starting Point

Current reality:
- can copy an existing rootfs
- can archive it
- validates rootfs according to typed validation/output mode policy

## Provider Policies

Each provider kind gets retry/timeout policy.

Provider command policies support:
- `retry_attempts`
- `retry_backoff_ms`
- `retry_backoff_strategy`
- `timeout_seconds`

Specialized fields:
- Rust: `allow_nested_build`
- Git: `allow_remote_resolution`

## Provider Errors

Providers now return typed error kinds instead of only free-form strings:
- `ToolStart`
- `Timeout`
- `OutputMissing`
- `BackendCommand`
- `PolicyBlocked`
- `RuntimeState`
- `Unknown`
