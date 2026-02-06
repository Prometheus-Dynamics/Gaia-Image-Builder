# Config Cheatsheet

## Minimal image config

```toml
imports = [
  "./configs/workspace.toml",
  "./configs/buildroot.toml",
  "./configs/stage.toml",
]
```

`workspace.toml`

```toml
[workspace]
root_dir = "examples/helios"
```

`buildroot.toml`

```toml
[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"
```

`stage.toml`

```toml
[stage]

[[stage.files]]
src = "assets/etc/hostname"
dst = "/etc/hostname"
mode = 420
```

## Common sections

- `[build]`: metadata like `version`.
- `[workspace]`: path roots and cleanup mode.
- `[buildroot]`: Buildroot pipeline behavior.
- `[buildroot.rpi]`: board-specific prep.
- `[program]` + `[program.*]`: app artifact builds.
- `[stage]`: rootfs files, env, services.
