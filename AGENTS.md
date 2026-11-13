# Working on the Gaia `next` branch

`next` is a clean-slate rebuild of Gaia as build-step nodes run by Daedalus. It started empty on
purpose. `dev` is the frozen reference, and nothing here changes it. Read
[`docs/design.md`](docs/design.md) first: it is the design everything here builds.

The work is a pre-planned list of PRs into `next`.

## If you are picking this up

1. Read [`docs/design.md`](docs/design.md).
2. Run `python3 scripts/pr-status.py` to see what's merged, open, ready and blocked.
3. Take a PR marked ready and read its brief at `docs/prs/<id>.md`. The brief is the whole
   assignment: what it needs, what it creates, which files on `dev` to read, what to build and what
   proves it.

[`TODO.md`](TODO.md) is the full list.

## Working a PR

- Base branch `next`, working branch `pr/<id>` (for example `pr/A1`), one PR into `next`.
- Read `dev` as a reference: `git fetch origin dev`, then `git show origin/dev:<path>`. Bring in only
  what the brief lists, and rework it to fit. Never check out another agent's checkout.
- Read Daedalus `next` the same way, at the commit locked in the root `Cargo.toml`. Its
  `docs/node-authoring.md`, `docs/host.md` and `docs/design.md` say how nodes, groups, failure
  policy, lanes and progress work. Use them; don't rebuild them.
- Stay inside the paths your PR creates. Don't edit the root `Cargo.toml`, `TODO.md`, or a changelog.
  Keeping to this is what lets many sessions run at once without conflicts. The one PR that moves
  the Daedalus pin edits every manifest, the root's included, and nothing else.
- `Cargo.lock` is committed: Gaia ships a program, and what is built at a commit is what its own
  gate tested. When your PR changes a `Cargo.toml`, run `cargo metadata --format-version 1 >
  /dev/null` and commit the lock with it; that updates only what the manifests changed. The gate
  stops on a lock that does not match. On a conflict in `Cargo.lock`, take `next`'s file and run
  the same command. Don't run `cargo update` in a feature PR: moving dependencies is its own PR.
- Add a crate by creating its directory under `crates/<folder>/`. Name it `gaia-<directory>`.
  Inherit shared dependencies with `workspace = true`, and declare any others in your own crate.
- If your PR needs a shared interface changed (`crates/core/*`), stop and say so in the PR
  description. Core interfaces change only through A-series or B-series PRs.
- A need named `X<n>` is an outside block (see `docs/design.md`). Don't work around it and don't
  open PRs in other repositories for it.
- A PR merges when `scripts/gate.sh` passes. Paste the proof named in the brief into the PR
  description.

## Rules

- Gaia is general. No board, product or camera name appears in code. They live in descriptions
  and in `examples/`.
- Every build step is a Daedalus node. Gaia has no scheduler, no reuse engine, no retry loop and no
  process supervisor of its own. If Daedalus lacks something, say so in the PR; don't build it here.
- A node reads only its inputs and writes only into its own output directory in the content store.
  Tools run pinned: a container image or toolchain named in the node's inputs.
- Port types are structured `TypeExpr`s with pinned keys. Nothing at a port is opaque.
- No shell hooks as a build mechanism. A command step is a node with declared inputs and outputs.
- Outputs are reproducible: fixed timestamps, sorted archives, no host paths inside outputs.
- A node that links a format or encoder library puts that library's version in its step key: pinned `=x.y.z`, or the runtime-reported version; a test checks the key equals it.
- Failure rule: never give up, backoff capped at 5 to 10 s, no default auto-off, an easy manual off,
  auto-off only as a configured policy. Retries are a step policy, never a loop inside a node.
- Lean is the default. A trade-off is a plain knob, not a mode or a profile.
- Boards and devices you can't reach are marked `untested on board`. Don't hold work back for one.
- Never claim something works without having run it. Report failures with their output.
- Never push tags. Never force-push `next` or `dev`. Don't change `dev`.
- Don't estimate how long anything will take. Describe size by scope and risk.
- Builds, scratch and content stores go on NVMe or SSD, never the Github HDD.

## Board runs

Briefs marked `Runs: board` need real hardware. They run in a local session on the user's machine
after the PR merges, and report what booted and what was compared. Timings count only from the
board itself.

Writing an image to the disk a board is running from:

- Copy the image to the board's tmpfs and check its digest there before anything is overwritten.
- Run the write from tmpfs only: a copy of the shell and its libraries, not the root's.
- Remount the old root read-only, write, `sync`, then restart at once (`reboot -f` or sysrq `b`).
- Never drop caches or read the disk back before the restart. Once the root is overwritten, a
  program that pages its code back in reads the new bytes and crashes; when that program is
  init, the kernel panics and the board needs a power cycle.
- Check the result from the new system after it boots.
