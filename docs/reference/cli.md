# CLI Reference

Binary: `gaia` (`crates/gaia-image-builder/src/main.rs`)

## Commands

### `gaia plan <build.toml> [--dot]`

- computes task plan
- prints ordered list or GraphViz DOT

### `gaia resolve <build.toml>`

- prints fully merged TOML after `extends` + `imports`

### `gaia run <build.toml> [--dry-run] [--max-parallel N]`

- executes planned tasks
- `--dry-run`: no task bodies
- `--max-parallel 0`: auto CPU count

### `gaia tui [--builds-dir DIR] [--max-parallel N]`

- interactive build picker/config explorer/runner

## Equivalent cargo commands

```bash
cargo run -- plan <build.toml>
cargo run -- resolve <build.toml>
cargo run -- run <build.toml> --dry-run
cargo run -- run <build.toml> --max-parallel 0
cargo run -- tui
```
