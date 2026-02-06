# HeliOS Recipes (Concrete Patterns)

These are copy-and-adapt patterns for building a HeliOS-style Gaia setup.

No external HeliOS repo is required; adjust paths to your own project layout.

## Recipe 1: Cross-compile Rust for arm64 with a container profile

Use a program profile:

```toml
[program.profiles.aarch64-linux]
target = "aarch64-unknown-linux-gnu"
container_image = "helios-cross"

  [program.profiles.aarch64-linux.env]
  DOCKERFILE = "docker/aarch64/Dockerfile.aarch64-rpi4"
  CARGO_TARGET_DIR = "../.gaia-target/aarch64-linux"
```

`workspace_dir` and relative paths in these examples are illustrative; point them at your actual backend/frontend directories.

Then attach artifacts:

```toml
[rust]
workspace_dir = "../backend"

[[rust.artifacts]]
id = "helios-api"
package = "helios-api"
kind = "bin"
profile = "aarch64-linux"
mode = "auto"
```

Why it works: Gaia resolves profile target/env, then either runs host commands or containerized commands based on `container_image`.

## Recipe 2: Build a frontend bundle as a custom artifact

```toml
[custom]
workspace_dir = "../frontend"

[[custom.artifacts]]
id = "helios-frontend"
profile = "host"
mode = "auto"
prebuilt_path = "../frontend/build"
build_command = ["bun", "run", "build"]
output_path = "../frontend/build"
```

Then install it:

```toml
[[items]]
artifact = "helios-frontend"
dest = "/opt/helios/frontend"
```

## Recipe 3: Plugin deployment via `cdylib` + install mapping

Rust artifact:

```toml
[[rust.artifacts]]
id = "daedalus-ai"
package = "helios-daedalus-ai-plugin"
kind = "cdylib"
profile = "aarch64-linux"
mode = "auto"
```

Install mapping:

```toml
[[items]]
artifact = "daedalus-ai"
dest = "/usr/lib/helios/plugins/daedalus/libhelios_daedalus_ai_plugin.so"
mode = 420
```

## Recipe 4: Reusable runtime env sets

Define once in `stage.env.sets`:

```toml
[sets.helios-engine]
ENGINE_METRICS_ADDR = "0.0.0.0:5804"
LOG_FOLDER = "/var/log/helios"
LD_LIBRARY_PATH = "/usr/lib/edgetpu:/usr/lib"
```

Consume in service config:

```toml
[units."helios-engine"]
src = "assets/services/helios-engine/helios-engine.service"
env_set = "helios-engine"
env_file = "/etc/default/helios-engine"
```

Gaia writes the env file and links the unit.

## Recipe 5: Socket activation for IPC daemons

Service config:

```toml
[units."helios-engine.socket"]
src = "assets/services/helios-engine/helios-engine.socket"
unit = "helios-engine.socket"
targets = ["sockets"]

[units."helios-engine"]
src = "assets/services/helios-engine/helios-engine.service"
targets = ["multi-user"]
```

Unit pair details:

- socket: `ListenStream=/run/helios/engine.sock`
- service: `Requires=...socket`, `Also=...socket`

## Recipe 6: First-boot partition provisioning

Service config fragment:

```toml
[units.helios-provision]
src = "assets/services/expand-rootfs/helios-provision.target"
unit = "helios-provision.target"
targets = ["local-fs"]

[units."helios-provision.service"]
src = "assets/services/expand-rootfs/helios-provision.service"
unit = "helios-provision.service"
targets = ["helios-provision", "local-fs"]
assets = [
  { src = "assets/services/expand-rootfs/provisions.toml", dst = "/etc/helios/provisions.toml", mode = 420 },
  { src = "assets/services/storage/helios-data.mount", dst = "/etc/systemd/system/var-lib-helios.mount", mode = 420 },
]
```

This pattern lets a oneshot unit repartition and then mount `LABEL=DATA` at `/var/lib/helios`.

## Recipe 7: Vendor units + custom units together

Networkd and SSH pattern:

```toml
[units."systemd-networkd"]
vendor = true
targets = ["sysinit", "multi-user"]

[units."sshd"]
vendor = true
unit = "sshd.service"
targets = ["multi-user"]
```

Custom service in same config is fine.

## Recipe 8: Ship scripts/assets with units

Attach scripts and config files directly in stage service entries:

```toml
[units."helios-api"]
src = "assets/services/helios-api/helios-api.service"
assets = [
  { src = "assets/services/helios-api/helios-api-startup-sanitize.sh", dst = "/usr/local/bin/helios-api-startup-sanitize.sh", mode = 493 },
  { src = "assets/services/helios-api/startup.toml", dst = "/etc/helios/startup.toml", mode = 420 },
]
```

This keeps runtime payloads colocated with their service definition.

## Recipe 9: Buildroot external tree + package toggles

```toml
[buildroot]
external = ["assets/buildroot"]

[buildroot.packages]
systemd = true
openssh = true
dnsmasq = true
ffmpeg = true
libcamera = true
```

Then add low-level symbol overrides in `[buildroot.symbols]` when package toggles are not enough.

## Recipe 10: CM5 board boot-image control

Use board-specific post-image logic in `assets/buildroot/boards/raspberrypicm5io/post-image.sh` to:

- fetch/extract official Pi5/CM5 boot partition
- copy required firmware/overlays
- generate final `sdcard.img` via `genimage`
- optionally stage EEPROM update files

This is how HeliOS ensures firmware parity with Raspberry Pi image expectations.
