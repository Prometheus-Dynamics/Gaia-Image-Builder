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
- image assembly runtime state
- output hygiene warnings for publish directories
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
- image assembly
- output hygiene warnings
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

Image assembly reuse is also invalidated when declared assembly inputs change, including staged file sources, glob match sets, transform inputs, BusyBox helper inputs, and resolved tool signatures where relevant.

## Output Hygiene

Gaia reports publish-directory hygiene warnings without failing the build. These warnings are visible in provenance and manifest reports, and are included in the summary warning count.

Current warnings include:
- known transient directories in image publish directories, such as `.cache`, `build`, `buildroot-output`, `sources`, or `target`
- large non-output files in publish directories

The defaults can be tuned at runtime:

```toml
[reporting.output_hygiene]
large_file_threshold_bytes = 104857600
transient_dir_names = [".cache", "build", "buildroot-output", "sources", "target"]
```

Expected image names, configured image archives, assembly filesystem outputs, assembly disk outputs, and Gaia state files are not treated as unexpected large files.

## Corrupt State Handling

Report generation tolerates malformed backend-state lines:
- malformed lines are ignored
- valid `key=value` lines still survive

Reuse-state parsing also ignores malformed entries where safe and rejects unusable state where necessary.
