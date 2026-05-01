# Reporting And State

Gaia emits both human-facing CLI output and machine-readable reports.

## Report Files

`run` writes report files under the output report directory, for example:
- `default.summary.json`
- `default.selection.json`
- `default.provenance.json`
- `default.manifest.json`
- `default.rebuild-reasons.json`

The base filename uses canonical `build_name`, not display name.

## Report Types

### Summary

Includes:
- build name/version/description
- operation counts
- completed/reused/rolled back counts
- domain counts
- checkpoint built/reused counts
- failure policy
- failure-class counts

### Selection

Includes:
- requested build
- selected build file
- selected preset
- selected inputs
- env files
- env overrides
- explicit overrides
- precedence order/layers
- failure policy

### Provenance

Includes:
- metadata
- product identity
- provenance identity
- selected inputs
- selected env files / overrides
- source provider list
- artifact provider list
- artifact install identities
- artifact output metadata
- image provider / feed / contract
- backend state collections for all major domains

### Manifest

Includes structured records for:
- operations
- sources
- artifacts
- installs
- stage files
- stage env sets
- stage services
- image outputs
- checkpoints

### Rebuild Reasons

Includes both:
- planned execute/reuse reasons
- rollback/failure-policy outcomes

## Secret Masking

Reporting and CLI both use masking policy:

```toml
[reporting.masking]
enabled = true
replacement = "***"
patterns = ["TOKEN", "SECRET", "PASSWORD"]
```

This applies to env-derived values in selection/provenance output.

## Provider State Files

Current provider/runtime state files:
- sources:
  - `<build_dir>/sources/<source-id>/.gaia-source-state.txt`
- artifacts:
  - file outputs: sibling `.gaia-state.txt`
  - directory outputs: nested `.gaia-state.txt`
- images:
  - `<collect_dir>/.gaia-image-state.txt`
- runtime ops:
  - `<out_dir>/.gaia/runtime/*.state`

## Reuse State

Gaia also persists reuse state including:
- spec fingerprint
- completed operation ids
- per-operation fingerprints
- per-operation output signatures

Reuse is invalidated when:
- spec changes
- operation fingerprint changes
- provider state changes
- runtime state changes
- output signatures change
- outputs disappear

## Corrupt State Handling

Report generation tolerates malformed backend-state lines:
- malformed lines are ignored
- valid `key=value` lines still survive

Reuse-state parsing also ignores malformed entries where safe and rejects unusable state where necessary.
