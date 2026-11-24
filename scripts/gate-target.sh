#!/usr/bin/env bash
# One cargo target directory belongs to one worktree. scripts/gate.sh runs
# this first.
#
# cargo names a workspace crate's build the same in every checkout of the
# repository, and decides whether it is fresh by file times. In a target
# directory two worktrees share (CARGO_TARGET_DIR), a crate one of them built
# from newer files is taken as fresh for the other's older, different
# sources: the gate then passes or fails on code that is not in the tree.
#
# So the target directory records the worktree that uses it, in
# <target>/.gaia-worktree, and a gate run from another worktree is refused
# (exit 1) before cargo runs. A directory with no record is given this
# worktree's; when it already holds something, that is said, since whatever
# built it is not known. With CARGO_TARGET_DIR unset the target is the
# worktree's own `target`, which no other worktree has, and nothing is
# checked.
#
# A new target directory on btrfs is made a subvolume: a subvolume has a
# file system tree of its own, so the compilers' metadata writes do not
# queue on one tree's locks with everything else on the disk (on a shared
# disk, a core-crate rebuild took 62.7 s in a plain directory and 18.9 s in
# a subvolume). Without the btrfs tool, or off btrfs, it is a plain directory.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
target=${CARGO_TARGET_DIR:-}
[[ -n $target ]] || exit 0
[[ $target == /* ]] || target=$here/$target
record=$target/.gaia-worktree
if [[ -f $record ]]; then
	owner=$(< "$record")
	if [[ $owner != "$here" ]]; then
		echo "the cargo target directory $target belongs to another worktree:" >&2
		echo "  its worktree: $owner" >&2
		echo "  this one:     $here" >&2
		echo "cargo would take that worktree's builds as fresh for this one's sources (it goes by file times)." >&2
		echo "Give this worktree a target directory of its own (CARGO_TARGET_DIR), or, if $owner is gone, remove $target." >&2
		exit 1
	fi
	exit 0
fi
if [[ -d $target && -n $(ls -A "$target" 2> /dev/null) ]]; then
	echo "the cargo target directory $target has no worktree on record and is not empty: recording $here." >&2
	echo "If another worktree built what is in it, remove it and run again." >&2
fi
if [[ ! -e $target ]]; then
	parent=$(dirname "$target")
	mkdir -p "$parent"
	if [[ $(stat -f -c %T "$parent") == btrfs ]] && command -v btrfs > /dev/null; then
		btrfs subvolume create "$target" > /dev/null 2>&1 || true
	fi
fi
mkdir -p "$target"
printf '%s\n' "$here" > "$record"
