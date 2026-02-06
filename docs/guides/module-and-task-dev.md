# Module And Task Development

This guide is for extending Gaia itself.

## 1) Add a module

Create a module file under `crates/gaia-image-builder/src/modules/`.

A module must implement:

- `id()`
- `detect(doc)`
- `plan(doc, plan)`

or use `#[Module(...)]` macro.

## 2) Add tasks

Use `#[Task(...)]` macro from `gaia-image-builder-macros`.

Each task declares:

- `id`
- `module`
- `phase`
- `provides`
- `after`
- `default_label`

Task runtime is implemented as:

```rust
fn run(cfg: &Self, doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()>
```

## 3) Register module + task executors

- add module in `modules::builtin_modules()`
- register task executors in `executor::builtin_registry()`

## 4) Add config docs and tests

- document new config table under `docs/modules/` and `docs/reference/`
- add tests under `crates/gaia-image-builder/tests/`

## 5) Validate

```bash
cargo test
cargo run -- plan <build.toml>
cargo run -- run <build.toml> --dry-run
```
