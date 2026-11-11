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
  Today Raze's sits in Atlas `devices/raze/` and is out of date for the Raze OS
  as it is meant to be now; it is read for facts, never ported (G1).
- **Component file.** One piece of software and what it installs: where its
  source is, which builder makes it, the files it puts in the image, the
  service it enables, the users and groups it needs. A library ships nothing
  Gaia-specific (no Gaia files in Lemnos or Orion); its component file lives
  with whoever owns the build config. For the Raze OS that is Atlas: board,
  components and product together.
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

## Reuse: Gaia is stateless

Gaia keeps no state between runs and needs nothing from Daedalus for reuse.
Give it inputs; it looks at what is already in the content store and makes only
what is missing.

- **Every step has a key**: a hash of its node kind, node version, config and
  its inputs' digests. Source steps take their digest from content (a commit, a
  tree digest, a locked download digest), so keys chain from the sources down.
- **Look up or make.** The step kit (A1) asks the store for the key. When the
  entry exists and every artifact it names is present with its digest, the step
  returns those references without running its body. Otherwise it runs, and
  the store records the entry only after the outputs are committed.
- **The store is the whole cache** (B1): content by digest, plus step key ->
  output references. Delete any of it and the next run remakes exactly that.
  There are no run records, no "last build" state, no Gaia reuse engine.
- Daedalus's own in-memory reuse is not used. Daedalus plans, runs, retries,
  stops and reports; checking what already exists is the nodes' job.

Editing one file in a source changes that tree's digest, so exactly the steps
downstream of it make anything.

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
  forced failure that stops the build, a flaky step that retries, and a second
  run that makes nothing.
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
| X2 | Atlas owns the Raze build config and adopts G1's files in place of `devices/raze/` and HeliOS `gaia/configs/` | nothing in Gaia; only the hand-over after G1 |
| X3 | Orion I4 ships systemd units only, no Gaia files (Lemnos `next` already has none) | G1 only |
| X4 | Orion recovery build (G6, done on Orion `next`) is what the Raze recovery image packs | G1 only |
| X5 | Daedalus `next` stays buildable at the locked commit | everything; locked by commit |
| X6 | Daedalus: per-step Recovery (Retry{max}, SkipTick) settable from a loaded graph; today LoadedHost::load sets Recovery::Fail for every step | retries in builds; A2's retry proof |
