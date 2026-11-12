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
  `Cargo.lock` is ignored. Keeping to this is what lets many sessions run at once without conflicts.
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
