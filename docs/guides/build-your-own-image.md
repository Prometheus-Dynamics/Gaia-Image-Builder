# Build Your Own OS Image

This is the end-to-end path from zero to a custom Gaia image.

If you want the full production-style blueprint, read:

- [Recreate HeliOS From Scratch](recreate-helios-from-scratch.md)
- [HeliOS Recipes](helios-recipes.md)

## 1) Start from the minimal example

Use `examples/helios/` as the base:

- `examples/helios/build.toml`
- `examples/helios/configs/*.toml`
- `examples/helios/assets/etc/*`

Copy it:

```bash
cp -r examples/helios examples/myos
```

## 2) Set workspace scope

Edit `examples/myos/configs/workspace.toml`:

```toml
[workspace]
root_dir = "examples/myos"
```

This keeps builds/artifacts isolated under your example folder.

## 3) Pick board + Buildroot defaults

Edit `examples/myos/configs/buildroot.toml`:

- choose `defconfig`
- optionally set `version`, `collect_out_dir`, archive settings, performance knobs

Minimal usable form:

```toml
[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"
```

## 4) Define rootfs overlay content

Edit `examples/myos/configs/stage.toml` and assets under `examples/myos/assets/`.

Typical first files:

- `/etc/hostname`
- `/etc/os-release`
- `/etc/motd`

## 5) Validate before full build

```bash
cargo run -- resolve examples/myos/build.toml
cargo run -- plan examples/myos/build.toml
cargo run -- run examples/myos/build.toml --dry-run
```

## 6) Run real build

```bash
cargo run -- run examples/myos/build.toml --max-parallel 0
```

`0` means use all CPU cores.

## 7) Find outputs

By default:

- Gaia runtime/manifests: `<workspace.out_dir>/<build-name>/gaia/`
- staged rootfs overlay: `<workspace.build_dir>/stage/<build-name>/rootfs`
- collected images: `<workspace.out_dir>/<build-name>/gaia/images` (unless `collect_out_dir` overrides)

## 8) Add services and programs

- Add systemd units/assets in `[stage.services]`.
- Add application build pipelines in `[program.rust]`, `[program.java]`, or `[program.custom]`.
- Stage produced binaries with `[program.install]`.

That is the full path to a production-style custom OS image in Gaia.
