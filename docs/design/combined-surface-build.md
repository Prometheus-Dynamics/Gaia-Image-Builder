# Design: the combined surface build

Status: proposal, sent to HeliOS and Daedalus before any building. Item 5 of
the shared-library plan.

## The problem

A product image holds programs and surface `cdylib`s (a small C ABI per
dependency, `lemnos-capi` the first). Daedalus's correction: the node crates
that ship in the image (Styx's, Eidos's, Lemnos's) link into the Daedalus
surface `cdylib` itself, with LTO across them. Only plugins built separately,
outside the image build, go through the stable plugin ABI.

So one surface library is built from crates of several repositories, and the
programs in the same image use some of the same crates (a core linked
statically). Three things must hold per image:

1. One version of each crate across the surface and the programs. Two
   versions of a protocol or core crate in one image is a mismatch waiting
   to happen.
2. One feature set: the surface and the programs resolve a shared crate's
   features once, and the image (static or shared, which nodes) picks them.
3. A lock: what built the image is pinned and reproducible.

## Recommendation: a surface crate in the product's workspace

The product owns a small crate per surface in its own cargo workspace, for
example `helios/surfaces/daedalus-image/`:

```toml
[package]
name = "helios-daedalus-image"

[lib]
crate-type = ["cdylib"]

[dependencies]
daedalus-capi = { git = "https://github.com/.../Daedalus.git" }
styx-nodes = { git = "https://github.com/.../Styx.git", optional = true }
eidos-nodes = { git = "https://github.com/.../Eidos.git", optional = true }

[features]
styx = ["dep:styx-nodes"]
eidos = ["dep:eidos-nodes"]
```

Its `lib.rs` names each node crate (`use styx_nodes as _;`) so the nodes'
registrations link in. Gaia builds it as an ordinary Rust artifact with a
declared surface. It goes in the same `build_group` as the product's
programs, which:

- gives one `cargo build -p ...` for all of them: one resolution, one
  feature set, one `Cargo.lock` (the product's, committed);
- makes the per-image choice the item-1 pattern: a layer per image patches
  the group's `features` (`helios-daedalus-image/styx`, the programs'
  `lemnos-shared`, ...);
- serves the other repositories through `cargo_git_sources`, from the
  sources Gaia already pins.

Nothing in Gaia's config changes for the build itself. That is the point:
the combined build is a cargo workspace, and cargo already does it well.

### What Gaia adds

1. **One version per crate per image** (check, warn first). The crate
   records (`.gaia/<file>.crates.json`, written now for "not both") gain
   each crate's version and source. `image:check` warns when two installed
   artifacts of one image resolved the same crate to different versions or
   sources (`crate_version_split`), naming both. With a build group this
   cannot happen inside the group, so it catches what is outside it: a
   program built from another workspace, or a stale artifact.
2. **Plugins as consumers.** Plugins are opened at run time (`dlopen`), so
   the surface consumer check and the std report do not see them today
   (`/usr/lib/helios/plugins/daedalus/libhelios_eidos_plugin.so` in the
   current Raze image). A surface takes `plugin_dirs = ["/usr/lib/helios/plugins"]`:
   each ELF file there is checked as a consumer of that surface (its
   imports against the surface's exports and its own `DT_NEEDED`), and
   counted in its loader's process for "not both" and the std report.

### Not chosen: a workspace Gaia generates

Gaia could write the surface crate itself from config (a list of crates and
features per image, `when` per entry). It keeps the node list in the build
file, but Gaia would own a generated `Cargo.toml`, `lib.rs` and a lock that
lives in the build directory, not the product's repository. Reproducing an
image would then depend on Gaia's generator. Worth it only if the node list
per image changes more often than the product's code does; it can be added
later on top of the same artifact.

## Questions before building

- Daedalus: do node crates register through a link-time mechanism (`linkme`,
  `inventory`) that needs only `use x as _;`, or through an explicit call
  the surface crate makes?
- HeliOS: one surface crate per surface in the HeliOS workspace, or a
  separate "image" workspace that depends on HeliOS's crates too?
- Both: is a version split between the surface and a program in one image
  an error from the start, or a warning first?
