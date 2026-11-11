# Gaia `next`: the plan

Status: plan only. Nothing on this branch is rebuilt yet; the tree is still
`dev` (5ac5894) plus this file. The clean slate, the skeleton and the PR briefs
come next, in that order.

## What Gaia is

Gaia builds whatever a device needs in order to run: an OS image, a slot, a
recovery image, and later MCU firmware. It is not a Raze or HeliOS tool. No
board name, product name or camera name appears in Gaia's code.

## What is wrong today

- 219k lines (123k code, 96k tests), of which the engine that schedules,
  plans, reuses and reports is the largest part, and all of it is Gaia's own.
- A board build is spread thin. Raze is 41 TOML files and 3,678 lines across
  four repos, with the fan in 6 files and the camera sensor named in 20.
- Facts are copied by hand: four `rev` pins under about 60 lines of comments.
- Shell hooks carry real build logic that nothing checks.
- Two config versions parse (v2, v3). Only v3 is used.

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

Four parts, and only the first two are Gaia code:

1. **Nodes.** Each build step is a Daedalus node with typed inputs and
   outputs: fetch a source, build with a language toolchain, run an OS
   provider, make a filesystem, lay out a disk, add verity, sign, check.
2. **The command.** Reads descriptions, builds a `GraphDocument`, hands it to
   Daedalus, prints the result.
3. **Recipes.** Named, parameterized node groups, stored as data outside
   Gaia's code. "Slots as files on a data partition" is a recipe, not a step
   type. A new layout is a new recipe, not a Gaia change.
4. **Descriptions.** A board file (what the hardware is and needs), component
   files (what each piece of software installs), a product file (which
   components on which board), and one lock.

## What Daedalus supplies (read in `next`, c158e63a)

| Need | Daedalus has |
|---|---|
| Stop on first failure | `FailurePolicy::with_fail_fast` |
| Retry a flaky step N times, then stop | per-step `Recovery::Retry { max }` |
| Skip, auto-off, manual off | `Recovery::SkipTick`, `auto_off_after`, `switch_off` |
| Steps in parallel | lanes on a worker pool, priority ordered |
| A long step that doesn't hold a lane | async steps returning pending |
| Skip unchanged work | per-node reuse keyed on input versions; upstream change invalidates downstream |
| Progress, cancel, why it failed | progress cells, cancel word, fault record, 64 KB stderr tail from workers |
| Steps in other processes | process backend, supervised workers |
| Groups and control flow | node groups, if / for-each / until regions |
| Validity before running | the planner |

One difference found: reuse state is in memory and is cleared on reload, so it
does not carry from one `gaia build` to the next. Until Daedalus stores it,
each Gaia node keeps its own result record on disk (inputs digest -> outputs)
and reports "unchanged" itself. That needs nothing from Daedalus.

## What is not ported

Gaia's scheduler, planner, reuse engine, process supervision and plugin
loading (about 30k lines with tests), config v2, shell hooks as a build
mechanism, hand-written pins, and the TUI's own progress model.

## What is ported, as nodes

| Group | Nodes | From `dev` |
|---|---|---|
| Sources | git, path, archive, download, OCI | `crates/providers/source` |
| Builders | rust, java, node, python, go, command | `crates/providers/artifact` |
| OS providers | Buildroot, imported image, none | `crates/providers/image` |
| Assembly | filesystem (ext4, EROFS, vfat, UBI), partition table, disk, initramfs, compress | `crates/core/formats`, `exec` |
| Integrity | verity, sign, verify | `exec`, `formats` |
| Checks | image structure, module and firmware presence, size budget | `validate`, `exec` |
| Reports | sizes, contents, lock | `report` |

"None" as an OS provider and open output kinds keep MCU firmware possible
without building it now: a product with no OS, a target that is not Linux, an
output that is a `.bin` or `.uf2`.

## Order of work

- **A. Proof.** The first PRs are the risk: three real steps as nodes (fetch,
  build, pack), run by Daedalus in parallel, with one forced failure that
  stops the build, one flaky step that retries, and a second run that skips
  everything. If this does not hold, the plan changes here, cheaply.
- **B. Model.** Types for artifacts, digests, the lock, descriptions.
- **C. Sources and builders** as nodes.
- **D. OS providers**, Buildroot first.
- **E. Assembly, integrity, checks.**
- **F. Recipes and descriptions**, and the command that turns them into a graph.
- **G. Boards.** Raze first, then the dev boards in `examples/boards`, each
  compared file by file against the image `dev` builds.
- **H. Retire.** `dev`'s engine is deleted when G matches.

## Blocked on other repos

Marked, not requested. No PR is made for or given to another repo.

| Item | Waits on | Stops Gaia? |
|---|---|---|
| Reuse across runs inside Daedalus | Daedalus storing reuse state | No. Nodes keep their own record |
| Gaia depends on Daedalus `next` | Daedalus `next` staying buildable | Yes for A onward; locked by commit |
| Lemnos in an image (module, drivers) | Lemnos has no packaging PR | Only the Raze board in G |
| Orion in an image, recovery build | Orion I4, G6 | Only the Raze board in G |
| Board packages (today Atlas `devices/`) | where they live (open call) | Only G |

## Open calls

1. Where board packages live. Lean: their own small repo, since Lemnos's
   design says board data lives outside Lemnos and Atlas should not own it.
2. The product build file stays in the product's repo (HeliOS for HeliOS).
