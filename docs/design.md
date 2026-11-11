# Gaia `next`: the design

Gaia builds what a device needs in order to run: an OS image, a slot, a
recovery image, a package for another system's installer, and later MCU
firmware. It is general. No board name, product name or camera name appears in
Gaia's code; those live in descriptions, which are data.

## Why a rebuild

`dev` is 219k lines (123k code, 96k tests). The largest part is an engine
that schedules, plans, reuses and reports, and all of it is Gaia's own. A
board build is spread thin (Raze is 41 TOML files and 3,678 lines across four
repos, the fan in 6 files, the camera sensor named in 20). Facts are copied by
hand (four `rev` pins under about 60 lines of comments). Shell hooks carry
build logic nothing checks. Two config versions parse; one is used.

## The shape

```
 descriptions (data)            Gaia                       Daedalus
 ------------------    ---------------------      -------------------------
 board     file   -->   reads descriptions  -->    plans the graph, proves it
 component files        builds one graph           valid before anything runs
 product   file         from nodes + recipes       runs steps on parallel lanes
 one lock file                                      retries / stops per step
                        nodes: the build steps     reuses unchanged steps
                        recipes: node groups       progress, cancel, fault record
```

Four parts. Only the first two are Gaia code.

1. **Nodes** (`crates/nodes/*`). Each build step is a Daedalus node with typed
   inputs and outputs: fetch a source, build with a toolchain, run an OS
   provider, make a filesystem, lay out a disk, add verity, sign, check.
2. **The command** (`crates/bins/gaia`). Reads descriptions, builds a
   `GraphDocument`, hands it to Daedalus, prints the result.
3. **Recipes.** Named, parameterized node groups, stored as data. "Slots as
   EROFS files on a data partition, with verity" is a recipe, not a step type.
   A new layout is a new recipe, not a Gaia change.
4. **Descriptions.** A board file, component files, a product file, one lock.

## Descriptions, plainly

- **Board file.** What the hardware is and what it needs to boot: SoC and CPU,
  bootloader and its blobs (with digests), kernel source and config fragments,
  device tree and overlays, firmware files, partition needs, how it is flashed.
  Today this sits in Atlas `devices/<board>/` (manifest, `gaia/*.toml`,
  `fetch.lock`, overlays, udev rules) and in Gaia `examples/boards/<board>/`.
- **Component file.** One piece of software and what it installs: where its
  source is, which builder makes it, the files it puts in the image, the
  service it enables, the users and groups it needs. A library ships nothing
  Gaia-specific; its component file lives with whoever puts it in an image.
- **Product file.** Which components go on which boards, which recipe lays out
  the image, and the size budget. It lives in the product's repo.
- **The lock.** Every source resolved to a commit or digest, written by
  `gaia lock`, never by hand. One per product.

## What Daedalus supplies

Read in Daedalus `next`. Gaia does not rebuild any of this.

| Need | Daedalus |
|---|---|
| Stop on first failure | `FailurePolicy::with_fail_fast` |
| Retry a flaky step N times, then stop | per-step `Recovery::Retry { max }` |
| Skip, auto-off, manual off | `Recovery::SkipTick`, `auto_off_after`, `switch_off` |
| Steps in parallel | lanes on a worker pool, priority ordered |
| A long step that doesn't hold a lane | async steps returning pending |
| Progress, cancel, why it failed | progress cells, cancel word, fault record, stderr tail |
| Steps in other processes | process backend, supervised workers |
| Groups and control flow | node groups, if / for-each / until regions |
| Validity before running | the planner |
| Skip unchanged work | reuse, see below |

## Reuse across runs

Daedalus reuse today keys a step on per-slot version counters, keeps outputs
as in-memory handles, and clears on reload. That fits a graph that runs every
frame and does not fit a build, where the next run is a new process.

Gaia does not build its own reuse. The change belongs in Daedalus, where it
serves both kinds of host (outside block X1):

- **A key per value, not a counter.** A reusable step's key is a hash of its
  node kind, node version, config and its inputs' keys. Host inputs take their
  key from the host. Keys chain, so they are the same in the next process.
- **A store behind reuse.** A small trait, `get(key)` and `put(key, outputs)`.
  The default store is in memory and holds handles: today's behaviour, and
  reload no longer clears it, because an unchanged node has an unchanged key.
  A file store, for outputs that have a value encoding, is optional.
- **The host decides a hit is still good.** The host's store can refuse a hit,
  for example when the files an output names are gone.

Gaia's side: a step's outputs are small references (`Artifact`: kind, digest,
path in Gaia's content store), so the file store holds a few hundred bytes per
step. Gaia's store answers a hit only when the referenced files exist with
their digests. Source nodes key their output on content (a commit, a file
digest), so editing one file reruns exactly the steps downstream of it.

HeliOS's side, for comparison: keys from frame sequence numbers cost one small
hash per opted-in step, never a hash of pixels; a graph edit reruns only the
changed nodes instead of everything; expensive start-up results (lookup
tables, compiled pipelines) can survive a restart through the file store.

Until X1 lands, every Gaia run runs every step. That is correct, only slower.

## Types at the boundary

Gaia's port types are structured `TypeExpr`s with pinned keys (`gaia:artifact`,
`gaia:tree`, `gaia:image`, `gaia:partition`, `gaia:lock-entry`, ...). Nothing at
a port is opaque. Graphs are versioned `GraphDocument`s with `requires` filled
from the registry.

## Rules for every node

- A node reads only its inputs and writes only into its own output directory
  in the content store. No global state, no ambient environment.
- A node that runs a tool runs it in a pinned container image or with a pinned
  toolchain, named in its inputs.
- Failure rule: retries are a policy on the step, never a loop inside a node.
  No step gives up silently; no step switches itself off.
- Outputs are reproducible from inputs: fixed timestamps, sorted archives.
- Lean is the default. A trade-off is a plain knob, not a mode.

## What is not ported

Gaia's scheduler, planner, reuse engine, process supervision and plugin
loading (about 30k lines with tests), config v2, shell hooks as a build
mechanism, hand-written pins, the TUI's own progress model.

## What is ported, as nodes

| Group | Nodes | Read on `dev` |
|---|---|---|
| Sources | git, path, archive, download, OCI | `crates/providers/source` |
| Builders | rust, command, java, node, python, go | `crates/providers/artifact` |
| OS providers | Buildroot, imported image, none | `crates/providers/image` |
| Assembly | file tree, ext4, EROFS, vfat, UBI, partition table, disk, initramfs, compress, strip | `crates/core/exec`, `crates/core/formats` |
| Integrity | verity, sign, verify | `crates/core/formats`, `crates/core/exec` |
| Checks | image structure, modules and firmware, kernel config, device tree, size budget | `crates/core/exec/src/operations/image_check`, `crates/core/validate` |
| Reports | sizes, contents, provenance | `crates/core/report` |

"None" as an OS provider and open output kinds keep MCU firmware possible
without building it now: no OS, a target that is not Linux, an output that is
a `.bin` or `.uf2`.

## Order of work

- **A. Proof.** Three real steps as nodes, run by Daedalus in parallel, a
  forced failure that stops the build, a flaky step that retries. The second
  run skipping everything waits on X1.
- **B. Model.** Content store, port types, the lock, descriptions.
- **C. Sources and builders.**
- **D. OS providers**, Buildroot first.
- **E. Assembly, integrity, checks, reports.**
- **F. Recipes, the graph builder, the command.**
- **G. Boards.** Each compared file by file with the image `dev` builds.
- **H. Finish.** CI, docs, the lock committed. Replacing `dev` is the user's call.

## Outside blocks

Marked here, not requested from those repos.

| Id | Item | Stops |
|---|---|---|
| X1 | Daedalus keyed reuse and a reuse store (above) | A3 only; every run is full until then |
| X2 | Where board files live (Atlas `devices/` today) | G1 only; dev-board files go in Gaia `examples/boards` |
| X3 | Orion I4 plans Gaia fragments in `dev`'s format; it should ship systemd units only, and the component file lives with the product | G1 only |
| X4 | Orion recovery build (G6, done on Orion `next`) is what the Raze recovery image packs | G1 only |
| X5 | Daedalus `next` stays buildable at the locked commit | everything; locked by commit |
