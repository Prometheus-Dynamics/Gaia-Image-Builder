# Gaia `next`: the PR list

Every PR has a brief in `docs/prs/`. `python3 scripts/pr-status.py` shows what is ready.
Needs named `X<n>` are outside blocks, listed in `docs/design.md`.

## A. Proof

Three real steps as Daedalus nodes, in parallel, with a forced failure and a retry. If this does not hold, the plan changes here.

- [ ] [A1](docs/prs/A1.md) The step kit (wave 1; needs nothing)
- [ ] [A2](docs/prs/A2.md) Proof: fetch, build, pack (wave 2; needs A1, B1)
- [ ] [A3](docs/prs/A3.md) Proof: the second run skips (wave 3; needs A2, X1)

## B. Model

The content store, the port types, the lock and the descriptions.

- [ ] [B1](docs/prs/B1.md) The content store (wave 1; needs nothing)
- [ ] [B2](docs/prs/B2.md) Port types (wave 2; needs A1)
- [ ] [B3](docs/prs/B3.md) The lock (wave 2; needs B1)
- [ ] [B4](docs/prs/B4.md) Descriptions (wave 3; needs B2, B3)

## C. Sources and builders

Every way Gaia gets source and turns it into artifacts, as nodes.

- [ ] [C1](docs/prs/C1.md) Git source (wave 3; needs B2)
- [ ] [C2](docs/prs/C2.md) Path, archive and download sources (wave 3; needs B2)
- [ ] [C3](docs/prs/C3.md) OCI images (wave 3; needs B2)
- [ ] [C4](docs/prs/C4.md) Running a step inside a pinned image (wave 4; needs C3, A1)
- [ ] [C5](docs/prs/C5.md) Rust builder (wave 5; needs C4, C1)
- [ ] [C6](docs/prs/C6.md) Command builder (wave 5; needs C4)
- [ ] [C7](docs/prs/C7.md) Java builder (wave 5; needs C4)
- [ ] [C8](docs/prs/C8.md) Node.js builder (wave 5; needs C4)
- [ ] [C9](docs/prs/C9.md) Python builder (wave 5; needs C4)
- [ ] [C10](docs/prs/C10.md) Go builder (wave 5; needs C4)

## D. OS providers

Where a root filesystem and kernel come from: Buildroot, an imported image, or none.

- [ ] [D1](docs/prs/D1.md) Buildroot (wave 5; needs C4, C1)
- [ ] [D2](docs/prs/D2.md) Buildroot caches and offline builds (wave 6; needs D1)
- [ ] [D3](docs/prs/D3.md) Out-of-tree kernel modules (wave 6; needs D1)
- [ ] [D4](docs/prs/D4.md) Imported image (wave 4; needs C2)
- [ ] [D5](docs/prs/D5.md) No OS (wave 3; needs B2)

## E. Assembly, integrity, checks, reports

Turning artifacts into filesystems, disks, packages and proofs about them.

- [ ] [E1](docs/prs/E1.md) File trees (wave 3; needs B2)
- [ ] [E2](docs/prs/E2.md) ext4 (wave 4; needs E1)
- [ ] [E3](docs/prs/E3.md) EROFS (wave 4; needs E1)
- [ ] [E4](docs/prs/E4.md) vfat (wave 4; needs E1)
- [ ] [E5](docs/prs/E5.md) UBI (wave 4; needs E1)
- [ ] [E6](docs/prs/E6.md) Partition tables and disks (wave 5; needs E2, E4)
- [ ] [E7](docs/prs/E7.md) initramfs (wave 4; needs E1)
- [ ] [E8](docs/prs/E8.md) Compression (wave 3; needs B2)
- [ ] [E9](docs/prs/E9.md) Strip and runtime closure (wave 4; needs E1)
- [ ] [E10](docs/prs/E10.md) dm-verity (wave 5; needs E3)
- [ ] [E11](docs/prs/E11.md) Signing and verification (wave 3; needs B2)
- [ ] [E12](docs/prs/E12.md) Image checks (wave 6; needs E6)
- [ ] [E13](docs/prs/E13.md) Size budgets (wave 6; needs E6)
- [ ] [E14](docs/prs/E14.md) Reports (wave 6; needs E6)
- [ ] [E15](docs/prs/E15.md) Packages for other installers (wave 4; needs E1, E8)

## F. Recipes, graph, command

Recipes as data, descriptions to a GraphDocument, and the `gaia` command.

- [ ] [F1](docs/prs/F1.md) Recipes as data (wave 3; needs A2, B2)
- [ ] [F2](docs/prs/F2.md) Descriptions to a graph (wave 4; needs F1, B4)
- [ ] [F3](docs/prs/F3.md) The `gaia` command (wave 5; needs F2)
- [ ] [F4](docs/prs/F4.md) Progress and failure output (wave 6; needs F3)
- [ ] [F5](docs/prs/F5.md) Standard recipes (wave 7; needs F2, E6, E7, E10, D1)
- [ ] [F6](docs/prs/F6.md) `gaia lock` (wave 6; needs F3, C1, C2, C3)

## G. Boards

Every board `dev` builds, rebuilt from descriptions and compared file by file with `dev`'s image.

- [ ] [G1](docs/prs/G1.md) Raze (wave 8; needs F5, F6, D3, E12, X2, X3, X4; board run)
- [ ] [G2](docs/prs/G2.md) RK35xx family and ZERO 3E (wave 8; needs F5, F6, E12; board run)
- [ ] [G3](docs/prs/G3.md) ROCK 5C and ROCK 4D (wave 9; needs G2; board run)
- [ ] [G4](docs/prs/G4.md) Cubie A7A, A7S and A7Z (wave 8; needs F5, F6, E12; board run)
- [ ] [G5](docs/prs/G5.md) Luckfox Pico Pro and Aura (wave 8; needs F5, F6, E5, E12; board run)
- [ ] [G6](docs/prs/G6.md) K230D Zero and AI-CAM (wave 8; needs F5, F6, E12; board run)
- [ ] [G7](docs/prs/G7.md) Cast receiver (wave 8; needs F5, F6, D4; board run)
- [ ] [G8](docs/prs/G8.md) Packages for SystemCore-style installers (wave 8; needs E15, F6)

## H. Finish

CI, docs, and the lock committed. Replacing `dev` with `next` is the user's call.

- [ ] [H1](docs/prs/H1.md) CI and the GitHub action (wave 6; needs F3)
- [ ] [H2](docs/prs/H2.md) Docs (wave 9; needs F5, G2)
- [ ] [H3](docs/prs/H3.md) Lock and hand-over (wave 10; needs G1, G2, G3, G4, G5, G6, G7, G8, H1, H2)
