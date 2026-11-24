# Dev builds: `gaia build --dev-reuse`

For iterating on a bench board. Not for anything else.

## What it does

A clean build of an OS step starts Buildroot from nothing, so a one-line
config change rebuilds the whole root or the whole kernel. With
`--dev-reuse`, each Buildroot step keeps its output directory under
`<store>/dev-buildroot/<identity>/` and, on the next dev build, rebuilds:

- a package that is new, or whose own config lines, version, downloads,
  dependencies or named files changed, and everything that depends on it;
- for a change to only the kernel's (or U-Boot's) config fragment files: the
  kernel's config, then Kbuild's own incremental build;
- everything, when anything else changed: an architecture, toolchain or
  other global option, a package removed, the Buildroot source, Gaia's
  patches, the host tools, or a build that failed.

The target tree and the images are always made again. The details, and the
exact rules, are in `crates/nodes/os-buildroot/src/dev.rs`.

Each Rust build step (`gaia.build.rust`) keeps its cargo target directory
under `<store>/dev-cargo/<identity>/`, one for each toolchain, image,
vendored crate set, package set, profile, features, target, CPU and
environment. The next dev build compiles what the changed files touch and
reuses every other crate, the packages of the step left unchanged included.
The source is mirrored into that directory, and only files whose bytes
changed get a new time, because cargo goes by times. The details are in
`crates/nodes/build-rust/src/warm.rs`. A description cannot set the step's
`dev` input, and a clean build in which a step has one is refused by name.

## What keeps it apart from a release

**A release is a clean build.** Whatever later builds the release path
builds clean and refuses a run record that says `"build": "dev-reuse"`.

- A dev build's step keys are never a clean build's, so a clean build never
  takes a dev result. Where a later step's input bytes are equal, the
  output is the same object either way: equal bytes are the same artifact.
- Refused at plan time: any step that signs (`gaia.sign`, or a step given a
  signing key's variable: a signed first-flash image, a signed bundle).
  An unsigned disk and first-flash image are allowed, so a dev build can go
  on a board.
- Marked: the run record has `"build": "dev-reuse"`; each output is copied to
  `<store>/dev-outputs/<run>/<output>` with `<output>.gaia-build.json`
  (`{"build":"dev-reuse",...}`) beside it, and that copy is the path printed;
  the build ends with `DEV BUILD — not for release` on stderr. The image's
  own bytes carry no mark.

Today, nothing in Gaia refuses an unsigned output, and the first-flash image
is always unsigned. Whether a board refuses an unsigned image depends on the
board's own security settings, and is not Gaia's to state.

## Check it: `--verify`

`gaia build <product.toml> --dev-reuse --verify` builds dev, then builds the
same product clean, and compares every output that ships, file by file: every
product output, and each Buildroot step's outputs except its build side
(`config`, `kernel-build` and `toolchain`, which are inputs to later steps and
are compared through what those steps make). **Run it before trusting a dev
image for anything but bench iteration.** It is pass or fail: it exits 0 when
everything compared is equal, and non-zero, listing the files, when anything
differs. A difference is a stale reuse: a bug in the fingerprint.

There is no expected noise. Two clean builds of the same inputs, each from an
empty store, are byte-identical in everything `--verify` compares, the disk
image included, because every Buildroot make runs where it sees one fixed
build directory, `/dev/shm/gaia-buildroot/out`, bound over its real one with
`bwrap`, and with the date pinned (`SOURCE_DATE_EPOCH` is Buildroot's release
date, `TZ=UTC`). Without `bwrap` on the build machine a build still runs, in
its real directory, and two builds then differ where that path is written
into a file; `--verify` reports them. What still differs
between two stores is build side only: the Buildroot `.config` names each
store's own path, and some host libraries and Python caches in the toolchain
carry dates; none of them is in the image.

## Disk

Each Buildroot step of each product keeps one warm directory, about the size
of its clean build's output directory (12 GB for a Raspberry Pi kernel and
root), and each Rust step one cargo target. A new Buildroot source, Gaia patch set, host tool set or product
input makes a new one, and nothing removes the old: `<store>/dev-buildroot`
and `<store>/dev-cargo` may be removed whenever no dev build is running.

## Measuring

`GAIA_DEV_REUSE_KCONFIG=dirclean` makes a kernel-config-only change remove
and rebuild the kernel instead of reconfiguring it. It is for comparing the
two. Leave it unset otherwise.
