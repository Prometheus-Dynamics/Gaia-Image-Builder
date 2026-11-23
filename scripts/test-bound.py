#!/usr/bin/env python3
"""Runs one test process under a bound on the time it could use.

    test-bound.py TEST-BINARY [ARGS...]     as cargo's target runner (scripts/gate.sh sets it)
    test-bound.py --self-test

A test that hangs would hold the gate forever, and a wall-clock limit fails tests that are only
starved: on a machine running more than it has cores for, a test waits for a CPU most of the time
it is alive. So the clock here is the time the test could use: wall time since it started, less
the time its threads spent runnable but waiting for a CPU. The kernel keeps that for every thread
(the second number of /proc/<pid>/task/<tid>/schedstat, in nanoseconds). The test runs as the
leader of a process group of its own; once a second every thread in the group is read, the growth
is summed and divided by the threads read (threads that all waited at once lost that time once),
and a thread seen for the first time counts from zero. A test that sleeps or spins past its bound
still fails at it. One that never gets a CPU is stopped at OUTER times the bound of wall time.
Where the kernel keeps no such record, the bound is wall time.

At the bound the whole group gets TERM, then KILL after a grace, and the runner exits 124 naming
the test, its bound, the wall time it ran and the time taken off. Otherwise it exits as the test
did. TERM, INT and HUP to the runner stop the group too.

GAIA_TEST_BOUND is the bound in seconds (default 600; the gate's slowest test takes about 110 s
on a loaded machine). GAIA_TEST_BOUND_SCALE multiplies it, for a slower machine.

The idea and the arithmetic are Daedalus's (scripts/bounded.py, Group.wait_fair), so the two
gates judge a starved test the same way.
"""
import os
import select
import signal
import subprocess
import sys
import time

TIMED_OUT = 124
GRACE = 5.0
OUTER = 10
POLL = 1.0
DEFAULT_BOUND = 600.0


def group_members(pgid):
    """The live processes of group `pgid`."""
    found = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/stat") as file:
                fields = file.read().rsplit(")", 1)[1].split()
        except (OSError, IndexError):
            continue
        if int(fields[2]) == pgid and fields[0] not in "ZX":
            found.append(int(entry))
    return found


def waited(pgid):
    """Seconds each thread of group `pgid` has waited for a CPU, by thread id. Empty where the
    kernel keeps no such record."""
    found = {}
    for pid in group_members(pgid):
        try:
            for task in os.listdir(f"/proc/{pid}/task"):
                with open(f"/proc/{pid}/task/{task}/schedstat") as file:
                    found[int(task)] = int(file.read().split()[1]) / 1e9
        except (OSError, IndexError, ValueError):
            continue
    return found


class Group:
    """A command started as the leader of a process group of its own."""

    def __init__(self, argv, **popen):
        self.proc = subprocess.Popen(argv, process_group=0, **popen)
        self.pgid = self.proc.pid
        self.pidfd = os.pidfd_open(self.proc.pid)

    def ended(self, timeout):
        ready, _, _ = select.select([self.pidfd], [], [], max(0.0, timeout))
        return bool(ready)

    def wait_fair(self, bound, outer=OUTER, poll=POLL):
        """(ended, wall, lost): whether the leader ended before it had `bound` seconds it could
        use, or `outer` times that of wall time; the wall time so far; the time taken off."""
        start = time.monotonic()
        seen, lost = {}, 0.0
        while True:
            wall = time.monotonic() - start
            left = min(bound - (wall - lost), outer * bound - wall)
            if left <= 0:
                return False, wall, lost
            if self.ended(min(poll, left)):
                return True, time.monotonic() - start, lost
            now = waited(self.pgid)
            more = sum(max(0.0, wait - seen.get(task, 0.0)) for task, wait in now.items())
            seen.update(now)
            lost += more / max(1, len(now))

    def stop(self):
        """TERM to the whole group, KILL to what is left after the grace; then the leader is
        reaped. Its group id stays its pid until then, so nothing else is signalled."""
        if group_members(self.pgid):
            try:
                os.killpg(self.pgid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + GRACE
            while group_members(self.pgid) and time.monotonic() < deadline:
                time.sleep(0.05)
            if group_members(self.pgid):
                try:
                    os.killpg(self.pgid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        code = self.proc.wait()
        os.close(self.pidfd)
        return code


class Stopped(Exception):
    def __init__(self, signum):
        self.signum = signum


def raise_stopped(signum, _frame):
    raise Stopped(signum)


def run(argv, bound, out=sys.stderr, **popen):
    """Runs `argv` within `bound` usable seconds. Returns its exit code, or TIMED_OUT."""
    group = Group(argv, **popen)
    ended, wall, lost = False, 0.0, 0.0
    try:
        ended, wall, lost = group.wait_fair(bound)
    finally:
        code = group.stop()
    if not ended:
        name = " ".join(argv)
        print(f"{name}: still running at its bound of {bound:.0f} s of usable time "
              f"(ran {wall:.0f} s, {lost:.0f} s of it waiting for a CPU): stopped it", file=out)
        return TIMED_OUT
    return 128 - code if code < 0 else code


def self_test():
    """Three cases on one CPU that outside spinners share, judged by usable time, never wall."""
    bound = 3.0
    cpu = sorted(os.sched_getaffinity(0))[-1]
    pin = lambda: os.sched_setaffinity(0, {cpu})
    spin = "import time\nend = time.process_time() + {cpu}\nwhile time.process_time() < end: pass\n"
    failed = []

    def check(what, ok, detail):
        print(("ok   " if ok else "FAIL ") + what + ": " + detail)
        if not ok:
            failed.append(what)

    def case(code, others):
        spinners = [subprocess.Popen([sys.executable, "-c", "while True: pass"], preexec_fn=pin)
                    for _ in range(others)]
        try:
            group = Group([sys.executable, "-c", code], preexec_fn=pin)
            try:
                return group.wait_fair(bound, poll=0.2)
            finally:
                group.stop()
        finally:
            for spinner in spinners:
                spinner.kill()
                spinner.wait()

    if not waited(os.getpgid(0)):
        print("skipped: the kernel keeps no schedstat here, so the bound is wall time")
        return 0
    # 1. Sleeping is not waiting for a CPU: stopped at the bound, nothing taken off.
    ended, wall, lost = case("import time; time.sleep(60)", 3)
    check("a sleeper is stopped at its bound", not ended and wall - lost >= bound and lost < 1.0,
          f"ended={ended} usable={wall - lost:.1f} s lost={lost:.1f} s")
    # 2. Needs less CPU than the bound, on a CPU three spinners share: starved, not slow.
    ended, wall, lost = case(spin.format(cpu=bound * 0.5), 3)
    check("a starved test that needs less than its bound passes", ended and lost > 0.5,
          f"ended={ended} usable={wall - lost:.1f} s lost={lost:.1f} s (wall is not judged)")
    # 3. Needs more CPU than the bound: stopped when its usable time reaches it, starved or not.
    ended, wall, lost = case(spin.format(cpu=bound * 3), 3)
    check("a test that needs more than its bound is stopped at it",
          not ended and bound <= wall - lost < bound + 1.5 and lost > 0.5,
          f"ended={ended} usable={wall - lost:.1f} s lost={lost:.1f} s")
    return 1 if failed else 0


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    if not argv or argv[0].startswith("-"):
        print(__doc__.split("\n\n")[0] + "\n\n" + __doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    bound = float(os.environ.get("GAIA_TEST_BOUND", DEFAULT_BOUND))
    bound *= float(os.environ.get("GAIA_TEST_BOUND_SCALE", "1"))
    for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(signum, raise_stopped)
    try:
        return run(argv, bound)
    except Stopped as stopped:
        return 128 + stopped.signum


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
