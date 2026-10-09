#!/usr/bin/env bash
# Interrupts a real Buildroot build at each phase in turn (download,
# configure, build, install, finalize), resuming after each, and checks the
# final target tree matches an uninterrupted build of the same config.
#
# usage: testing/interrupt/interrupt-resume.sh <build.toml> <buildroot-output-dir> <reference-manifest>
#
# <reference-manifest> is `manifest <output>/target` of an uninterrupted build
# (see below); create it with:  testing/interrupt/interrupt-resume.sh --manifest <output-dir> > ref.txt
# Start from a cleaned build (`gaia clean <build.toml>`) and the package cache
# off (`--set policy.providers.buildroot.package_cache.enabled=false`) so the
# phases are really reached.
set -euo pipefail

gaia="${GAIA:-gaia}"

# Every file under a target tree: path, type, mode, size and content digest.
manifest() {
    (cd "$1" && find . -mindepth 1 -printf '%p %y %m %s %l\n' | sort |
        while read -r path type mode size link; do
            if [[ "$type" == f ]]; then
                printf '%s f %s %s %s\n' "$path" "$mode" "$size" "$(sha256sum <"$path" | cut -c1-16)"
            else
                printf '%s %s %s %s\n' "$path" "$type" "$mode" "$link"
            fi
        done)
}

if [[ "${1:-}" == --manifest ]]; then
    manifest "$2/target"
    exit 0
fi

build="$1"
output="$2"
reference="$3"
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# Is some package of the tree in `phase`?
in_phase() {
    local phase="$1" dir
    if [[ "$phase" == finalize ]]; then
        grep -q "Finalizing target directory" "$log"
        return
    fi
    for dir in "$output"/build/*/; do
        [[ -d "$dir" ]] || continue
        has() { [[ -e "$dir/.stamp_$1" ]]; }
        case "$phase" in
        download) ! has downloaded && return 0 ;;
        configure) has patched && ! has configured && return 0 ;;
        build) has configured && ! has built && return 0 ;;
        install) has built && ! has installed && return 0 ;;
        esac
    done
    return 1
}

for phase in download configure build install finalize; do
    echo "== run until $phase, then cancel"
    : >"$log"
    "$gaia" run "$build" --set policy.providers.buildroot.package_cache.enabled=false >"$log" 2>&1 &
    run=$!
    until in_phase "$phase"; do
        if ! kill -0 "$run" 2>/dev/null; then
            echo "build ended before reaching $phase"
            wait "$run" || true
            break
        fi
        sleep 0.5
    done
    if kill -0 "$run" 2>/dev/null; then
        # Let the phase do some work, then cancel as a user would.
        sleep 3
        "$gaia" cancel "$build"
        wait "$run" || true
        grep -E "cancel|interrupted" "$log" | tail -3 || true
    fi
done

echo "== resume to completion"
"$gaia" run "$build" --set policy.providers.buildroot.package_cache.enabled=false | tee "$log"
grep -E "resuming" "$log" || true

if diff <(manifest "$output/target") "$reference" >/dev/null; then
    echo "PASS: the resumed target tree matches the uninterrupted build"
else
    echo "FAIL: target trees differ:"
    diff <(manifest "$output/target") "$reference" | head -40
    exit 1
fi
