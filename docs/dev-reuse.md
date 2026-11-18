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
same product clean, and compares every output and every Buildroot step's
outputs, file by file. **Run it before trusting a dev image for anything but
bench iteration.** It exits non-zero and lists the files when anything
differs.

Some files differ between any two builds of the same inputs, dev or not, and
are expected in the list: a package's build date (busybox), the build
directory's path written into a file (the libstdc++ gdb helper, the dbus user
socket, udev's `hwdb.bin`), files whose layout depends on that path's
length (util-linux's `mount`, `umount`, `fsck`, `libmount`), and the host
programs the kernel's build compiles for itself (`kernel-build`'s
`scripts/…`, `lib/crc/gen_crc*table`, the toolchain's `bin/linux-dtc`),
which run on the build machine and are not in the image. A difference
anywhere else is a stale reuse: a bug in the fingerprint.

Until those files are made the same in every build, `--verify` exits
non-zero for every product that has them, and the outputs made from the
root (its file system image, the kernel command line that names its hash,
the boot files, the bundle, the disk) are listed too. Read the list: a dev
build is good when every Buildroot step's lines are among the files above.

## Disk

Each Buildroot step of each product keeps one warm directory, about the size
of its clean build's output directory (12 GB for a Raspberry Pi kernel and
root). A new Buildroot source, Gaia patch set, host tool set or product
input makes a new one, and nothing removes the old: `<store>/dev-buildroot`
may be removed whenever no dev build is running.

## Measuring

`GAIA_DEV_REUSE_KCONFIG=dirclean` makes a kernel-config-only change remove
and rebuild the kernel instead of reconfiguring it. It is for comparing the
two. Leave it unset otherwise.
