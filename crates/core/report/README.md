# `gaia-report`

Reporting layer for Gaia execution results.

## Purpose

- render summaries and future provenance/manifests from the spec, plan, and execution outcome

## Owns

- final summary models
- future provenance output
- future manifest/report generation
- future rebuild reason reporting

## Does Not Own

- config loading
- planning
- execution runtime behavior

## Depends On

- `gaia-spec`
- `gaia-plan`
- `gaia-exec`
- `gaia-validate`

## Current Task List

- [x] Define a `RunSummary` type that can be rendered in CLI output.
- [x] Add a provenance model tied to `ResolvedBuildSpec` and execution results.
- [x] Add manifest/report output structs for machine-readable output files.
- [x] Emit machine-readable report files with visible output paths and sizes during `run`.
- [x] Add rebuild-reason models once planning and execution expose typed reuse decisions.
- [x] Ensure this crate consumes typed results rather than scraping logs or raw config.
