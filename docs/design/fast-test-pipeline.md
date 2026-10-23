# Fast test pipeline: Gaia design (parts 1–3)

**Status: proposal, awaiting approval. Nothing here is implemented yet.**
Plan: HeliOS `docs/docs/fast-test-pipeline-plan.md` (f529ee63). This covers Gaia's parts:
image checks (tier 2a), layered outputs, and the OCI writer with a `sim-x86_64` target. The
emulated kernel boot (2b) comes later and is out of scope here.

Constraints carried over: no new crates (the ELF, FDT and OCI pieces are small hand-written
readers and writers; compression uses the external `xz`/`zstd`/`gzip` tools Gaia already
calls), every feature is opt-in or additive, and each piece is pushed when its gate passes.
Expected growth of the `gaia` binary: under 300 KB.

## 1. Image checks (tier 2a)

A new operation, `image:check`, runs after `image:build` and the assembly. It only reads
trees and files, takes seconds, and is reused when nothing it reads changed. Each finding is
an error (the run fails) or a warning (shown in the summary). Results go to
`<out>/<build>.checks.json` and the run summary.

### 1.1 Kernel modules

For Buildroot images with `BR2_LINUX_KERNEL=y`, this is on by default (`[image.checks]
kernel_modules = false` turns it off). Inputs are found automatically: `target/lib/modules/<release>`,
`build/linux-*/Module.symvers`, and the kernel actually shipped (the assembly's boot file, such
as `kernel8.img`, or `images/Image`). Each can be overridden.

Checks, per shipped `.ko` (and `.ko.xz`/`.ko.zst`, decompressed first):

- **Kernel release:** read from the shipped kernel image itself (its `Linux version …`
  string), not from the build dir, so a kernel and modules from different builds are caught.
- **vermagic:** each module's `.modinfo` vermagic must equal the shipped kernel's release
  plus flags (`7.2.9-v8-16k SMP preempt mod_unload modversions aarch64`).
- **Symbols:** every undefined symbol must be exported by the kernel (`Module.symvers`, vmlinux)
  or by another module that ships (its `__ksymtab` entries). With `CONFIG_MODVERSIONS=y` (Raze
  has it), each symbol's CRC in the module's `__versions` must equal the exporter's CRC. A
  mismatch is exactly the "loads on one kernel, refuses on another" failure.
- **Dependencies:** each `depends=` module ships or is built in, and `modules.dep` in the
  image agrees with the modules present (re-derived, not trusted).
- **Duplicates:** a module name shipped twice (Raze has `kernel/…/ov9282.ko` and
  `updates/ov9282.ko`) is reported with which one `modprobe` loads. Both are checked.
- **Firmware:** each `firmware=` entry is present under `/lib/firmware` (any compression).
  This is a warning by default, since many drivers list optional firmware, and an error for
  modules named by the hardware list (1.2).

The reader is a small ELF64 parser (sections `.modinfo`, `__versions`, `__ksymtab*`,
`.symtab`/`.strtab`). A full Raze tree has 252 modules and 12,622 exports, which should take
well under a second.

### 1.2 Supported hardware, per board

The board recipe declares what the image must support. Atlas owns these lists; the paths
inside them are Styx's and the kernel owners'.

```toml
[[image.hardware]]
id = "ov9782"
kind = "camera-sensor"                        # free-form label for reports
modules = ["ov9282"]                          # module names (or built in)
compatible = ["ovti,ov9782"]                  # device-tree compatible the driver must claim
overlays = ["ov9782"]                         # .dtbo that must ship in the boot overlays dir
files = [                                     # tuning, Styx sensor description, etc.
  "/usr/share/libcamera/ipa/rpi/pisp/ov9782.json",
  "/usr/share/styx/sensors/ov9782.toml",
]
firmware = []
```

Each entry passes only if its whole chain is there and consistent:

- every module passes 1.1;
- the module that `modprobe` actually picks claims each `compatible` (its `of:` aliases, as in
  `modules.alias`);
- each overlay ships, and its fragments name that `compatible`. That needs a small FDT reader;
  an overlay with no driver claiming it is an error;
- the files and firmware are present in the final rootfs (after the feed, the exclusions and
  the layer split).

A missing or broken chain fails the build and names the board, the item and the link that
broke. Overlays enabled with `dtoverlay=` in the shipped `config.txt` are also checked to
exist.

### 1.3 Image manifest and diff

- **Manifest:** every image gets `<out>/<image>.contents.json`. It lists every path with its
  type, mode, uid/gid, size, sha256 and link target; the kernel release; each module with its
  vermagic and `srcversion`; the Buildroot packages with versions and installed sizes; and the
  size of each layer.
- **Last good:** `gaia image accept <build>` (run by tier 3, or by hand after a good board
  test) stores the current manifest as the last good one under `.gaia/state/`.
- **Diff:** every run compares against the last good manifest. The summary shows the kernel
  release, module changes (added, removed, vermagic or srcversion changed), firmware, overlays,
  `/etc`, systemd units, and the size change per package and per layer. The full diff goes to
  `<out>/<image>.contents.diff`. It's informational by default; `[image.checks] fail_on =
  ["kernel", "modules-removed"]` and `max_size = { app = "8 MiB", … }` make chosen changes errors.

### 1.4 Units (cheap and included)

- Every enabled unit's `*.wants` link resolves.
- Each `ExecStart=` binary exists.
- Each ELF's `NEEDED` libraries resolve within the image. The runtime-closure logic for
  assembly already exists and is reused.

### What 1 catches, and what it doesn't

The bench-versus-fresh-board incident: if the shipped driver doesn't match its kernel, through
its vermagic, a missing or CRC-mismatched symbol, a missing dependency or overlay, or another
copy taking precedence, the build fails with the reason. A driver that loads but fails at
`probe` is caught only by the emulated boot (2b) or a board.

## 2. Layered outputs

### 2.1 Layers from one rootfs

- **The unit is a tar:** Buildroot already applies ownership, device nodes and users under
  fakeroot when it makes the rootfs. Gaia asks Buildroot for `rootfs.tar` as well, so the
  ownership is right, and splits that tar into one tar per layer. Each layer tar is
  deterministic (sorted, fixed mtimes), and its sha256 is the layer id.
- **Assigning content:** each Buildroot package, install and stage item gets a layer, from
  `packages-file-list.txt` (the same data per-package sizes use) and the stage-ownership
  record:

  ```toml
  [image.layers]
  order = ["base", "platform", "app"]          # lowest first
  default = "base"
  platform = { packages = ["linux", "rpi-firmware", "linux-ext-*"], paths = ["/lib/modules", "/lib/firmware"] }
  app = { installs = ["helios-*"], stage = ["helios-*"], services = ["helios-*"] }
  ```

- **Overlaps:** a path in two layers is an error, unless it's declared as an override; the
  upper layer wins at boot. Directories may repeat. Deletions across layers aren't supported,
  and no whiteouts are needed.
- **Outputs per layer:**
  - the tar;
  - an erofs made from it (`mkfs.erofs --tar`, multithreaded, byte-identical between runs);
  - optional dm-verity, using the existing transform;
  - `<out>/layers.json`, listing the order, sha256, size, verity root hash and offset, and the
    inputs fingerprint for each layer.
- **Cache:** each layer is keyed by its own inputs. An unchanged layer is reused as-is, with no
  tar, mkfs or verity work.
- **Full image unchanged:** the full image is still produced exactly as now, so nothing
  changes for a build until it opts in. Atlas's slot format (the initramfs mounting base,
  platform and app as overlayfs, verity per layer) reads `layers.json`. That format is
  Atlas's to design with us.

### 2.2 The app layer fast path

- **Built outside Buildroot:** a change only to app-layer items must not re-run Buildroot.
  Items in the app layer leave the Buildroot feed and its fingerprint. Gaia stages them
  straight from the artifact outputs into the app layer tree, then makes the tar, erofs and
  verity.
- **Gaia's part of a rebuild:** about 1–2 s for a few MB. The remaining time is the cargo
  build of the changed crate, for which HeliOS's thin-LTO `dev-image` profile helps.
- **Safety checks:** app ELFs' `NEEDED` must resolve against base and platform (1.4), and the
  app layer must not touch base-owned files.
- **Users and groups:** these stay in base, declared in the board's users table.
- **Shared libraries:** an app library must sit in an app-owned directory with an RPATH,
  since Buildroot ships no `ld.so.cache` to update.
- **Tradeoff:** a single image build (`image:build`) then produces base and platform only, so
  the first build after opting in rebuilds the Buildroot tree once.

## 3. OCI writer and the `sim-x86_64` target

### 3.1 OCI writer, with no daemon

`kind = "oci"` in the assembly takes layer tars (from 2.1) and writes either:

- an OCI image layout directory (`oci-layout`, `index.json`, `blobs/sha256/…`), which
  `podman load`, `skopeo copy oci:…` and `docker load` (Docker 25+) all accept; or
- a `docker load` tarball for older Docker.

Layers are compressed with gzip by default; zstd is opt-in. `diff_id` is the sha256 of the
uncompressed tar. The config sets `Entrypoint ["/sbin/init"]`, `StopSignal SIGRTMIN+3`, env,
labels (the source commits from provenance), and a fixed `created` time, so the same layers
give the same image digest. An unchanged layer is reused by hash, so an app-only change
rewrites one blob and the manifest. Expect 1–2 s.

### 3.2 `sim-x86_64`

- **What it is:** a HeliOS build variant (`target = "sim"`). Gaia's own changes are small.
  - **Base:** the same base recipes on an x86_64 Buildroot defconfig (glibc and systemd as on
    Raze, no kernel, no bootloader; `BR2_LINUX_KERNEL=n` is already supported).
  - **Sim platform layer:** lemnosd in simulation mode with Raze's board definitions (from
    Lemnos), and Styx on simulated sensors and replay (from Styx). It ships no kernel, modules
    or firmware, and 1.1 is skipped automatically for it.
  - **App:** the same HeliOS artifacts built for `x86_64-unknown-linux-gnu` against the x86
    Buildroot sysroot, so the glibc matches. The artifact provider already cross-builds
    against Buildroot's toolchain.
  - **Output:** the OCI image from 3.1, with systemd as init. `helios-sim up` runs it (that's
    HeliOS's part; `podman run --systemd=always`, or the Docker cgroup flags).
- **Size:** about 20–30 MB compressed is expected for base, so the <40 MB target is
  realistic; this gets measured after the first build. 1.3's `max_size` enforces the target.
- **Speed:** the first build costs one cold x86 Buildroot build, about 30–45 min of machine
  time, cached afterwards. After that, an app-only change is the cargo build plus about 2–4 s
  of Gaia work, covering the app layer, the OCI blob and the manifest.

## Needed from other repos

- **Atlas:**
  - the `[[image.hardware]]` list for Raze: each sensor's module name, compatible string,
    overlay, tuning files and firmware;
  - which of its recipes go in the platform layer;
  - the slot format reading `layers.json`: initramfs overlay mounts and verity per layer;
  - pushing an app layer to a board.
- **Styx:**
  - one sensor description file per sensor, at a fixed path for the hardware list (proposed
    `/usr/share/styx/sensors/<sensor>.toml`);
  - Styx userspace and simulated sensors building for x86_64 in Buildroot (no PiSP; the
    simple pipeline or vimc);
  - packaged for the sim platform layer.
- **Lemnos:** lemnosd simulation mode, packaged as installs or stage items for the sim
  platform layer, using the same board definitions as Raze.
- **HeliOS:**
  - the layer assignment in its recipes;
  - the `sim` target variant with x86 artifacts;
  - `helios-sim up` and the smoke test;
  - running `gaia image accept` after good board runs.
- **Kernel owners:** keep `CONFIG_MODVERSIONS=y`, which Raze has today and the CRC check
  relies on, and keep `Module.symvers` in the build output.

## Order and estimates

These are working time, including tests and the full gate, with each step pushed on its own.
The x86 cold build is extra machine time.

| # | Piece | Estimate | Usable on its own |
|---|---|---|---|
| 1 | 1.1 kernel module checks | ~3 h | yes: catches the incident class on every Raze build |
| 2 | 1.2 hardware list and overlay/compatible check | ~2 h | yes, once Atlas sends the list |
| 3 | 1.3 manifest, diff and size budgets, plus 1.4 units | ~2 h | yes |
| 4 | 2.1 layer split, per-layer tar/erofs/verity, `layers.json`, cache | ~4 h | yes: unblocks Atlas's slot design |
| 5 | 2.2 app layer outside Buildroot | ~3 h | yes: the seconds-level rebuild |
| 6 | 3.1 OCI writer | ~2 h | yes: any layer set becomes a container |
| 7 | 3.2 `sim-x86_64` support, plus a generic example | ~3 h | end to end once Lemnos, Styx and HeliOS recipes exist |

That's about 19 h of Gaia work, roughly 2.5–3 working days. Items 1–3 are useful
immediately and don't depend on anyone. Item 4 should land before Atlas finalizes the slot
format, so it can design against real `layers.json` output.
