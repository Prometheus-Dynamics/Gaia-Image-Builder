#!/usr/bin/env python3
"""What the gate has to test, from what changed since a code tree it passed.

The gate (scripts/gate.sh) asks this before its tests and tells it after a
pass. Nothing here skips a test whose result could have changed:

- The **code tree** is the commit's tree less what no build or test reads:
  `*.md`, `docs/`, `LICENSE-*` (the same set next.yml's push trigger leaves
  out). Its hash names a pass.
- `passed`: a clean worktree's code tree is recorded as passed, under
  `$GAIA_GATE_RECORDS` (default `~/.cache/gaia/gate-passed`). A dirty
  worktree records nothing: what passed is not a commit.
- `plan`: prints one line.
  - `none <commit>`: this code tree passed already, at that commit. The
    tests have nothing new to run.
  - `full <why>`: the whole workspace. That is the answer whenever the base
    is unknown (no passed ancestor in the last 200 commits), or anything
    outside a crate's directory changed (Cargo.lock, the toolchain, .cargo/,
    scripts/, examples/, recipes/, a workflow, a top-level file), since
    tests read those.
  - `lanes <base> <package>...`: the packages (`name:bin` for one with no
    library, so no doc tests) whose crate directories
    changed since the passed ancestor `base`, and every workspace package
    that depends on one of them (normal, build or dev dependency), to the
    end of the chain. Their tests, and only theirs, can see the change.
"""
import hashlib
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
RECORDS = pathlib.Path(
    os.environ.get("GAIA_GATE_RECORDS")
    or pathlib.Path.home() / ".cache" / "gaia" / "gate-passed"
)
WALK = 200


def git(*args, check=True):
    return subprocess.run(
        ["git", *args], cwd=ROOT, capture_output=True, text=True, check=check
    ).stdout


def not_code(path):
    name = path.rsplit("/", 1)[-1]
    return path.endswith(".md") or path.startswith("docs/") or name.startswith("LICENSE-")


def code_hash(commit):
    listing = [
        line
        for line in git("ls-tree", "-r", "--full-tree", commit).splitlines()
        if not not_code(line.split("\t", 1)[1])
    ]
    return hashlib.sha256("\n".join(listing).encode()).hexdigest()


def clean():
    return git("status", "--porcelain", "--untracked-files=normal").strip() == ""


def passed_at(digest):
    record = RECORDS / digest
    return record.read_text().strip() if record.is_file() else None


def changed_since(base):
    tracked = git("diff", "--name-only", base).splitlines()
    untracked = git("ls-files", "--others", "--exclude-standard").splitlines()
    return sorted({p for p in tracked + untracked if not not_code(p)})


def packages():
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout
    )
    members = set(meta["workspace_members"])
    by_id = {p["id"]: p for p in meta["packages"] if p["id"] in members}
    dirs = {
        str(pathlib.Path(p["manifest_path"]).parent.relative_to(ROOT)): p["name"]
        for p in by_id.values()
    }
    users = {p["name"]: set() for p in by_id.values()}
    libs = {
        p["name"]
        for p in by_id.values()
        if any("lib" in t["kind"] or "proc-macro" in t["kind"] for t in p["targets"])
    }
    for node in meta["resolve"]["nodes"]:
        if node["id"] not in by_id:
            continue
        for dep in node["deps"]:
            if dep["pkg"] in by_id:
                users[by_id[dep["pkg"]]["name"]].add(by_id[node["id"]]["name"])
    return dirs, users, libs


def plan():
    head = git("rev-parse", "HEAD").strip()
    if clean():
        at = passed_at(code_hash(head))
        if at:
            return f"none {at}"
    base = None
    for commit in git("rev-list", f"--max-count={WALK}", "HEAD").split():
        if passed_at(code_hash(commit)):
            base = commit
            break
    if base is None:
        return f"full no passed code tree in the last {WALK} commits"
    try:
        dirs, users, libs = packages()
    except subprocess.CalledProcessError as error:
        return f"full cargo metadata failed: {error.stderr.strip().splitlines()[-1:]}"
    hit = set()
    for path in changed_since(base):
        owner = next((d for d in dirs if path.startswith(d + "/")), None)
        if owner is None:
            return f"full {path} is outside every crate"
        hit.add(dirs[owner])
    if not hit:
        return f"none {base}"
    todo, closure = list(hit), set(hit)
    while todo:
        for user in users[todo.pop()]:
            if user not in closure:
                closure.add(user)
                todo.append(user)
    # A package with no library has no doc tests: `:bin` says so.
    named = [n if n in libs else f"{n}:bin" for n in sorted(closure)]
    return "lanes " + " ".join([base[:12], *named])


def main():
    if sys.argv[1:] == ["plan"]:
        print(plan())
    elif sys.argv[1:] == ["passed"]:
        if clean():
            head = git("rev-parse", "HEAD").strip()
            RECORDS.mkdir(parents=True, exist_ok=True)
            (RECORDS / code_hash(head)).write_text(head + "\n")
            print(f"gate: code tree of {head[:12]} recorded as passed")
        else:
            print("gate: worktree not clean; no pass recorded")
    else:
        sys.exit("usage: gate-lanes.py plan|passed")


main()
