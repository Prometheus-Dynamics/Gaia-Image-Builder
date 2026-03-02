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
- `[inputs]`: user-selectable build inputs/toggles consumable by conditions.
- `[stage]`: rootfs files, env, services.
- `[checkpoints]` + `[[checkpoints.points]]`: in-flow checkpoint restore/capture policy.

## Build inputs (`--set`)

```toml
[inputs.options.pv_jar_source]
type = "string"
default = "release"
choices = ["release", "local", "repo"]

[inputs.options.use_driver]
type = "bool"
default = false
```

Runtime override:

```bash
cargo run -- run <build.toml> --set pv_jar_source=repo --set use_driver=true
```

Condition usage in program artifacts/install items:

```toml
[[program.custom.artifacts]]
id = "driver-jar"
enabled_if = ["use_driver=true"]
output_path = "inputs/driver.jar"
build_command = ["bash", "scripts/fetch-driver.sh"]

[[program.custom.artifacts]]
id = "photonvision-jar"
enabled_if = ["pv_jar_source=repo"]
after_artifacts = ["driver-jar?"]
output_path = "inputs/photonvision.jar"
build_command = ["bash", "scripts/build-photonvision.sh"]
```

## Single-flow base OS checkpoint

Additive example (no module splitting):

```toml
[checkpoints]
enabled = true
default_use_policy = "auto"
default_upload_policy = "off"
trust_mode = "verify"

[[checkpoints.points]]
id = "base-os"
anchor_task = "buildroot.build"
use_policy = "auto"
upload_policy = "off"
fingerprint_from = [
  "buildroot.version",
  "buildroot.defconfig",
  "buildroot.packages",
  "buildroot.package_versions",
  "buildroot.symbols",
  "buildroot.external",
]
```

Behavior:

- checkpoint hit: `buildroot.configure`/`buildroot.build` are skipped via restore.
- checkpoint miss: full buildroot compile runs, then checkpoint is captured.
- changing buildroot package/symbol/version inputs invalidates the checkpoint.
- changing Rust/program content does not invalidate this base OS checkpoint.

Remote backend example:

```toml
[checkpoints.backends.http.cache]
base_url = "https://cache.example.com/checkpoints"
token_env = "GAIA_CHECKPOINT_TOKEN"

[[checkpoints.points]]
id = "base-os"
anchor_task = "buildroot.build"
backend = "http:cache"
upload_policy = "on_success"
```

S3 with env-provided credentials:

```toml
[checkpoints.backends.s3.cache]
bucket_env = "GAIA_CP_S3_BUCKET"
endpoint_url_env = "GAIA_CP_S3_ENDPOINT"
prefix = "gaia"
aws_access_key_id_env = "GAIA_CP_AWS_KEY"
aws_secret_access_key_env = "GAIA_CP_AWS_SECRET"
```

SSH with env-provided identity:

```toml
[checkpoints.backends.ssh.cache]
target = "builder@cache-host:/srv/gaia-checkpoints"
port_env = "GAIA_CP_SSH_PORT"
identity_file_env = "GAIA_CP_SSH_KEY"
strict_host_key_checking = false
```

## Build without Buildroot (external starting point)

Use a pre-existing rootfs directory, tarball, or disk image while keeping the same Gaia flow:

```toml
[buildroot.starting_point]
enabled = true
rootfs_tar = "inputs/base-rootfs.tar"
apply_stage_overlay = true

[buildroot.starting_point.packages]
enabled = true
manager = "auto"
execute = false
install = ["htop", "curl"]
remove = ["nano"]
release_version = "bookworm"
allow_major_upgrade = false
```

Notes:

- `buildroot.fetch/configure/build` are skipped automatically.
- `buildroot.collect` copies starting-point rootfs into collect output and applies stage/program content.
- package manager detection uses files in the imported rootfs (`apt`, `dnf`, `apk`, etc.).
- set `execute = true` to run package commands in `chroot` (requires root).
- for disk images, set `image = "inputs/base.img"` and optionally `image_partition = "2"` (or `p2` / full `/dev/...`); image mode requires root because Gaia loop-mounts partitions.
