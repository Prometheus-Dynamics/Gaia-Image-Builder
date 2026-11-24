# Gaia `next`: the PR list

Every PR has a brief in `docs/prs/`. `python3 scripts/pr-status.py` shows what is ready.
Needs named `X<n>` are outside blocks, listed in `docs/design.md`.

## Working rows (DO queue, work/do-queue.md), as of 2026-11-23

Since 2026-11-17 work lands on `next` straight from a green local gate, not
through `pr/<id>` PRs, so `scripts/pr-status.py` does not see it. The rows
below are in working order. Done rows have their commit; the PR list after
them has its boxes checked against `git log origin/next`.

Open, in working order:

1. [ ] GADGET-KEEP after-proof: rerun the five-session series on Lemnos's
   rev. Expected: lemnos0's ifindex unchanged, connects carry on through the
   restart, no rebind lines. Blocked on Lemnos landing GADGET-KEEP. Pre-empts
   everything else when it lands.
2. [ ] (a) proof: the same one-line lemnosd change to the bundle with the
   cargo target kept (expected about 40 s). In progress (code landed
   eb5c9bd2). After it: delete the store copy work/item19/store.
3. [ ] Row 17, the Gaia Raze example line: blocked on Lemnos's boot-holds row.
4. [ ] G5 Aura: blocked on the RV1126B loader (u-boot-rockchip.bin).
5. [ ] G4 Cubie: A7S is blocked on Atlas's fragment lines
   (UBOOT_FORMAT_CUSTOM, kernel over the bay). A7A/A7Z are blocked on a
   Makefile hunk that fails at fuzz 0.
6. [ ] G6 K230D on hardware: blocked on board access and the NAND/strap facts.
7. [ ] M2 rows 7–9 (Raze M2 image, size rows, busybox/payload sizes):
   blocked on Lemnos's M2 rev.
8. [ ] Row 40 USB load drop: waiting for the next drop's relay log to name
   the error.
9. [ ] Old rows 2 (B2 env check real-build run) and 5 (B4 Atlas Z2/C1
   slot-image builds): the queue has no closing line for either. Row 2 looks
   covered by item 17 (work/r17/run.txt). Row 5 is unverified. Both are with
   the coordinator to close or keep.

Not scheduled (need a coordinator OK to queue): F9 (lock every Buildroot
download), F14 (Android boot image), F15 (plan notice, bootloader sign of
life, `trial_attempts`), H2 (docs/guide).

Parked (coordinator): (b) dev LTO override (revisit with (a)'s numbers);
row 19 shared host stage (design accepted, no code); 22 .ipk plan; 25 kernel
Rust (mechanism done); 12 SPL check hook; 13 Cubie A7A/A7Z dts as patches;
14 RV1126B port; 15 K230D boot chain.

Done today (2026-11-23): item 19 dev column (no-op 4.4 s; one files change
43.6 s to the image, 7.5 s to the bundle; one lemnosd line 131 s to the
bundle, of which cargo deps 64 s, sibling packages 26 s, fat-LTO 33 s);
376a4b56 EROFS lz4 cluster read-back; e0252eac stock-host re-attach; eb5c9bd2
kept cargo target under --dev-reuse; item B wedge series closed (25 runs, no
board-like wedge); housekeeping (26 worktrees and 31 targets removed, 113G to
4.6G); earlier today 03b3a805 item 10, 8044ed98 item 17, 6f98adbc root
checks, 5765773f item 7, a9be0bff item 8, 7554f760 item 11, 69b06039 row 16,
331a51d7 xattrs, df716c26 Aura, fc9d7827 row 43, 7b9d6379 row 41, 48d81db1
row 42, a42c7ae9 row 40, 08e71181 row 39.

## A. Proof

Three real steps as Daedalus nodes, in parallel, with a forced failure and a retry. If this does not hold, the plan changes here.

- [x] [A1](docs/prs/A1.md) The step kit (wave 1; needs nothing) — done 08a057d9 (#2, 2026-11-11)
- [x] [A2](docs/prs/A2.md) Proof: fetch, build, pack (wave 2; needs A1, B1) — done ee52e2b2 (#4, 2026-11-11)
- [x] [A3](docs/prs/A3.md) Proof: the second run skips (wave 3; needs A2) — done 0b3b4287 (#6, 2026-11-11)

## B. Model

The content store, the port types, the lock and the descriptions.

- [x] [B1](docs/prs/B1.md) The content store (wave 1; needs nothing) — done 4ce21a04 (#1, 2026-11-11)
- [x] [B2](docs/prs/B2.md) Port types (wave 2; needs A1) — done 43f31e1f (#3, 2026-11-11)
- [x] [B3](docs/prs/B3.md) The lock (wave 2; needs B1) — done 65e91870 (#5, 2026-11-11)
- [x] [B4](docs/prs/B4.md) Descriptions (wave 3; needs B2, B3) — done 6123718a (#7, 2026-11-11)
- [x] B5 (no brief; pr/B5) — done 56f34f23 (#47, 2026-11-12)

## C. Sources and builders

Every way Gaia gets source and turns it into artifacts, as nodes.

- [x] [C1](docs/prs/C1.md) Git source (wave 3; needs B2) — done 96261ea2 (#8, 2026-11-11)
- [x] [C2](docs/prs/C2.md) Path, archive and download sources (wave 3; needs B2) — done 7e1d3a3a (#9, 2026-11-11)
- [x] [C3](docs/prs/C3.md) OCI images (wave 3; needs B2) — done c77c2a26 (#10, 2026-11-11)
- [x] [C4](docs/prs/C4.md) Running a step inside a pinned image (wave 4; needs C3, A1) — done 978006a0 (#12, 2026-11-11)
- [x] [C5](docs/prs/C5.md) Rust builder (wave 5; needs C4, C1) — done 203e8d83 (#21, 2026-11-11)
- [x] [C6](docs/prs/C6.md) Command builder (wave 5; needs C4) — done 59389fc2 (#19, 2026-11-11)
- [x] [C7](docs/prs/C7.md) Java builder (wave 5; needs C4) — done 1d926547 (#23, 2026-11-11)
- [x] [C8](docs/prs/C8.md) Node.js builder (wave 5; needs C4) — done be0f607d (#27, 2026-11-11)
- [x] [C9](docs/prs/C9.md) Python builder (wave 5; needs C4) — done 8a15d000 (#22, 2026-11-11)
- [x] [C10](docs/prs/C10.md) Go builder (wave 5; needs C4) — done 2ef6d3d7 (#26, 2026-11-11)

## D. OS providers

Where a root filesystem and kernel come from: Buildroot, an imported image, or none.

- [x] [D1](docs/prs/D1.md) Buildroot (wave 5; needs C4, C1) — done 3c3dc145 (#42, 2026-11-11)
- [x] [D2](docs/prs/D2.md) Buildroot caches and offline builds (wave 6; needs D1) — done d6a475f7 (#66, 2026-11-12)
- [x] [D3](docs/prs/D3.md) Out-of-tree kernel modules (wave 6; needs D1) — done e90ee038 (#61, 2026-11-12)
- [x] [D4](docs/prs/D4.md) Imported image (wave 4; needs C2) — done bc09a01a (#20, 2026-11-11)
- [x] [D5](docs/prs/D5.md) No OS (wave 3; needs B2) — done 74019a38 (#14, 2026-11-11)

## E. Assembly, integrity, checks, reports

Turning artifacts into filesystems, disks, packages and proofs about them.

- [x] [E1](docs/prs/E1.md) File trees (wave 3; needs B2) — done 84cc16c0 (#15, 2026-11-11)
- [x] [E2](docs/prs/E2.md) ext4 (wave 4; needs E1) — done df1cb91a (#24, 2026-11-11)
- [x] [E3](docs/prs/E3.md) EROFS (wave 4; needs E1) — done 17a3d871 (#28, 2026-11-11)
- [x] [E4](docs/prs/E4.md) vfat (wave 4; needs E1) — done aabc7d49 (#32, 2026-11-11)
- [x] [E5](docs/prs/E5.md) UBI (wave 4; needs E1) — done b0701853 (#35, 2026-11-11)
- [x] [E6](docs/prs/E6.md) Partition tables and disks (wave 5; needs E2, E4) — done cf975d0d (#39, 2026-11-11)
- [x] [E7](docs/prs/E7.md) initramfs (wave 4; needs E1) — done 86670710 (#31, 2026-11-11)
- [x] [E8](docs/prs/E8.md) Compression (wave 3; needs B2) — done 9bdf1956 (#16, 2026-11-11)
- [x] [E9](docs/prs/E9.md) Strip and runtime closure (wave 4; needs E1) — done b292574a (#36, 2026-11-11)
- [x] [E10](docs/prs/E10.md) dm-verity (wave 5; needs E3) — done a3edf9c2 (#33, 2026-11-11)
- [x] [E11](docs/prs/E11.md) Signing and verification (wave 3; needs B2) — done 2775d2d2 (#17, 2026-11-11)
- [x] [E12](docs/prs/E12.md) Image checks (wave 6; needs E6) — done 41822606 (#45, 2026-11-12)
- [x] [E13](docs/prs/E13.md) Size budgets (wave 6; needs E6) — done 086a34de (#46, 2026-11-12)
- [x] [E14](docs/prs/E14.md) Reports (wave 6; needs E6) — done e46adce3 (#49, 2026-11-12)
- [x] [E15](docs/prs/E15.md) Packages for other installers (wave 4; needs E1, E8) — done aff8f666 (#38, 2026-11-11)

## F. Recipes, graph, command

Recipes as data, descriptions to a GraphDocument, and the `gaia` command.

- [x] [F1](docs/prs/F1.md) Recipes as data (wave 3; needs A2, B2) — done 7756a784 (#18, 2026-11-11)
- [x] [F2](docs/prs/F2.md) Descriptions to a graph (wave 4; needs F1, B4) — done ee18ba9e (#29, 2026-11-11)
- [x] [F3](docs/prs/F3.md) The `gaia` command (wave 5; needs F2) — done 28c2d137 (#34, 2026-11-11)
- [x] [F4](docs/prs/F4.md) Progress and failure output (wave 6; needs F3) — done 9054deb8 (#50, 2026-11-12)
- [x] [F5](docs/prs/F5.md) Standard recipes (wave 7; needs F2, E6, E7, E10, D1) — done 448be64f (#60, 2026-11-12)
- [x] [F6](docs/prs/F6.md) `gaia lock` (wave 6; needs F3, C1, C2, C3) — done 01f2793c (#40, 2026-11-11)
- [x] [F7](docs/prs/F7.md) SD/eMMC disk image recipe — done 10beed87 (#72, 2026-11-13)
- [x] [F8](docs/prs/F8.md) The OS and the board in the graph — done 5afedf67 (#69, 2026-11-13)
- [ ] [F9](docs/prs/F9.md) Buildroot's downloads in the lock, and D2's caches on the os step — partly done: shared downloads by sha256 db9f61f1, ccache 67c82036, resume f8888603; open: `gaia lock` recording every Buildroot download (not found on next)
- [x] [F10](docs/prs/F10.md) A disk that boots from the slot — done 0f11917e (recipes/slot-disk.toml), bootloader areas c0d084fa (#152)
- [x] [F11](docs/prs/F11.md) A small OS change is a short rebuild — done 265f9a63 (--dev-reuse Buildroot), host seed 7d34b4bd, dev erofs c7198359, Rust target eb5c9bd2
- [x] [F12](docs/prs/F12.md) The firmware-boot family, and files a tool makes at build time — done 987984b3 + bb98a262 (rpi-ramdisk-boot), 7a5bbb16 (assembled files), c3cef016 (generator in sandbox)
- [x] [F13](docs/prs/F13.md) A pinned build root — done 08e71181 (Rust toolchain a pinned source); the C compiler is the product's (old row 26 closed 2026-11-23)
- [ ] [F14](docs/prs/F14.md) A flasher's outputs — partly done: initramfs-boot and rpi-ramdisk-boot recipes, flasher initramfs (rows 41/42); open: an Android boot image for `fastboot boot` (none on next)
- [ ] [F15](docs/prs/F15.md) Smaller things the board files need — partly done: board `patches` series, `--input` (args.rs), `kernel-module` builder 9f741405; open: `gaia plan` no-way-in notice, bootloader USB sign of life, `trial_attempts` counter (none found on next)

## G. Boards

Every board `dev` builds, rebuilt from descriptions and compared file by file with `dev`'s image.

- [~] [G1](docs/prs/G1.md) Raze — superseded: Atlas owns the Raze build (0bfef8c9); the Gaia example line is row 17 (blocked)
- [x] [G2](docs/prs/G2.md) RK35xx family and ZERO 3E — done (rk35xx + zero-3e on next; DO row 1 accepted 2026-11-22)
- [x] [G3](docs/prs/G3.md) ROCK 5C and ROCK 4D — ROCK 5C done b5d7b99f; ROCK 4D dropped (coordinator)
- [ ] [G4](docs/prs/G4.md) Cubie A7A, A7S and A7Z — builds reported 2026-11-23 (Atlas pr/C1, pr/A7AZ); blocked: A7S kernel 43,538,944 B over the 33,554,432 B bay (needs UBOOT_FORMAT_CUSTOM lines in Atlas's fragment), A7A/A7Z Makefile hunk fails at fuzz 0
- [ ] [G5](docs/prs/G5.md) Luckfox Pico Pro and Aura — Aura landed df716c26 + 26125b21, stops by name: no loader (u-boot-rockchip.bin); Pico Pro dropped (coordinator)
- [ ] [G6](docs/prs/G6.md) K230D Zero and AI-CAM — landed 8e05f304, both reach login in QEMU 69b06039 (NOT K230 hardware); board run blocked on board access
- [~] [G7](docs/prs/G7.md) Cast receiver — dropped (coordinator)
- [x] [G8](docs/prs/G8.md) Packages for SystemCore-style installers (wave 8; needs E15, F6) — done b98701eb (#55, 2026-11-12)

## H. Finish

CI, docs, and the lock committed. Replacing `dev` with `next` is the user's call.

- [x] [H1](docs/prs/H1.md) CI and the GitHub action (wave 6; needs F3) — done d51e6d56 (#41, 2026-11-11)
- [ ] [H2](docs/prs/H2.md) Docs — open, not started (no docs/guide on next)
- [ ] [H3](docs/prs/H3.md) Lock and hand-over — blocked on G4, G5, G6, H2; replacing `dev` is the user's call
