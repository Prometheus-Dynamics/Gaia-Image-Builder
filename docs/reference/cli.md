# CLI Reference

Binary: `gaia` (`crates/gaia-image-builder/src/main.rs`)

## Commands

### `gaia init [DIR] [--force]`

- writes a minimal scaffold (`build.toml`, `configs/*`, `assets/etc/*`)
- default target dir is `./gaia`
- pass `.` to scaffold in the current directory
- refuses to overwrite scaffold files unless `--force` is set

### `gaia plan <build.toml> [--dot] [--set KEY=VALUE ...]`

- computes task plan
- prints ordered list or GraphViz DOT
- `--set`: override build inputs declared under `[inputs.options.*]`

### `gaia resolve <build.toml> [--set KEY=VALUE ...]`

- prints fully merged TOML after `extends` + `imports`
- includes computed `[inputs.resolved]` values

### `gaia run <build.toml> [--dry-run] [--max-parallel N] [--set KEY=VALUE ...]`

- executes planned tasks
- `--dry-run`: no task bodies
- `--max-parallel 0`: auto CPU count
- `--set`: override build inputs before planning/execution

### `gaia checkpoints status <build.toml> [--set KEY=VALUE ...]`

- prints per-checkpoint decision state before execution
- includes `exists`, `remote_exists`, `will_use`, `will_download`, `will_rebuild`, `will_upload`, `pending_upload`, and reason

### `gaia checkpoints retry <build.toml> [--max N] [--set KEY=VALUE ...]`

- retries failed/pending checkpoint uploads from queue
- `--max N`: retry at most N entries

### `gaia checkpoints list <build.toml> [--remote] [--id <checkpoint-id>] [--set KEY=VALUE ...]`

- lists checkpoint objects/fingerprints from local store
- with `--remote`, also queries configured backend fingerprints for each point
- `--id`: filter output to a single checkpoint id

### `gaia tui [--builds-dir DIR] [--max-parallel N]`

- interactive build picker/config explorer/runner
- includes an `Inputs` screen (Quick menu or `i`) for editing `[inputs.options.*]` values before run
- booleans use direct toggles; inputs with `choices` use a selector (no free-form typing required)
- `d` resets selected input to default; `D` resets all inputs to defaults
- persists per-build TUI overrides (including input values) to `<build-dir>/.gaia/<build.toml>.tui-overrides.toml`

## Equivalent cargo commands

```bash
cargo run -- init
cargo run -- init my-image
cargo run -- plan <build.toml>
cargo run -- plan <build.toml> --set pv_jar_source=release
cargo run -- resolve <build.toml>
cargo run -- run <build.toml> --dry-run
cargo run -- run <build.toml> --max-parallel 0
cargo run -- checkpoints status <build.toml>
cargo run -- checkpoints retry <build.toml>
cargo run -- checkpoints list <build.toml> --remote
cargo run -- tui
```
