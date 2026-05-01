# Full Template Set

This directory is a commented reference set for the current Gaia config model.

Files:
- `base.toml`
  Build metadata, inputs, presets, env, provenance, workspace, policy, reporting.
- `artifacts.toml`
  Sources, artifacts, installs.
- `stage.toml`
  Stage files, env sets, services, checkpoints.
- `image-buildroot.toml`
  Buildroot image contract.
- `image-starting-point.toml`
  Starting-point image contract.

Typical usage:

1. Copy `base.toml`, `artifacts.toml`, `stage.toml`.
2. Choose one image file:
   - `image-buildroot.toml`
   - `image-starting-point.toml`
3. Make a real top-level build file:

```toml
build_name = "my-image"
extends = "base.toml"
imports = ["artifacts.toml", "stage.toml", "image-buildroot.toml"]
```
