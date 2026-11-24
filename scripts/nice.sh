# Sourced by the scripts that do heavy work (scripts/gate.sh, check.sh,
# qemu-virt-kernel.sh): one niceness for all of it, inherited by every
# process they start.
#
# Every repository on the shared build host runs its gates, builds, tests
# and emulators at nice 10. With mixed levels a run at 19 gets a fraction of
# the CPU a neighbour's run at 10 does (weight 15 against 110): its times
# measure the neighbours, not the run. GAIA_NICE sets another level.
# Raising niceness needs no privilege; lowering it does, so a shell that is
# already nicer than the level stays where it is, and says so.
GAIA_NICE=${GAIA_NICE:-10}
gaia_now=$(ps -o ni= -p $$ | tr -d ' ')
if [[ $gaia_now != "$GAIA_NICE" ]]; then
  if ! renice --priority "$GAIA_NICE" -p $$ > /dev/null 2>&1; then
    echo "nice: running at $gaia_now, not $GAIA_NICE (lowering niceness needs privilege)" >&2
  fi
fi
unset gaia_now
