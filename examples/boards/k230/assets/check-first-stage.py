#!/usr/bin/env python3
"""Check a K230 disk image's first-stage areas, as the boot ROM reads them.

Usage: check-first-stage.py <image>

For the SPL at 1 MiB and its copy at 1.5 MiB: the K230 header ('K230', length,
encryption type 0, sha256 of version word + payload, 484 zero bytes), the
version word 0, and the SPL's first instruction csrr a0, mhartid (f1402573).
The Canaan SDK overlay's start.S puts a boot-timing prologue (rdtime, rdcycle,
stores to 0x80200000) before it; with that prologue the K230D AI-CAM's ROM
stopped with "boot failed with exit code 19" (bench, 2026-11-16). Also the
U-Boot area at 2 MiB: header and sha256. Exit 1 on any failure.
"""
import hashlib
import struct
import sys

HEADER = 0x210
CSRR_A0_MHARTID = bytes.fromhex("732540f1")


def area(f, offset, name, spl):
    f.seek(offset)
    head = f.read(HEADER)
    if head[:4] != b"K230":
        return [f"{name} at {offset:#x}: no K230 magic"]
    length, kind = struct.unpack("<iI", head[4:12])
    errors = []
    if kind != 0:
        errors.append(f"{name}: encryption type {kind}, not 0")
    if head[44:] != bytes(HEADER - 44):
        errors.append(f"{name}: reserved header bytes are not zero")
    data = f.read(length)
    if len(data) != length or hashlib.sha256(data).digest() != head[12:44]:
        errors.append(f"{name}: sha256 does not match the payload")
    if data[:4] != bytes(4):
        errors.append(f"{name}: version word is {data[:4].hex()}, not 0")
    if spl and data[4:8] != CSRR_A0_MHARTID:
        errors.append(
            f"{name}: first instruction {data[4:8].hex()}, not csrr a0, mhartid (f1402573)"
        )
    return errors


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with open(sys.argv[1], "rb") as f:
        errors = (
            area(f, 0x100000, "spl", True)
            + area(f, 0x180000, "spl-copy", True)
            + area(f, 0x200000, "uboot", False)
        )
    for error in errors:
        print(error)
    if errors:
        sys.exit(1)
    print("first stage ok: spl, spl-copy, uboot")


main()
