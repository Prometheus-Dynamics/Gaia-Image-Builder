#!/usr/bin/env python3
"""The board profile's sysfs against a board's (scripts/qemu-rpi-slots.sh
--profile board; see docs/board-qemu.md).

  scripts/qemu-board-sysfs.py collect STATE OUT [SECONDS]
  scripts/qemu-board-sysfs.py diff GUEST BOARD...

collect: logs in as root on a running guest's serial console
(STATE/console.sock; an image whose console asks for a password is refused),
waiting for the socket and pressing Enter until the console shows a login
or shell prompt (for up to SECONDS in all, default 600: the guest may still
be booting), and writes to OUT what the guest has under /sys/class/block, udc and
watchdog: each entry's link, the watchdog's identity, timeouts and state,
each block device's size, read-only flags, partition and device number, the
eMMC card's attribute names and the UDC's state. The listing is in the form
Atlas lists a board's, so the two compare as they are.

diff: compares the guest's listing with a board's, given in one or more
listings of the same board (merged; two that disagree on a fact are
refused). Each is either that form (`ls -l /sys/class/X`
under a "/sys/class/X:" line, then "PATH: VALUE" lines; Atlas's
"/sys/block/X force_ro: V" and "X size: N" are read too) or Atlas's block
form ("== X -> /sys/devices/..." then "  key=value" lines, and the card's
attribute names after its "/sys/devices/.../of_node" line). Every fact
either side has is printed once: "same", "differs" (board, then guest),
"board only" or "guest only"; then the counts. A fact one side does not
list is not compared. The exit status is 0 whatever the differences: the
profile's known ones are in docs/board-qemu.md.
"""
import re
import socket
import sys
import time

# The guest's half of collect: busybox sh (no readlink or basename needed).
COLLECT = r"""
for c in block udc watchdog; do echo "/sys/class/$c:"; ls -l /sys/class/$c; echo; done
for w in /sys/class/watchdog/*; do
 for a in identity timeout min_timeout max_timeout state status bootstatus nowayout; do
  [ -r $w/$a ] && echo "$w/$a: $(cat $w/$a)"
 done
done
for b in /sys/class/block/*; do
 for a in size ro removable hidden force_ro ro_lock_until_next_power_on ext_range partition dev; do
  [ -r $b/$a ] && echo "$b/$a: $(cat $b/$a)"
 done
done
for d in /sys/class/block/*/device; do
 [ -e $d/cid ] && echo "card-attributes ${d%/device}: $(ls $d | tr '\n' ' ')"
done
for u in /sys/class/udc/*; do
 for a in state current_speed maximum_speed is_selfpowered; do
  [ -r $u/$a ] && echo "$u/$a: $(cat $u/$a)"
 done
done
"""


# Prompts, as the console's last output: a getty's, a password's and a root
# shell's. Only at the end, so a boot log line with "login:" or "# " in it
# (the kernel's version line has "# SMP") is not taken for one.
LOGIN = re.compile(rb"login: ?$")
PASSWORD = re.compile(rb"[Pp]assword: ?$")
SHELL = re.compile(rb"# ?$")


def collect(state, out, wait):
    end = time.time() + wait
    # The socket is there once QEMU has started.
    while True:
        s = socket.socket(socket.AF_UNIX)
        try:
            s.connect(f"{state}/console.sock")
            break
        except OSError as e:
            s.close()
            if time.time() > end:
                sys.exit(f"no console at {state}/console.sock in {wait} s: {e}")
            time.sleep(1)
    s.settimeout(0.5)
    buf = bytearray()

    def last():
        """The console's last output, colours removed."""
        return re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", bytes(buf[-400:]))

    def read(seconds, *until):
        """Reads for up to SECONDS, or until one of UNTIL is in the buffer
        (a bytes) or matches its end, colours removed (a compiled pattern)."""
        end = time.time() + seconds
        while time.time() < end:
            try:
                data = s.recv(65536)
                if data:
                    buf.extend(data)
            except socket.timeout:
                pass
            if any(u.search(last()) if isinstance(u, re.Pattern) else u in buf for u in until):
                return True
        return False

    def send(text):
        # In small pieces: a guest's console line discipline drops a burst.
        for i in range(0, len(text), 64):
            s.sendall(text[i : i + 64].encode())
            time.sleep(0.05)

    while True:
        send("\r")
        if read(3, LOGIN, SHELL):
            break
        if time.time() > end:
            sys.exit(f"no login or shell prompt on the guest's console in {wait} s")
    if LOGIN.search(last()):
        del buf[:]
        send("root\r")
        read(5, PASSWORD, SHELL)
    if PASSWORD.search(last()):
        sys.exit("the guest's console asks for a password: collect needs a root login without one")
    send("cat > /tmp/gaia-sysfs.sh << 'GAIA_SYSFS_EOF'\r")
    read(1)
    for line in COLLECT.strip().splitlines():
        send(line + "\r")
    send("GAIA_SYSFS_EOF\r")
    read(2)
    del buf[:]
    # The markers are split in what is typed, so the echo does not match.
    send("sh /tmp/gaia-sysfs.sh > /tmp/gaia-sysfs.out 2>&1; echo GAIA-SYSFS-''BEGIN; cat /tmp/gaia-sysfs.out; echo GAIA-SYSFS-''END\r")
    if not read(120, b"GAIA-SYSFS-END"):
        sys.exit(f"no listing from the guest's console in 120 s; it showed:\n{bytes(buf[-2000:]).decode('utf-8', 'replace')}")
    text = bytes(buf).decode("utf-8", "replace")
    found = re.findall(r"GAIA-SYSFS-BEGIN\r?\n(.*?)GAIA-SYSFS-END", text, re.S)
    if not found:
        sys.exit("the guest's console output has no listing between the markers")
    listing = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", found[-1]).replace("\r", "")
    with open(out, "w") as f:
        f.write(listing)
    print(f"{out}: {len(listing.splitlines())} lines")


LINK = re.compile(r"^l\S+\s+.*?\s(\S+) -> (\S+)$")


def target(path):
    """A link's target as a path under /sys/devices, whichever way it is written."""
    path = re.sub(r"^(\.\./)+", "", path)
    path = re.sub(r"^/sys/", "", path)
    return path


def parse(path):
    """A listing's facts: {key: value}."""
    facts = {}
    section = None
    card = None
    # Atlas's block form: the first device with a card ("  device ->").
    disk = None
    with open(path, errors="replace") as f:
        lines = f.read().splitlines()
    for line in lines:
        if card is not None:
            if line.startswith("=="):
                card = None
            else:
                card.extend(line.split())
                continue
        if not line.strip() or line.startswith("total "):
            continue
        m = re.match(r"^/sys/class/(\w+):$", line)
        if m:
            section = m.group(1)
            continue
        m = LINK.match(line)
        if m and section:
            facts[f"{section}/{m.group(1)} link"] = target(m.group(2))
            continue
        m = re.match(r"^card-attributes /sys/class/block/(\S+): (.*)$", line)
        if m:
            facts[f"block/{m.group(1)} card attributes"] = " ".join(sorted(m.group(2).split()))
            continue
        m = re.match(r"^/sys/class/(\w+)/([^/\s]+)/(\w+): ?(.*)$", line)
        if m:
            facts[f"{m.group(1)}/{m.group(2)} {m.group(3)}"] = m.group(4).strip()
            continue
        m = re.match(r"^/sys/block/(\S+) (\w+): (.*)$", line)
        if m:
            facts[f"block/{m.group(1)} {m.group(2)}"] = m.group(3).strip()
            continue
        m = re.match(r"^(\S+) size: (\d+)$", line)
        if m:
            facts[f"block/{m.group(1)} size"] = m.group(2)
            continue
        m = re.match(r"^== (\S+) -> (\S+)$", line)
        if m:
            section = m.group(1)
            facts[f"block/{section} link"] = target(m.group(2))
            continue
        m = re.match(r"^  (\w+)=(.*)$", line)
        if m and section:
            facts[f"block/{section} {m.group(1)}"] = m.group(2).strip()
            continue
        m = re.match(r"^  device -> ", line)
        if m:
            disk = disk or section
            continue
        m = re.match(r"^/sys/devices/\S+/of_node$", line)
        if m and disk:
            # The card's attribute names, in columns, follow the line.
            card = []
            facts[f"block/{disk} card attributes"] = card
            continue
    for key, value in facts.items():
        if isinstance(value, list):
            facts[key] = " ".join(sorted(value))
    return facts


def diff(guest, boards):
    g = parse(guest)
    b = {}
    for board in boards:
        for key, value in parse(board).items():
            if b.setdefault(key, value) != value:
                sys.exit(f"the board listings disagree on {key}: {b[key]!r}, and {value!r} in {board}")
    counts = {"same": 0, "differs": 0, "board only": 0, "guest only": 0}

    def order(key):
        name, _, fact = key.partition(" ")
        return (name.split("/")[0], name, fact != "link", fact)

    for key in sorted(set(b) | set(g), key=order):
        if key in b and key in g:
            kind = "same" if b[key] == g[key] else "differs"
            if kind == "same":
                print(f"same        {key}: {b[key]}")
            else:
                print(f"differs     {key}:\n  board  {b[key]}\n  guest  {g[key]}")
        elif key in b:
            kind = "board only"
            print(f"board only  {key}: {b[key]}")
        else:
            kind = "guest only"
            print(f"guest only  {key}: {g[key]}")
        counts[kind] += 1
    print(", ".join(f"{n} {k}" for k, n in counts.items()))


def main():
    if len(sys.argv) in (4, 5) and sys.argv[1] == "collect":
        collect(sys.argv[2], sys.argv[3], int(sys.argv[4]) if len(sys.argv) == 5 else 600)
    elif len(sys.argv) >= 4 and sys.argv[1] == "diff":
        diff(sys.argv[2], sys.argv[3:])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
