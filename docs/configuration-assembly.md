# Configuration: Image assembly

[Configuration index](configuration.md)

### Image Assembly

Image assembly runs after the image provider has produced its base outputs. It creates named work trees, stages selected files, runs typed transforms, and can package trees into supported filesystem artifacts.

```toml
[image.assembly]
work_dir = "${workspace.build_dir}/assembly"
out_dir = "$provider.images"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.files]]
tree = "boot"
src = "@assets/board/config.txt"
dest = "config.txt"

[[image.assembly.files]]
tree = "boot"
src_glob = "$provider.images/*.dtb"
dest = "."
optional = true

[[image.assembly.files]]
tree = "initramfs"
src = "@assets/initramfs/init"
dest = "init"
mode = "0755"

[[image.assembly.transforms]]
kind = "gzip"
src = "$provider.images/Image"
dest = "$assembly.tree.boot/kernel.img"
deterministic = true

[[image.assembly.transforms]]
kind = "compile-dts"
src = "@assets/overlays/example-overlay.dts"
dest = "$assembly.tree.boot/overlays/example-overlay.dtbo"

[[image.assembly.busybox_initramfs]]
tree = "initramfs"
busybox = "$provider.target/bin/busybox"
include_runtime_libs = false
applets = ["sh", "mount", "mkdir", "switch_root"]

[[image.assembly.filesystems]]
id = "bootfs"
kind = "vfat"
source_tree = "boot"
output = "$provider.images/boot.vfat"
size = "32M"

[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-gzip"
source_tree = "initramfs"
output = "$assembly.tree.boot/initramfs"
deterministic = true

[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "mbr"
signature = "0x48454c49"

[[image.assembly.disks.partitions]]
name = "boot"
type_alias = "fat32-lba"
bootable = true
image = "$provider.images/boot.vfat"

[[image.assembly.disks.partitions]]
name = "rootfs"
type = "0x83"
image = "$provider.images/rootfs.squashfs"
```

Assembly roots:
- `$provider.images`: provider image output directory.
- `$provider.target`: provider target root tree, when the provider exposes one.
- `$provider.host`: provider host tools directory, when the provider exposes one.
- `$provider.staging`: provider staging/sysroot directory, when the provider exposes one.
- `$assembly.work`: configured assembly work directory.
- `$assembly.out`: configured assembly output directory.
- `$assembly.tree.<id>`: path for a declared assembly tree.
- `@assets/...`: workspace asset path.

Assembly behavior:
- Declared `trees` are cleaned and recreated before staging.
- `strip = true` on a tree strips every ELF file in it right before a filesystem or archive packs the tree (the steps that fill the tree have run by then). Executables and shared libraries lose their symbols (`strip --strip-unneeded`), kernel modules (`*.ko`) lose only their debug data (`--strip-debug`), and relocatable objects and non-ELF files are left alone. Only the copies in the tree change, never a source. Hard links are stripped once and stay linked, symlinks are left as they are, and a file that is already stripped is skipped. The run state and summary record the files stripped and the bytes saved per tree.
- `strip_tool = "<program>"` (under `[image.assembly]`) names the strip program every `strip = true` entry uses: a program name looked up in the provider's host tools and then on `PATH`, or a path (it may use `$provider.*` tokens). Unset, the tool is a `<triplet>-strip` in the provider's host tools matching the ELF machine (`aarch64-*-strip`, `arm-*-strip`, `x86_64-*-strip`), else `llvm-strip` on `PATH`, else a plain `strip` when the file is for the host's own machine. When none is found the build fails, naming the file and these options.
- File entries use exactly one of `src`, `src_glob`, `src_dir`, `content` or `json`. `content` and `json` write a generated file (see [Generated content](#generated-content)).
- `src_dir = "<path template>"` copies a whole directory tree under `dest`: the directory's contents, not the directory itself, so a static site's `_app/immutable/chunks/...` lands at the same paths under `dest`. `dest` names a directory: `.`, a path ending in `/`, or a bare directory path. Entries are copied recursively in sorted order, a directory before its contents. Symbolic links are kept as links and never followed. A regular file keeps its permission bits unless `mode` is set, which then applies to regular files only. A directory keeps its source directory's permission bits unless `dir_mode` is set (for example `dir_mode = "0750"`), which then applies to every copied directory; directory modes are set after the contents are copied, so a read-only directory still receives its files. `dir_mode` on any other entry is an error. A hard link is copied as a regular file. `exclude = [...]` leaves out entries by file name at any depth, and an excluded directory is skipped with its contents. An absent or empty directory fails the build unless `optional = true`. `strip = true`, the dynamic-library check and the package device checks apply to each copied regular file as they do to `src`. `src_dir` is refused together with `src`, `src_glob`, `content` or `json`, with `include`, and with `preserve_symlink` (links are always kept). The step reads the directory, so it runs after the steps that write inside it. Its fingerprint covers the whole tree (names, contents, modes and link targets): adding, removing or changing a nested entry reruns the assembly.
- `exclude = ["dwc2.dtbo", "i2c*.dtbo"]` on a `src_glob` entry leaves out matches by file name (simple `*` globs allowed). Use it to copy a stock set except the names a later entry provides, so the result does not depend on copy order. It is refused on other entries.
- Glob staging is deterministic and currently supports simple non-recursive `*` filename patterns. A directory that matches is skipped: a `src_glob` copies its files and links (the same holds for an archive member's `src_glob`).
- A `dest` of `.` or ending in `/` is a directory the sources are copied into; any other `dest` is the file a single source is copied to. A `src_glob` that matches several files needs a directory `dest` (otherwise every match would land on one file) and fails without one.
- `optional = true` skips missing sources without failing the build and records the skip in runtime state.
- A copied file that is dynamically linked (it names a program interpreter or `DT_NEEDED` libraries) is checked against its tree. Once the steps that fill the tree have run, before a filesystem or archive packs it, each interpreter and library the file needs must be in the tree, looked up as [listed binaries](#listed-binaries) look them up. A missing one is a warning, not an error: the run output and the summary name the file and the libraries (`copy it with [[image.assembly.binaries]] to bring its libraries`). A tree may be an overlay on a full rootfs, which is why it is not an error. `allow_missing_libs = true` on the entry accepts the file without the warning. The check reads only ELF headers, never the host, and a static or non-ELF file is never checked. `allow_missing_libs` is refused on `content` and `json` entries.
- `mode = "0755"` applies Unix file modes on Unix hosts.
- `include = ["<name>", ...]` on a `src_glob` entry copies only the matches whose file name one of the names matches (simple `*` globs allowed); it is applied after `exclude`. An entry that matches no file fails the build, naming it, unless `optional = true`. It is refused on other entries. For example, twelve named overlays out of a directory of several hundred: `src_glob = "$provider.target/boot/overlays/*.dtbo"`, `include = ["vc4-kms-v3d.dtbo", "i2c*.dtbo"]`, `dest = "overlays/"`.
- `strip = true` on a file entry strips each copied ELF file (the `src`, or each `src_glob` match) with the strip tool described under the tree; a copied file that is not ELF is left alone. It is refused on `content` and `json` entries.
- Transform kinds currently implemented are `copy`, `gzip`, `zstd`, `xz`, `compile-dts`, `verity` (see [dm-verity and ext4](#dm-verity-and-ext4)) and `uboot-env` (see [U-Boot environment](#u-boot-environment)).
- `gzip` runs `gzip -n -c [-<level>] <src>`. `-n` leaves the name and the modification time out of the header, so the same input and level give the same bytes on every run. The optional `level` (1-9) is accepted on `gzip` transforms and defaults to gzip's own.
- `zstd` runs `zstd -q -c --no-progress -T0 [-<level>] <src>`, resolving `zstd` from provider host tools first, then host `PATH`; the tool path and version are recorded in assembly state. The optional `level` (1-19) is only accepted on `zstd` transforms and defaults to 19, the level `cpio-zstd` uses (it was zstd's own default of 3). `long = true` (`zstd` only, default `false`) adds `--long=27` when the level is 19 and the input is over 128 MiB: a 2^27-byte match window. A stream written that way decodes only with `zstd -d --long=27` (or a decoder that accepts that window); the default output decodes with any zstd.
- `xz` runs `xz -T0 --block-size=24MiB -c [-<level>] <src>`, resolved like `zstd`. Its fixed block size makes the output the same for any core count. The optional `level` (0-9, xz's preset) is only accepted on `xz`, `zstd` and `gzip` transforms, as `gzip`'s and `zstd`'s own levels are. Use it to compress another disk into the output dir, for example `kind = "xz"`, `src = "$assembly.work/slots/<id>.img"`, `dest = "$assembly.out/<id>.img.xz"`.
- A transform, verity, archive or disk output written under `$assembly.out` is a published output of the run: it is listed in the summary and copied by `--export` (see `image.output.disk` above for raw disks).
- `compile-dts` resolves `dtc` from provider host tools first, then host `PATH`.
- `busybox_initramfs` copies the configured BusyBox binary to `bin/busybox` and creates the requested applet symlinks in `bin`. With `include_runtime_libs = true` it also copies the dynamic loader and the shared libraries the binary needs, from the target's sysroot (see the next bullet). Each entry takes:
  - `tree`, `busybox`, `applets`, `include_runtime_libs`: as above.
  - `sysroot` (optional): the target root the runtime libraries are read from, as a path template such as `$provider.target`. Unset, it is derived from `busybox`: the directory above its `bin/` or `sbin/`, or two levels up from `usr/bin/` or `usr/sbin/`. A dynamic binary whose sysroot cannot be derived fails the build, asking for `sysroot`.
- `binaries` copies a binary into a tree together with the runtime it needs (see [Listed binaries](#listed-binaries)).
- `kernel_modules` copies the named kernel modules, and the modules they depend on, from a kernel's `modules.dep` into a tree under `lib/modules/<kernel version>/` at the same relative paths. It also copies `modules.order`, `modules.builtin` and `modules.builtin.modinfo` when present, then runs `depmod -b <tree> <kernel version>`, so the tree's module index covers exactly the copied set. Each entry takes:
  - `tree`: the tree to fill.
  - `from`: the directory holding the `<kernel version>` directories, such as `$provider.target/lib/modules`.
  - `modules`: module names. `-` and `_` are the same, and a name may carry `.ko`, `.ko.xz`, `.ko.zst` and similar suffixes or a path.
  - `kernel_version` (optional): the version directory to use. Required when `from` holds more than one.
  - `depmod` (optional): a path template for `depmod`. Unset, it takes the provider's `sbin/depmod`, then the host's `depmod` from `PATH`; a missing depmod fails the build.
  - `metadata` (optional): the `modules.*` files to install, such as `["modules.dep", "modules.alias"]`. Each name must start with `modules.`. Unset, the default set is installed (`modules.order`, `modules.builtin` and `modules.builtin.modinfo`, when present). Set, only the listed files are installed, and each must exist in the kernel's module directory (a missing one fails the build). depmod still writes its indexes into the tree; the `modules.*` files it wrote that the list does not name are then removed, so `.bin` indexes and `modules.builtin.modinfo` are left out.

  A module the kernel has built in (listed in `modules.builtin`) is skipped and noted. A name that is neither in `modules.dep` nor built in fails the build, naming it. The step runs with the other tree-populating steps, before the filesystems that pack the tree.
- Runtime libraries are found from the binary's ELF headers, never from the host: the program interpreter, the `DT_NEEDED` names and the `DT_RUNPATH` (or `DT_RPATH`) entries are read from the file, and each name is looked up under the sysroot. The search order is the binary's RUNPATH/RPATH entries (`$ORIGIN` is expanded; an entry with another `$` token, or a relative path, is skipped), then `lib`, `lib64`, `usr/lib`, `usr/lib64`, the `<triplet>` directories present under `lib` and `usr/lib`, and the directories of `etc/ld.so.conf` and its `include` files. A library is used only when its class, byte order and machine match the binary, so a host library is never taken for a target one.
- Symlinks inside the sysroot are followed (an absolute link target means the sysroot's root). The tree gets each file the loader reaches as a real copy at its absolute path, and each symlink crossed on the way as a symlink with the same target text, so a soname such as `libc.so.6` resolves in the tree as it does on the target. The interpreter is placed at its exact path. A static binary copies nothing. A NEEDED name that is not found under the sysroot fails the build, and the error lists every missing name. The tree must not create a directory that the sysroot links (for example `lib` when the sysroot has `/lib -> usr/lib`), since the link cannot replace it.
- For a cross-compiled BusyBox taken from a Buildroot target tree, set `sysroot = "$provider.target"` (or leave it unset when the binary sits at `$provider.target/bin/busybox`). The reuse fingerprint covers the sysroot, the interpreter, every symlink and the digest of every library file, so a changed libc rebuilds the assembly.
- Filesystem kinds currently implemented are `vfat`, `cpio`, `cpio-gzip`, `cpio-zstd`, `cpio-xz`, `ext4` (see [dm-verity and ext4](#dm-verity-and-ext4)), `squashfs` and `erofs` (see [squashfs and EROFS](#squashfs-and-erofs)).
- `cpio` kinds write a newc archive with uid and gid 0, entries in sorted order and zero timestamps, so the same tree packs to the same bytes. `cpio-zstd` compresses that archive with `zstd -q -c --no-progress -T0 -<compression_level>`, resolving `zstd` like the zstd transform; `compression_level` (1-19) is only accepted on `cpio-zstd` and defaults to 19. A hard-linked file is stored once: its links share one inode number and link count, and the data goes with the last link in archive order (the form cpio and the kernel's initramfs unpacker read). Device nodes and FIFOs in the tree are written with their mode and device numbers, and `[[image.assembly.devices]]` adds more (see [Device nodes](#device-nodes)). `cpio-gzip` and `cpio-zstd` stream the archive into the compressor's standard input, so no full cpio copy is written; `zstd` is given the planned size (`--stream-size`), which makes its output the same as from a file. `cpio-xz` still writes the cpio to a file first, as its proven-bootable command was tested with that input.
- `cpio-xz` compresses the same newc archive with `xz -T2 --check=crc32 --lzma2=preset=<compression_level>,dict=<size> -c`. The CRC32 check is the one the kernel's XZ initramfs decoder reads. That decoder allocates the whole LZMA2 dictionary, so the dictionary is capped at 1 MiB (or the cpio's size rounded up to 4 KiB, when smaller) whatever the level: the 64 MiB dictionary of `-9` gives an initramfs the kernel cannot unpack. `compression_level` (0-9) is accepted on `cpio-xz` (default 6) and `cpio-zstd` (1-19, default 19 there). Threaded mode writes the block sizes into the block header, which the kernel also needed (a `-T1` stream, without them, did not boot), and xz's own threaded block size is kept: with `--block-size` set to the cpio size xz wrote a shorter block header that did not boot either. The fixed thread count keeps the output the same on every host and run. Of the encodings tried on a Raspberry Pi kernel, only this one booted, with a padded 20-byte block header; a 64 MiB dictionary, a `-T1` stream without sizes, and a 16-byte header from `--block-size` each died before /init. For a small, safe initramfs on that board, `cpio-zstd` is the tested choice; boot-test a `cpio-xz` image (and any one over 3 MiB, which xz splits into several blocks) before relying on it.
- `publish = true` on a filesystem also copies its image to the image output dir (`$assembly.out`, the collect dir by default) under the output's file name, and the published image is reported like a raw disk: in the run summary, its image sizes, and the content digests. The name must not collide with another output or published copy. In a RAM work dir the image is built in RAM and copied to the image output dir when built.
- `vfat` uses `mtools` (`mformat` and `mcopy`), resolving provider host tools first, then host `PATH`; `size` accepts byte values or binary `K`, `M`, `G`, and `T` suffixes, or `size = "auto"`. Without `size` the image is 32M. With `deterministic = true` (the default) the volume serial is fixed (`mformat -N`, derived from the filesystem `id`), every entry carries the modification time 1980-01-01 00:00 UTC (copied from a staging tree, then kept by `mcopy -m`), and mtools runs with `TZ=UTC`: the same spec gives the same bytes on every run and host.
- `fat_size` (vfat only, optional): the FAT type, `12`, `16` or `32`. Without it a vfat image from 3 MiB up to 16 MiB is formatted FAT16 (one sector per cluster) rather than the FAT12 `mformat` picks at those sizes, because some boot firmwares (e.g. Raspberry Pi's) do not boot from FAT12; other sizes keep `mformat`'s choice (FAT12 below 3 MiB, FAT16 from 16 MiB). `fat_size = 12` keeps `mformat`'s choice at every size, `16` is the default rule, and `32` formats FAT32. With `fat_size` set, the build fails when the image size cannot hold that type: FAT12 needs less than 16 MiB, FAT16 at least 3 MiB.
- `free = "256M"` (only with `size = "auto"`) leaves that much free space beyond the content estimate: the margin becomes the larger of the automatic margin and `free`, and the result is still rounded up to a whole MiB. Use it when files will be added to the image later.
- `size = "auto"` (vfat and ext4) sizes the image from its source tree; the vfat rule is: each file is counted rounded up to 4 KiB, each directory as 4 KiB, plus 1 MiB for the FAT tables and root directory, plus a margin of 5% (at least 1 MiB), rounded up to a whole MiB. The size depends only on the tree's content, so the same tree always gets the same size. The 4 KiB per entry is an estimate of the cluster size mtools picks: a tree that overruns it fails while copying, rather than being truncated.
- Raw disk assembly writes 1 MiB-aligned partitions sequentially, with an MBR (`partition_table = "mbr"`, the default) or a GPT (`partition_table = "gpt"`) (see [Disk partitions](#disk-partitions) and [GPT disks](#gpt-disks)).
- MBR partition `type` accepts raw `0xNN` values; `type_alias` currently supports `fat32-lba` and `linux`. On a GPT disk `type` is a raw type GUID and `type_alias` names one (see [GPT disks](#gpt-disks)).
- Assembly runtime state is included in provenance and manifest reports with staged file, transform, filesystem, disk, and archive output sizes and digests.
- Assembly order follows dependencies: a step that reads a path another step writes (the same file, a file in a directory it fills, or a glob it matches) runs after it, whatever its kind or position, so for example a transform compressing a filesystem built by the same assembly sees this run's image. Trees are prepared first; steps that do not depend on each other run in the order dirs, symlinks, files, BusyBox initramfs, kernel modules, transforms, filesystems, UBI images, disks, archives, OCI images. Steps that read each other's outputs in a cycle are rejected, by validation and at run time.
- Before the steps run, outputs of transforms, filesystems, UBI images, disks, archives and OCI images left from an earlier run are removed, so a step reading one before it is rebuilt fails instead of using a stale file.

#### Work directory and RAM

`[image.assembly] work_dir` takes a path (the intermediates' directory, as
above), `"disk"` (the build dir's `assembly` directory, the default) or
`"ram"`.

With `"ram"` the intermediates go to tmpfs under
`/dev/shm/gaia-<user>/<hash>/assembly`: the trees, filesystem images such as
`boot.vfat`, transform outputs under the work dir, and raw disk images. The
published outputs stay on disk. The archive is compressed from the RAM copy
of the raw disk. Every raw disk image is built in RAM and copied to its spec
path on disk when built; the copy is sparse, so the zeros of an unwritten
partition cost no disk writes. The RAM copies are removed when
the assembly ends, whether it succeeded or not.

`/dev/shm` is shared, so `/dev/shm/gaia-<user>` must be a real directory the
current user owns: Gaia creates it private (0700) when it is missing, and a
symbolic link, a file or a directory another user owns stops the build with
an error. Each run records its owner (pid, start time and build dir) in its
`<hash>` dir. Before a run starts, the `<hash>` dirs of dead runs are removed:
those older than 24 hours whose owner pid is gone, whose build dir no longer
exists, or that have no owner record.

In RAM mode a filesystem image such as `boot.vfat` is an intermediate and is
not left in the collect dir; use `"disk"` to keep it there, or set `publish = true`
on the filesystem to copy it to the image output dir.

When `work_dir` is unset it follows `[providers.buildroot] work_dir`: RAM when
that is `"ram"`, disk otherwise. A path, such as
`${workspace.build_dir}/assembly`, keeps the intermediates on disk even when
the Buildroot tree is in RAM; set `work_dir = "ram"` to move them.

RAM is used only when the expected size of the intermediates fits in
`MemAvailable` and in the free space on `/dev/shm` with 4 GiB to spare. The
expected size is an upper bound from the spec: the disks' layouts, the
filesystems' `size` and the transform inputs. Otherwise the assembly runs on
disk and the run says why (`assembly work dir falls back to disk: ...`).

The assembly state records `work_dir.placement` (`ram` or `disk`),
`work_dir.path` and `work_dir.expected_bytes`.

#### Device nodes

`[[image.assembly.devices]]` adds a device node or FIFO to the cpio archive of a tree, so a build that cannot `mknod` into the tree (no root) can still ship `/dev/console` in an initramfs. The filesystems that pack the tree as cpio (`cpio`, `cpio-gzip`, `cpio-xz`, `cpio-zstd`) add them; missing parent directories are added as directories (mode 0755).

```toml
[[image.assembly.devices]]
tree = "rootfs"
path = "dev/console"
kind = "char"        # char | block | fifo
major = 5            # char and block only
minor = 1            # char and block only
mode = "0600"        # permission bits, octal; default 0600
```

- `tree`: a tree id. `path`: relative, inside the tree, and not already an entry of it.
- `kind = "char"` or `"block"` needs both `major` and `minor`; `kind = "fifo"` takes neither.
- `mode`: one to four octal digits, at most `7777`.
- A device node already in the tree (created by a root build) is kept as it is; the list adds nodes the tree does not have.

#### Disk partitions

`[[image.assembly.disks.partitions]]` fields:
- `name`: label used in state and errors; on a GPT disk also the partition name (at most 36 UTF-16 code units, unique on the disk).
- `type` (raw `0xNN`) or `type_alias` (`fat32-lba`, `linux`); defaults to `linux` (`0x83`). On a GPT disk, a type GUID or a GPT alias (see [GPT disks](#gpt-disks)).
- `bootable`: sets the MBR active flag (on GPT, the legacy BIOS bootable attribute, bit 2). Only allowed on primary partitions; validation rejects it on logical partitions (`assembly_partition_bootable_logical`).
- `image`: file written at the partition start: a filesystem, transform or UBI output of the assembly, a provider path or an `@assets/` file (`assembly_partition_image_unknown` otherwise). Optional when `size` is set.
- `size`: partition size in bytes or with a binary `K`/`M`/`G`/`T` suffix (`"16M"`, `"2G"`). Without it the partition is exactly as large as its image (rounded up to 512-byte sectors). With it the image must fit, otherwise assembly fails before the disk is written; the space after the image is left unwritten.
- `wipe = true`: only for empty partitions (no `image`). Writes zeros over the first 1 MiB of the partition (less if the partition is smaller) so filesystem signatures left by a previous flash do not survive when the image is written to a device. Validation rejects `wipe` together with `image`, since an image already overwrites the partition start.
- `materialize = false`: the partition is in the table at its full `size`, but nothing is written to it (no image, no wipe). It needs `size`; validation rejects `image` or `wipe` with it (`assembly_partition_unmaterialized_image`, `assembly_partition_unmaterialized_wipe`). Pair it with `truncate = "last-data"` to leave the partition past the end of the file so it is created on first boot. Defaults to `true`.
- `offset`: a fixed start on the disk, in bytes or with a binary suffix (`"32K"`, `"8M"`). It must be a multiple of 512 and lie past the partition table (sector 1 on MBR, sector 34 on GPT), and it is not rounded to `alignment_lba`. Not allowed on a logical partition (`assembly_partition_offset_invalid`).
- `table = false`: the image is written at its place on the disk but the partition table has no entry for it, as for a bootloader the boot ROM reads by offset. It takes no table slot or partition number, and it rejects `bootable`, `type`, `type_alias` and `guid` (`assembly_partition_table_option`). Defaults to `true`.

Layout:
- Partitions are placed in declaration order, each starting at the next `alignment_lba` boundary (default 2048 sectors) after `first_lba` (default 2048). A partition with an `offset` starts there instead, and the next partition without one follows the furthest end so far.
- Assembly fails when two partitions, a partition and an EBR, or a partition and the partition table share a sector.
- Only partitions in the table count toward the 4-partition limit below and toward the partition numbers.
- With 4 or fewer partitions all of them are primary partitions p1-p4.
- With more than 4 partitions Gaia writes the standard sfdisk extended layout: partitions 1-3 are primary (p1-p3), MBR slot 4 becomes an extended partition (type `0x05`), and partitions 4.. become logical partitions p5, p6, ... Each logical partition has an EBR in the first sector of its aligned slot and its data starts at the next `alignment_lba` boundary. EBR entry 0 describes the logical partition relative to its EBR; entry 1 links the next EBR, relative to the extended partition start, and spans that EBR through the end of its logical partition. The extended entry spans from the first EBR to the end of the last logical partition.
- The disk file is created at its full size with unwritten ranges left as holes, so empty and padded partitions take no space on filesystems that support sparse files.
- Assembly state records each partition's `start_lba`, `sector_count`, image `bytes`, and for layouts with more than 4 partitions the kernel `number` and the `ebr_lba` of logical partitions; empty partitions record `empty=true` and `wipe_bytes`.

##### Example: Rockchip loaders before the GPT partitions

The Rockchip boot ROM reads the loader at sector 64 and U-Boot at 8 MiB, outside any partition:

```toml
[[image.assembly.disks]]
id = "emmc"
output = "$assembly.out/emmc.img"
partition_table = "gpt"

[[image.assembly.disks.partitions]]
name = "idbloader"
image = "$provider.images/idbloader.img"
offset = "32K"
table = false

[[image.assembly.disks.partitions]]
name = "uboot"
image = "$provider.images/u-boot.itb"
offset = "8M"
table = false

[[image.assembly.disks.partitions]]
name = "rootfs-a"
image = "$assembly.work/rootfs.verity.erofs"
offset = "16M"
```

#### GPT disks

`partition_table = "gpt"` writes a GUID partition table: a protective MBR (one `0xEE` entry over the disk), the primary header at LBA 1 with 128 partition entries from LBA 2, and the backup entries and header in the last 33 sectors, each with its CRC32. Partitions are placed as on MBR disks (`first_lba` and `alignment_lba`, both 2048 by default), all of them primary and numbered in order; `first_lba` must be at least 34. The disk ends after the backup header, rounded up to `alignment_lba`, so its last usable LBA is 34 sectors before the end. At most 128 partitions.

Disk options:
- `disk_guid` (optional): the disk GUID. Unset, it is derived from the disk `id`.
- `signature`, `signature_text` and `ebr_placement = "packed"` are MBR-only and rejected on a GPT disk; `truncate` is too, since the backup header is at the end of the full-size disk.

Partition options:
- `type_alias`: one of `linux` (the default), `esp`, `bios-boot`, `microsoft-basic-data`, `xbootldr`, `swap`, `home`, `srv`, `var`, `linux-lvm`, `linux-raid`, and the [Discoverable Partitions](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/) types `linux-root-x86`, `linux-root-x86-64`, `linux-root-arm`, `linux-root-arm64`, `linux-root-riscv64`, `linux-usr-x86-64`, `linux-usr-arm`, `linux-usr-arm64`, `linux-usr-riscv64`, `linux-root-verity-x86-64`, `linux-root-verity-arm`, `linux-root-verity-arm64`, `linux-root-verity-riscv64`, `linux-usr-verity-x86-64`, `linux-usr-verity-arm`, `linux-usr-verity-arm64`, `linux-usr-verity-riscv64`.
- `type`: a raw type GUID, `8-4-4-4-12` hex digits, instead of `type_alias`.
- `guid` (optional): the unique partition GUID (the `PARTUUID`). Unset, it is derived from the disk GUID and the partition name.

The derived GUIDs depend only on the disk id and the partition names, not on the images, so the same config gives the same disk, and a `root=PARTUUID=<guid>` on a kernel command line stays valid when the images change. The assembly state records `disk_guid`, and per partition its `type` (the type GUID) and `guid`.

```toml
[[image.assembly.disks]]
id = "disk"
output = "$assembly.out/disk.img"
partition_table = "gpt"

[[image.assembly.disks.partitions]]
name = "ESP"
type_alias = "esp"
image = "$assembly.work/esp.vfat"

[[image.assembly.disks.partitions]]
name = "root"
type_alias = "linux-root-arm64"
image = "$assembly.work/root.erofs"
```

#### Truncated disks

`[[image.assembly.disks]]` options that make the raw file shorter than the disk:
- `truncate = "last-data"`: the file ends after the last written byte instead of at the end of the full-size layout. The end is the largest of: the partition table sectors (the MBR and every EBR), each materialized partition's image end (its start plus the image's size, not the partition size), and the full size of each `wipe = true` partition. The result is rounded up to `alignment_lba` (1 MiB by default) and capped at the full size. Unset (the default) keeps the full size. Only `mbr` is supported; `gpt` with `truncate` is a validation error, since a GPT backup header lives at the end of the full-size disk.
- The partition table still lists every partition at its full size. The bytes past the end are not written, so the partitions beyond it are blank until the disk is written to a device of its full size (or repaired with `sfdisk`/`partx` after the first boot).
- The size checks still apply against the partition size: an image larger than its partition fails before the disk is written.
- The RAM path, the archives (xz, zstd, gzip, raw) and the content digests all use the shorter file. The summary reports the raw and written sizes per image (see below).
- `ebr_placement = "default"` (the default) puts each EBR right before its logical partition, so a logical partition with no data at the end of the disk pulls its EBR, and so the file, a little further out. Nothing is cut off: the file always covers every EBR.
- `ebr_placement = "packed"`: the EBR chain of the extended partition is written in consecutive sectors at its start (EBR n at the extended start plus n sectors). Logical data starts after the chain, so with `truncate` the file ends after the last logical data and every logical partition is still described at its full size. Each EBR's first entry describes its logical partition relative to that EBR; its second entry links the next EBR, relative to the extended start. The planner errors if the chain leaves no room for the first logical partition. Only affects layouts with more than four partitions; use it with `truncate` when logical partitions are unmaterialized at the end of the disk.

Assembly results report, per published raw disk image (and the primary image), `raw_bytes` (file length) and `content_bytes` (bytes the filesystem allocates, from `st_blocks`, so holes are excluded). The run summary has them in `image_sizes` and adds a note such as `sdcard.img: 1.36 GB raw, 412.00 MB written`. Every published output's file size, and its change since the previous run, is in the summary's `sizes` list (see [Image sizes](reporting-and-state.md#image-sizes)).

#### U-Boot environment

A `uboot-env` transform writes a U-Boot environment image, the bytes `mkenvimage` writes, from a text file of `name=value` lines (`src`). Blank lines and `#` comments are skipped. A line without a name before its `=`, or a name set twice, is an error. Lines do not continue with a trailing backslash.

- `env_size` (required): the board's `CONFIG_ENV_SIZE`, a byte size such as `"64K"`. The image is exactly that long. Variables that do not fit are an error (`assembly_uboot_env_size_invalid` for a missing or too small size).
- `redundant = true`: the layout of a redundant environment (`CONFIG_ENV_REDUNDAND`): a flags byte after the CRC, set to the active copy. Write the same image at both of the board's environment offsets.

The image is a little-endian CRC32 of the data area, the flags byte when redundant, then each `name=value` and a NUL, one more NUL after the last, and `0xff` to the end. Variables keep their file order. Generate the text with a `content` file to fill in tokens. For example, an A/B environment at a Rockchip board's 4 MiB offset, outside the partition table:

```toml
[[image.assembly.trees]]
id = "env"
path = "$assembly.work/env"

[[image.assembly.files]]
tree = "env"
dest = "uboot-env.txt"
content = "boot_slot=a\nbootcount=0\nbootlimit=3\naltbootcmd=run swap_slot; run bootcmd\nroothash_a=${assembly.verity:$assembly.work/rootfs-a.erofs:root_hash}\n"

[[image.assembly.transforms]]
kind = "uboot-env"
src = "$assembly.tree.env/uboot-env.txt"
dest = "$assembly.work/uboot.env"
env_size = "32K"
redundant = true

[[image.assembly.disks.partitions]]
name = "env"
image = "$assembly.work/uboot.env"
offset = "4M"
table = false
```

#### UBI images

`[[image.assembly.ubis]]` writes a UBI image for raw NAND flash (SPI-NAND
or parallel NAND), the bytes `ubinize` writes, without mtd-utils. Each
volume holds an image the assembly writes, such as an EROFS slot with its
verity hash tree, or stays empty for the system to fill. A read-only slot
is mounted through `ubiblock`, so dm-verity sits on it as on a disk
partition. UBI images are written after the filesystems and transforms
and before the disks, so a disk can place one at an offset next to the
loaders (G1's `table = false`).

```toml
[[image.assembly.ubis]]
id = "nand"
output = "$assembly.out/nand.ubi"
peb_size = "128K"      # the flash's eraseblock
min_io_size = "2K"     # the flash's page
flash_size = "128M"    # the UBI partition's whole flash device
# sub_page_size, vid_header_offset, image_seq, bad_pebs_per_1024: below

[[image.assembly.ubis.volumes]]
name = "rootfs_a"
image = "$assembly.work/rootfs.erofs"
size = "40M"           # room for later, larger updates

[[image.assembly.ubis.volumes]]
name = "rootfs_b"
size = "40M"           # empty; the updater writes it

[[image.assembly.ubis.volumes]]
name = "data"
size = "4M"
autoresize = true      # grows over the free flash on first attach
```

- `peb_size` and `min_io_size` (required): the flash's eraseblock and page size, byte sizes and powers of two. They must match the flash part: the kernel refuses an image whose header offsets differ from the ones it derives from the MTD device.
- `sub_page_size` (optional): the flash's sub-page size, when it has sub-pages (`ubinize -s`). Unset means none, which is the common case for SPI-NAND. The VID header goes at the sub-page size and data at the next page after it: for a 2 KiB page, at 2 KiB and 4 KiB, so each 128 KiB PEB holds a 124 KiB LEB.
- `vid_header_offset` (optional): sets the VID header offset directly (`ubinize -O`). Only for a kernel attached with the same offset (`ubi.mtd=<dev>,<offset>`).
- `image_seq` (optional): the image sequence number. Unset, it is derived from the `id`, so the same config always writes the same bytes.
- `flash_size` (optional): the size of the whole flash device the image is for. Set, the build fails when the volumes' reservations, the layout volume, UBI's two working PEBs and the bad-block reserve do not fit it. The step's message and the assembly state (`minimum_flash_bytes`) give the smallest flash the image fits, set or not.
- `bad_pebs_per_1024` (optional, default 20): the PEBs UBI keeps for bad blocks per 1024 PEBs of the whole device, Linux's `CONFIG_MTD_UBI_BEB_LIMIT`. At most 768.
- Each volume takes:
  - `name` (required): 1 to 127 bytes, unique in the image.
  - `id` (optional): the volume id; unset, its position in the list. Ids are unique and below the number of volume table records (128 for any LEB over 21.5 KiB).
  - `type` (optional): `dynamic` (the default) or `static`. A static volume records each LEB's size and CRC, which UBI checks when the volume is opened; use it for read-once blobs such as a kernel. Slots that an updater rewrites with `ubiupdatevol` work as either.
  - `image` (optional): the volume's contents, a filesystem or transform output, a provider path or an `@assets/` file. Unset, the volume is empty.
  - `size` (optional): the bytes reserved for the volume, rounded up to whole LEBs and at least the image. Unset reserves the image's size. A volume needs an `image`, a `size` or `autoresize`.
  - `autoresize = true`: UBI grows this volume over the free PEBs on first attach. At most one volume.

The image holds the layout volume (two copies of the volume table, in PEBs 0 and 1) and one PEB per LEB of data, in volume order. Reserved LEBs past a volume's data, and empty volumes, take no PEBs in the image: UBI hands them out from the erased flash after it. Every erase counter is 0, as `ubinize` writes it.

Flash the image with `ubiformat -f nand.ubi /dev/mtdN` (or `ubi write` from U-Boot after `ubi part`), not with `nandwrite`: a UBI PEB ends in erased pages, and programming those `0xff` pages leaves them unusable for UBI's later writes. A factory programmer must skip `0xff` pages in the same way.

On the target, attach and map a slot with `ubi.mtd=<partition> ubi.block=0,rootfs_a` on the kernel command line, then put dm-verity on `/dev/ubiblock0_0`. In U-Boot, `ubi part <partition>` and `ubi read <addr> <volume>` read a volume.

#### Archives

`[[image.assembly.archives]]` builds a plain POSIX ustar archive after all disks, written by Gaia itself (no external `tar`):
- `id`: unique archive id.
- `output`: path template, like other assembly outputs; it can point into the image collect dir (`$provider.images/...`).
- `members`: ordered list. Each member has exactly one source: `src` (a path template; the file is copied, stored as `name`), `entries` (a generated `KEY=value` file, stored as `name`), `json` (a generated JSON document, see [Generated content](#generated-content)), `tree` or `src_dir` (every entry under an assembly tree or a directory), or `src_glob` (each file the pattern matches).
- `tree = "<tree id>"` and `src_dir = "<path template>"` archive every entry under that root, stored under `prefix` (empty, or a relative path ending in `/`). `name` may be left empty for these. Entries are walked in sorted order, a directory before its contents: regular files keep mode 0644, or 0755 when executable; directories are 0755; symbolic links are stored as links and not followed. Only regular files appear in the archive's state. A `tree` member reads its tree's root, so it runs after the steps that fill the tree, and its fingerprint covers the tree's content.
- `src_glob = "<pattern>"` archives each file the pattern matches under `name`, a directory prefix ending in `/`, followed by the file's own name.
- `exclude = [...]` on a `tree` or `src_glob` member leaves out files (not directories) by name, simple `*` globs allowed.
- Names and link targets longer than 100 bytes are written as ustar allows or as PAX extended headers say: a name that splits at a `/` into a directory part of at most 155 bytes and a base of at most 100 uses the ustar `prefix` field; any other long name, and any long link target, goes in a PAX `path` or `linkpath` record. `tar -tf` lists such archives.
- `generated`: ordered list of generated members (`name`, `entries`). They are written **before** all `members`; to interleave generated and file members, put `entries` directly on a `members` item instead.
- `entries`: `[["KEY", "value"], ...]`, rendered as one `KEY=value` line per entry in order. Keys must match `[A-Za-z_][A-Za-z0-9_]*`. Values made only of `A-Z a-z 0-9 _ @ % + = : , . / -` are written as-is; anything else (including an empty value) is wrapped in single quotes with embedded `'` written as `'\''`, so the file can be sourced by `sh`. Values cannot contain newlines.
- Entry values may contain `${assembly.sha256:<path template>}`, replaced at assembly time by the lowercase hex sha256 of that file (an assembly output such as a transform result, or any other file). The file must exist when the archive is built. Normal config interpolation such as `${build.version}` runs first and may also appear inside the path (`${assembly.sha256:$assembly.out/${build.name}.img}`). Any other `${...}` left in a value is a validation error.
- Entry values may also contain `${assembly.size:<path template>}`, replaced by the size of that file in bytes.
- Entry values may also contain `${assembly.verity:<path template>:<field>}`, replaced by a field of a `verity` transform's output. The path must be the transform's `dest` written exactly as in the transform. The fields are `root_hash`, `salt`, `hash_offset`, `data_blocks`, `data_block_size` and `hash_block_size`; see [dm-verity and ext4](#dm-verity-and-ext4). A token that names no verity transform output is a validation error, and an unknown field name is rejected at validation too.

Archive output is deterministic: entries appear in the configured order with mtime 0, uid/gid 0, empty user and group names, and mode `0644`. Member names must be relative paths of at most 4096 bytes (longer names use PAX records, see above) without `.`/`..` components, and must be unique per archive. The archive is written to a temporary file and published atomically; state records `archives.N.output`, `bytes`, `sha256`, and per-member name, source, size, and sha256. Archive outputs are treated as expected outputs, so a bundle in the collect dir is not reported as a large unexpected file. Reuse fingerprints cover the archive config and the state of every member and digest input file.

#### OCI images

`[[image.assembly.oci]]` builds an OCI image from layers after the archives, with no container daemon and no `docker` or `podman` needed to build it:
- `id`: unique id.
- `dest`: path template. With `format = "oci-layout"` (the default) it is a directory holding an OCI image layout (`oci-layout`, `index.json`, `blobs/sha256/...`) plus the `manifest.json` that `docker save` also writes. `skopeo copy oci:<dir> ...` reads the directory; `podman load` and `docker load` (with Docker's classic image store too) take a tar of it (`tar -C <dir> -cf image.tar .`). With `format = "docker-archive"` it is a `.tar` that `docker load` and `podman load` take as it is. Gaia's tests load and run both formats with podman and docker, and read them with skopeo.
- `layers`: lowest first, at least one. Each layer is `{ tar = "<path template>" }`, an uncompressed layer tar used as it is (such as `$provider.images/layers/base.tar`), or `{ tree = "<tree id>" }`, an assembly tree that Gaia tars itself. Exactly one of the two per layer. A tree layer strips its tree first when the tree says `strip = true`, as archives do.
- `compression`: `"gzip"` (the default), `"zstd"` or `"none"`. `docker-archive` takes gzip or none.
- `ref`: optional, such as `"helios-sim:dev"`. It is the `org.opencontainers.image.ref.name` annotation of an OCI layout, or the `RepoTags` entry of a docker archive.
- `[image.assembly.oci.config]` is the image config:
  - `architecture`: required, `"amd64"` or `"arm64"`.
  - `os`: `"linux"` (the default and the only value).
  - `entrypoint`, `cmd`, `env` (`KEY=value` strings), `working_dir` (an absolute path), `stop_signal`, and `labels` (a table). Empty settings are left out of the config.
  - Label values and the other strings take `${source.<id>...}` tokens, as the rest of `[image.assembly]` does.

```toml
[[image.assembly.oci]]
id = "sim"
dest = "$assembly.out/helios-sim.oci"
layers = [ { tar = "$provider.images/layers/base.tar" }, { tree = "app-tree" } ]
ref = "helios-sim:dev"

[image.assembly.oci.config]
architecture = "amd64"
entrypoint = ["/sbin/init"]
env = ["container=docker"]
working_dir = "/"
stop_signal = "SIGRTMIN+3"
labels = { "org.opencontainers.image.revision" = "${source.x.commit}" }
```

Output is deterministic: the same inputs give the same bytes and digests. A tree layer is tarred with entries sorted, a directory before its contents, mtime 0, uid and gid 0 (assembly trees do not record ownership), the permission bits kept, symbolic links kept as links, and a second name for a file written as a hard link to the first. Device nodes and FIFOs in a tree are an error for a layer. The config's `created` time is `SOURCE_DATE_EPOCH` when that is set, else `1970-01-01T00:00:00Z`. Compression runs the assembly's `gzip -n -c -6` or `zstd -q -c -3` (one thread, fixed levels).

Each layer's `diff_id` is the sha256 of its uncompressed tar. Compressed blobs are cached under the build dir (`<build dir>/oci-blobs/<diff_id>.gzip` or `.zstd`), so an unchanged layer is not compressed again: an app-only change rewrites one blob, the config and the manifest. The layer tar of a `tar` layer is read each run to get its `diff_id`; a tree layer is tarred each run.

State records `oci.N.id`, `format`, `compression`, `output`, `bytes`, `config_digest`, `manifest_digest` and `layer_count`, and for each layer `oci.N.layer.M.source`, `diff_id`, `blob_digest`, `blob_bytes` and `reused`. The output is an expected output of the assembly, so it is reported with its size. Reuse fingerprints cover the OCI config, the state of each `tar` layer, and the content digest of each `tree` layer.

#### Packages (ipk, deb)

`[[image.assembly.packages]]` turns an assembly tree into a package an existing OS installs: an opkg `ipk` (OpenWrt, Yocto/Buildroot-based OSes such as Limelight's SystemCore) or a Debian `deb` (Debian, Ubuntu, Raspberry Pi OS). It is how one build ships the same software as a guest on someone else's OS instead of owning the image; pair it with `[image] kind = "none"` when no image is built at all, and with `[[toolchains]]` to build against that OS's SDK. Both formats are an `ar` archive of `debian-binary` (`2.0`), `control.tar.gz` and `data.tar.gz`, written byte-identically for the same inputs; no `opkg-build` or `dpkg-deb` is needed. It runs after the OCI images. Each entry takes:

- `id`: unique id.
- `format`: `"ipk"` (default) or `"deb"`. A `deb` always starts with `debian-binary`, so `debian_binary = false` is an error there; its `Architecture` uses Debian names (`arm64`, `amd64`, `armhf`).
- `tree`: the assembly tree whose files, as installed, are the data. An ipk's data tar has no `./` root entry (opkg records it as a file named `.` and treats it as obsolete on upgrades); a deb's keeps it, as dpkg-deb writes it.
- `dest`: a directory when it ends in `/`, and the package file is named `<Package>_<Version>_<Architecture>.ipk` (or `.deb`) in it; otherwise the path of the file.
- `debian_binary`: `true` (the default) writes the `debian-binary` member first, holding `2.0`. `false` gives the layout of Limelight's examples, with only `control.tar.gz` and `data.tar.gz`.
- `allowed_prefixes`: optional absolute paths. Every file, symlink and hard link of the data must be under one of them; directories are not checked, so the parents of an allowed file are fine.
- `[image.assembly.packages.control]`: the control fields, in the spelling opkg and dpkg use. `Package`, `Version`, `Architecture`, `Maintainer` and `Description` are required. `Architecture` has no default, so name the target (such as `aarch64`) rather than `all`. `Depends` is a list, joined with `, `. `Section`, `Priority`, `Source` and `Homepage` are optional single-line strings. `Description` takes continuation lines after its first line; an empty line is written as ` .`. `Installed-Size` is added by Gaia (the regular files' bytes, rounded up to KiB).
  - `[image.assembly.packages.control.extra]`: further fields, as `X-Port = "5801"`. A name is letters, digits and `-`, and may not repeat a field the package sets. Values are single lines.
- `[image.assembly.packages.scripts]`:
  - `services`: systemd unit file names, each of which must be in the tree under `etc/systemd/system` or `usr/lib/systemd/system`. Gaia generates the maintainer scripts for them: `postinst` runs `daemon-reload`, then for each unit `reset-failed` (so a unit the old version left failed at its start limit is not refused), `enable` and `start`; `prerm` stops and disables each one, tolerating failures; `postrm` runs `daemon-reload` and resets each unit. Each script is POSIX sh and ends in `exit 0`. With no services, no script is generated.
  - `postinst`, `preinst`, `prerm` and `postrm`: a file that replaces the generated script (or, for `preinst`, adds one). `@self/...` paths are resolved as in other config files.
  - `conffiles`: absolute guest paths, each a file of the tree. They are listed in the control tar's `conffiles`.

```toml
[[image.assembly.packages]]
id = "helios-systemcore"
format = "ipk"
tree = "pkg-root"
dest = "$assembly.out/"
debian_binary = true
allowed_prefixes = ["/usr/local", "/etc/systemd/system"]

[image.assembly.packages.control]
Package = "helios"
Version = "${build.version}"
Architecture = "aarch64"
Maintainer = "Helios Team <dev@example.com>"
Description = "Helios SystemCore services"
Depends = ["libc6 (>= 2.38)"]
Section = "base"
Priority = "optional"
extra = { X-Port = "5801" }

[image.assembly.packages.scripts]
services = ["helios-api.service"]
postinst = "@self/pkg/postinst"
conffiles = ["/etc/helios/startup.toml"]
```

Output is deterministic: the data and control tars are written with entries sorted, a directory before its contents, `./`-prefixed names, mtime `SOURCE_DATE_EPOCH` when that is set (else 0), uid and gid 0, the permission bits kept, and links kept. Both tars are gzipped with `gzip -n -9`, so the same inputs give the same bytes. The archive is read back before it is published: the members must be in order and gunzip and list, the control must parse and name the package, the scripts must be executable, each service unit and conffile must be in the data, and every file must be under `allowed_prefixes`. A failed check fails the build and leaves no package in `dest`.

State records `package.N.id`, `format`, `output`, `bytes`, `sha256`, `files` (the non-directory entries of the data) and `installed_kib`. The package is an expected output of the assembly, so it is reported with its size. Reuse fingerprints cover the tree's content digest and any script file the spec names.

##### Device checks

`[image.assembly.packages.device]` checks the package's ELF files against the
device before the package is written, so a package built for the wrong device
never reaches the archive. The device is a sysroot: the libraries and the C
library of the device OS. It takes one of:
- `toolchain = "<id>"`: the sysroot of a `[[toolchains]]` entry (see
  [Toolchains](configuration-sources.md#toolchains)); the package waits for that toolchain's sources.
- `sysroot = "<path>"`: a directory (a workspace path) that is the sysroot.

and these limits, all optional:
- `max_glibc = "2.17"`: the highest `GLIBC_x.y` a file may need. Default: the
  highest version the sysroot's `libc.so.6` defines (`GLIBC_PRIVATE` is not a
  level and is ignored).
- `min_page_size`: the smallest `PT_LOAD` alignment, in bytes, a power of two.
  Default: the toolchain's `page_size` when `toolchain` names one that sets it,
  else 4096 (the smallest page size a kernel uses). An explicit value wins.
- `check_needed = true`: every `DT_NEEDED` library, and the program
  interpreter, must be found in the sysroot or in the package itself. A
  `$ORIGIN` in RUNPATH or RPATH is taken as the file's directory, as the loader
  does. Set `false` to skip this.

The checks are:
1. The architecture: the sysroot's `libc.so.6` gives the class, byte order and
   machine every ELF file must have. An x86-64 file in an AArch64 package is a
   problem, as is an ELF file that cannot be read.
2. Symbol versions: each `GLIBC_x.y` a file needs (its `DT_VERNEED` entries)
   must be at most `max_glibc`.
3. Alignment: each `PT_LOAD` segment's `p_align` must be at least
   `min_page_size`.
4. Libraries (with `check_needed`): each `DT_NEEDED` name resolves, the way
   the loader would look it up, in the package or the sysroot.

Only regular files are read (symlinks are not followed), and the files are
read as bytes: nothing in the package runs. Every problem is reported at once,
one line per file and problem, named by the file's path in the package and by
the symbol version, alignment or library. A package with problems fails the
build, and no `.ipk` is written.

```toml
[image.assembly.packages.device]
toolchain = "systemcore"
max_glibc = "2.41"
min_page_size = 16384
check_needed = true
```

#### Generated content

Three ways to write generated text or JSON into a tree or an archive. All three render `${assembly...}` tokens at assembly time and run after the steps whose outputs they read, so a file can name a transform's output that is declared after it.

- **Text files**: `[[image.assembly.files]]` with `content = "..."` (in place of `src`/`src_glob`) writes the text to `dest` in the tree. `mode` applies as for copied files; unset, a generated file is `0644` whatever the host's umask. `dest` must name a file, not a directory. Config interpolation such as `${build.name}` runs first. Any other `${...}` left in the text is an error, so a file that carries its own `${VAR}` (a shell script, say) belongs in `src` rather than `content`.
- **JSON documents**: `json = { ... }` on a file entry, or on an archive `members` or `generated` item (in place of `entries`/`src`). It is a TOML table whose values are strings, integers, booleans, arrays and tables. Floats are rejected. Keys keep the order the file wrote them, and the output is pretty-printed with two-space indentation and a trailing newline, so two runs write the same bytes.
- **Tokens** in any string value: `${assembly.sha256:<path>}` (hex digest), `${assembly.size:<path>}` (size in bytes) and `${assembly.verity:<path>:<field>}`. A string that is exactly one numeric token (`size`, or a verity `hash_offset`, `data_blocks`, `data_block_size` or `hash_block_size`) is written as a JSON number; any other token is written as text.
- **Tree digest map**: a JSON string that is exactly `${assembly.sha256_tree:<tree id or path template>}` is replaced by an object mapping each regular file's path under that tree (relative to its root, `/` separators, sorted by path) to its lowercase hex sha256. Symbolic links and directories are not listed. An empty tree gives `{}`. It is a whole JSON value only: in `content`, an entry or inside a longer string it is a validation error. The tree is read after the steps that fill it, so a file declared before its tree's files still digests all of them, and the map is a function of the bytes, so rewriting a file with the same bytes keeps the assembly reused. A file cannot digest the tree it is written into (`assembly_tree_digest_self`). For example, a boot tree's `slot.json` naming the payload:

  ```toml
  [[image.assembly.files]]
  tree = "boot"
  json = { boot = { files = "${assembly.sha256_tree:slots-payload}" } }
  dest = "slot.json"
  ```
- `${build.date}` is the build's time as RFC 3339 UTC (`2026-10-15T12:00:00Z`): `SOURCE_DATE_EPOCH` when the environment sets it, else the commit time of the build file's git `HEAD`. Outside both it stays unresolved and the assembly fails rather than write a wall-clock time.

A token that reads a work or out path no step writes is a validation error (`assembly_token_path_not_produced`), since nothing would produce it before the read. The rest of the rules are the archive rules above.

```toml
[[image.assembly.files]]
tree = "boot"
dest = "cmdline.txt"
content = "console=tty1 roothash=${assembly.verity:$assembly.work/slot/rootfs.erofs:root_hash} rootwait"

[[image.assembly.files]]
tree = "boot"
dest = "state.json"
json = { schema = 1, state = "trial", size = "${assembly.size:$assembly.work/slot/rootfs.erofs}" }
```

#### Listed binaries

`[[image.assembly.binaries]]` copies one binary from a target tree into a tree, with the runtime closure a dynamically linked binary needs: its interpreter and its shared libraries, found under a sysroot and copied to the same absolute paths in the tree (symlinks crossed on the way are recreated). A static binary is only copied. Each entry takes:
- `tree`: the tree to fill.
- `src`: the binary, a path template such as `$provider.buildroot_output/target/usr/sbin/e2fsck`.
- `dest`: where it goes in the tree, such as `usr/sbin/e2fsck`. It must be a file path inside the tree.
- `sysroot` (optional): the target root its libraries are read from. Unset, it is derived from `src`: the directory above its `bin/` or `sbin/` (or `usr/bin/`, `usr/sbin/`).
- `mode` (optional): octal, `0755` unless set.
- `strip` (optional): `true` strips the copied program and every copied runtime library in the tree. The source is never touched; a static binary's copy is stripped too.

Several binaries may share libraries: each library is copied once per binary, the later copy replacing the same file. Reuse fingerprints cover the binary and its closure, like BusyBox's.

A `[[image.assembly.files]]` entry that copies a dynamic binary without its libraries gets a warning naming them (see [Image Assembly](#image-assembly)); `binaries` is the entry that brings them. Libraries a `binaries` entry or another file entry brings into the tree count, whatever the entries' order.

#### squashfs and EROFS

Read-only compressed images of `source_tree`, as large as their content (neither kind takes `size` or `free`). `squashfs` runs `mksquashfs`, `erofs` runs `mkfs.erofs`, each resolved from provider host tools first, then host `PATH`. Both are reproducible, and `deterministic` must stay `true`: every entry is owned by root (0:0) with the epoch as its time, and extended attributes from the build host (its SELinux labels, for example) are left out. File modes are kept. Two runs over the same tree give the same bytes. Either image can be the `src` of a [`verity`](#dm-verity-and-ext4) transform, a partition image, or a published output (`publish = true`).

Options:
- `compression`: for `squashfs` one of `gzip` (the default, which every squashfs kernel reads), `xz`, `zstd` or `lz4`; for `erofs` one of `lz4hc` (the default, read by the kernel's LZ4 decoder), `lz4`, `lzma`, `deflate` or `none` (uncompressed).
- `compression_level`: `squashfs` `gzip` 1-9 and `zstd` 1-22 (`-Xcompression-level`); `erofs` `lz4hc` 0-12, `lzma` and `deflate` 0-9 (`-z<name>,<level>`). The other compressors take no level. Unset uses the tool's default.
- `block_size`: bytes, a power of two: `squashfs` 4 KiB to 1 MiB (default 128 KiB), `erofs` 512 to 64 KiB (default 4096; the kernel mounts only block sizes up to its page size).
- `erofs` only: `pcluster_size` (the largest physical cluster, `-C`, a multiple of the block size), and `dedupe`, `fragments`, `ztailpacking` (`-E`), each `false` by default. They need compression.
- `erofs` only: `label` (at most 16 bytes) and `uuid`. Unset, the UUID is derived from the filesystem `id`.

`mksquashfs` gets `-noappend -all-root -no-xattrs -mkfs-time 0 -all-time 0 -reproducible`; `mkfs.erofs` gets `-T0 --all-root -x-1 -U <uuid>`.

```toml
[[image.assembly.filesystems]]
id = "root"
kind = "erofs"
source_tree = "rootfs"
output = "$assembly.work/root.erofs"
compression = "lz4hc"
compression_level = 12
pcluster_size = 65536
fragments = true
```

#### dm-verity and ext4

Two kinds for images that a device mounts read-only or from a data partition.

**`verity`** (a transform) builds a dm-verity hash tree over `src`, a data image such as an EROFS root filesystem, with `veritysetup format`. `veritysetup` resolves from provider host tools first (`bin/`, `usr/bin/`, or the host dir), then host `PATH`; a missing tool fails the build with a clear error. Options, all for `kind = "verity"` only (other kinds reject them):
- `dest`: the output data image. The data is copied and zero-padded to whole `data_block_size` blocks. Unless `hash_dest` is set the hash tree is appended after the padded data, so `dest` is one file that holds the data and its tree.
- `hash_dest` (optional): write the hash tree to its own file instead; `dest` then holds the padded data alone.
- `data_block_size`, `hash_block_size`: powers of two from 512 to 4096, default 4096.
- `salt` (optional): hex, 1 to 256 bytes. Unset, the salt is the sha256 of `src`, so the same data always gets the same tree.
- `uuid` (optional): the superblock UUID. Unset, it is derived from the sha256 of `src`.

The tree uses sha256 and the default superblock. The layout is recorded for the consumer in the assembly state under `verity.<n>.*`, and archive entries reach it with `${assembly.verity:<dest>:<field>}`:

| Field | Meaning |
| --- | --- |
| `root_hash` | the root hash, lower-case hex |
| `salt` | the salt, lower-case hex |
| `hash_offset` | byte offset of the verity superblock in the hash device: the first hash-block boundary after the padded data when appended, `0` for a separate file |
| `data_blocks` | data blocks the tree covers |
| `data_block_size`, `hash_block_size` | the block sizes in bytes |

To open the tree, the hash device is `dest` when appended or `hash_dest` otherwise, and the root hash and hash offset are the recorded values: `veritysetup open <dest> <name> <hash device> <root_hash> --hash-offset=<hash_offset>`. The superblock records the block sizes, salt and UUID. Two runs over the same `src` give byte-identical output and the same root hash. The reproducibility rests on the salt and UUID being derived from the data, not on a random salt.

**`ext4`** (a filesystem) packs `source_tree` into an ext4 image with `mkfs.ext4 -d`, resolved like `veritysetup` (`mkfs.ext4` and `debugfs` from provider host tools, then `PATH`). Options, for `kind = "ext4"` only:
- `size` (required): a byte size with a binary suffix, or `"auto"`. The size must be a multiple of 4 KiB. Auto sizing counts each file and directory in whole 4 KiB blocks, the inode tables (two inodes per entry plus 64, 256 bytes each), 8 MiB of fixed overhead and the journal, plus a margin of 5% (at least 4 MiB), rounded up to a whole MiB.
- `label` (optional): at most 16 bytes, no `/`.
- `reserved_percent` (optional, 0-50): the percentage of blocks reserved for root, passed as `mkfs.ext4 -m`. Unset is 0, so the whole image is usable; mke2fs's own default of 5% is not used. Set it (for example to 5) for an image that needs the root reserve.
- `uuid` (optional): a UUID. Unset, it is derived from the filesystem `id`.
- `journal` (optional): `false` for no journal; the default is a journal of 1/64 of the image, between 4 and 128 MiB.

`mke2fs` copies the builder's owners and times into the image, so Gaia then sets every entry to root:root with timestamps of 0 (the epoch). File modes are kept. The fixed UUID, hash seed and time make two runs over the same tree byte-identical. Entry names containing `"`, `\` or a control character are rejected, because `debugfs` cannot set them. `publish = true` and the RAM work dir work as for the other filesystems, and `deterministic` must stay `true`.

##### Example: dynamic slot bundle

A read-only root slot: the EROFS root filesystem from the image provider (Buildroot's EROFS rootfs option, for example), a verity tree appended to it, and a bundle that carries the root's digest and the verity values the initramfs needs to open the tree:

```toml
[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/rootfs.verity.erofs"

[[image.assembly.archives]]
id = "slot"
output = "$provider.images/slot.update"

[[image.assembly.archives.generated]]
name = "manifest.env"
entries = [
  ["ROOTFS_SHA256", "${assembly.sha256:$provider.images/rootfs.erofs}"],
  ["VERITY_ROOT_HASH", "${assembly.verity:$assembly.work/rootfs.verity.erofs:root_hash}"],
  ["VERITY_SALT", "${assembly.verity:$assembly.work/rootfs.verity.erofs:salt}"],
  ["VERITY_HASH_OFFSET", "${assembly.verity:$assembly.work/rootfs.verity.erofs:hash_offset}"],
  ["VERITY_DATA_BLOCKS", "${assembly.verity:$assembly.work/rootfs.verity.erofs:data_blocks}"],
  ["VERITY_DATA_BLOCK_SIZE", "${assembly.verity:$assembly.work/rootfs.verity.erofs:data_block_size}"],
  ["VERITY_HASH_BLOCK_SIZE", "${assembly.verity:$assembly.work/rootfs.verity.erofs:hash_block_size}"],
]

[[image.assembly.archives.members]]
name = "rootfs.erofs"
src = "$assembly.work/rootfs.verity.erofs"
```

The bundle's `manifest.env` is the file the updater sources. The root slot is `rootfs.verity.erofs`; the initramfs opens it with the manifest's root hash and hash offset.

##### Example: a dynamic slot with its manifest, cmdline and tools

The same slot, with the root's verity values in a JSON manifest and in the kernel command line, a fragment that names the slot's trial state, and `e2fsck` copied into the initramfs with its libraries:

```toml
[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/slot/rootfs.erofs"

[[image.assembly.files]]
tree = "initramfs"
content = "roothash=${assembly.verity:$assembly.work/slot/rootfs.erofs:root_hash}\n"
dest = "cmdline.txt"

[[image.assembly.files]]
tree = "initramfs"
content = "trial=1\n"
dest = "default.txt"

[[image.assembly.files]]
tree = "initramfs"
dest = "state.json"
json = { schema = 1, id = "${input.slot_id}", version = "${build.version}", built = "${build.date}", source = "${project.describe}", image = { size = "${assembly.size:$assembly.work/slot/rootfs.erofs}", sha256 = "${assembly.sha256:$assembly.work/slot/rootfs.erofs}" }, verity = { root_hash = "${assembly.verity:$assembly.work/slot/rootfs.erofs:root_hash}", salt = "${assembly.verity:$assembly.work/slot/rootfs.erofs:salt}", hash_offset = "${assembly.verity:$assembly.work/slot/rootfs.erofs:hash_offset}", data_blocks = "${assembly.verity:$assembly.work/slot/rootfs.erofs:data_blocks}" } }

[[image.assembly.binaries]]
tree = "initramfs"
src = "$provider.buildroot_output/target/usr/sbin/e2fsck"
dest = "usr/sbin/e2fsck"
```

The `state.json` document is written in the order shown, and its `hash_offset` and `data_blocks` are JSON numbers.

##### Example: ext4 data partition

A data partition built from a tree of its own, sized from the tree and labelled, as the last partition of a disk:

```toml
[[image.assembly.filesystems]]
id = "data"
kind = "ext4"
source_tree = "data"
output = "$provider.images/data.ext4"
size = "auto"
label = "data"

[[image.assembly.disks.partitions]]
name = "data"
type_alias = "linux"
image = "$provider.images/data.ext4"
```

#### Example: Raspberry Pi CM5 eMMC A/B layout with an update bundle

A board example for the Raspberry Pi Compute Module 5: an eMMC image with a small autoboot partition, A/B boot and root slots, a data partition, and a `.update` bundle containing a manifest and zstd-compressed slot images:

| Partition | Content |
| --- | --- |
| p1 | 16M FAT holding only `autoboot.txt` |
| p2, p3 | boot A/B, 128M FAT each, both from `boot.vfat` |
| p4 | extended partition |
| p5, p6 | root A/B, 2G ext4 each; A from `rootfs.ext4`, B empty and wiped |
| p7 | `/data` ext4 from a small image |

```toml
build_name = "my-os"
version = "1.2.3"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"

[[image.assembly.trees]]
id = "autoboot"
path = "$assembly.work/autoboot"

[[image.assembly.files]]
tree = "autoboot"
src = "@assets/autoboot.txt"
dest = "autoboot.txt"

[[image.assembly.filesystems]]
id = "autoboot"
kind = "vfat"
source_tree = "autoboot"
output = "$provider.images/autoboot.vfat"
size = "16M"

[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/boot.vfat"
dest = "$assembly.work/boot.vfat.zst"

[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/rootfs.ext4"
dest = "$assembly.work/rootfs.ext4.zst"
level = 19

[[image.assembly.disks]]
id = "emmc"
output = "$provider.images/emmc.img"
partition_table = "mbr"

[[image.assembly.disks.partitions]]
name = "autoboot"
type_alias = "fat32-lba"
bootable = true
image = "$provider.images/autoboot.vfat"
size = "16M"

[[image.assembly.disks.partitions]]
name = "boot-a"
type_alias = "fat32-lba"
image = "$provider.images/boot.vfat"
size = "128M"

[[image.assembly.disks.partitions]]
name = "boot-b"
type_alias = "fat32-lba"
image = "$provider.images/boot.vfat"
size = "128M"

[[image.assembly.disks.partitions]]
name = "rootfs-a"
type_alias = "linux"
image = "$provider.images/rootfs.ext4"
size = "2G"

[[image.assembly.disks.partitions]]
name = "rootfs-b"
type_alias = "linux"
size = "2G"
wipe = true

[[image.assembly.disks.partitions]]
name = "data"
type_alias = "linux"
image = "$provider.images/data.ext4"

[[image.assembly.archives]]
id = "update"
output = "$provider.images/${build.name}-${build.version}.update"

[[image.assembly.archives.members]]
name = "manifest.env"
entries = [
  ["MODEL", "cm5"],
  ["VERSION", "${build.version}"],
  ["OS", "my-os"],
  ["BOOT_SHA256", "${assembly.sha256:$assembly.work/boot.vfat.zst}"],
  ["ROOTFS_SHA256", "${assembly.sha256:$assembly.work/rootfs.ext4.zst}"],
]

[[image.assembly.archives.members]]
name = "boot.vfat.zst"
src = "$assembly.work/boot.vfat.zst"

[[image.assembly.archives.members]]
name = "rootfs.ext4.zst"
src = "$assembly.work/rootfs.ext4.zst"
```

The bundle `my-os-1.2.3.update` is a tar of `manifest.env` (`MODEL`, `VERSION`, `OS`, `BOOT_SHA256`, `ROOTFS_SHA256`) followed by `boot.vfat.zst` and `rootfs.ext4.zst`, where each digest is the sha256 of the compressed member in the bundle.


Provider hooks and assembly:
- Keep provider-native post-image hooks for provider-specific behavior that must run inside the provider build environment.
- Prefer typed assembly for generic staging, compression, device-tree compilation, filesystem images, raw MBR disk layout, and BusyBox initramfs generation.
- During migration, provider hooks and typed assembly can coexist; assembly runs after provider image build and feed refresh so it can consume provider outputs.
- Gaia reports publish-directory hygiene warnings for transient directories and large unexpected files, but these warnings do not fail builds. Tune the policy with `[reporting.output_hygiene]`:

```toml
[reporting.output_hygiene]
large_file_threshold_bytes = 104857600
transient_dir_names = [".cache", "build", "buildroot-output", "sources", "target"]
```

Buildroot migration example:
- Old hook-oriented approach: set `BR2_ROOTFS_POST_IMAGE_SCRIPT="support/scripts/genimage.sh"` and `BR2_ROOTFS_POST_SCRIPT_ARGS="-c ${CONFIG_DIR}/genimage.cfg"` in the Buildroot defconfig to create `sdcard.img`.
- Typed assembly equivalent: leave Buildroot responsible for producing root filesystem artifacts, then declare an MBR disk in TOML:

```toml
[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "mbr"
signature_text = "GAIA"

[[image.assembly.disks.partitions]]
name = "rootfs"
type_alias = "linux"
image = "$provider.buildroot_output/images/rootfs.ext4"
```

#### Example: flasher boot image with an initramfs

A flasher image boots a kernel with an initramfs that loads USB gadget modules
and runs the flashing script. The initramfs is packed as `cpio-zstd` and
placed in a FAT boot image, which is published as an image of the build:

```toml
build_name = "flasher"
version = "1.0.0"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "flasher_defconfig"

[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

# BusyBox and the kernel modules the gadget needs, with their dependencies.
[[image.assembly.busybox_initramfs]]
tree = "initramfs"
busybox = "$provider.target/bin/busybox"
include_runtime_libs = false
applets = ["sh", "mount", "mkdir", "modprobe", "switch_root"]

[[image.assembly.kernel_modules]]
tree = "initramfs"
from = "$provider.target/lib/modules"
modules = ["libcomposite", "usb-f-mass-storage"]

[[image.assembly.files]]
tree = "initramfs"
src = "@assets/flasher/init"
dest = "init"
mode = "0755"

# The initramfs, compressed with zstd level 19 (the default).
[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-zstd"
source_tree = "initramfs"
output = "$assembly.work/initramfs.cpio.zst"
compression_level = 19

# The boot tree: kernel, device tree, config.txt and the initramfs.
[[image.assembly.files]]
tree = "boot"
src = "$provider.images/Image"
dest = "Image"

[[image.assembly.files]]
tree = "boot"
src = "$provider.images/bcm2711-rpi-4-b.dtb"
dest = "bcm2711-rpi-4-b.dtb"

[[image.assembly.files]]
tree = "boot"
src = "@assets/flasher/config.txt"
dest = "config.txt"

[[image.assembly.files]]
tree = "boot"
src = "$assembly.work/initramfs.cpio.zst"
dest = "initramfs.cpio.zst"

# boot.img, published to $provider.images/boot.img.
[[image.assembly.filesystems]]
id = "boot"
kind = "vfat"
source_tree = "boot"
output = "$assembly.work/boot.img"
size = "64M"
publish = true
```

The boot tree's `config.txt` carries the line `initramfs initramfs.cpio.zst followkernel`, which tells the firmware to load the initramfs next to the kernel. The boot tree reads the `cpio-zstd` output, so it runs after the initramfs is packed.

## Assembly checks

```toml
[[image.assembly.checks]]
id = "boot-overlays"
command = ["@self/scripts/check-boot-overlays.sh", "$assembly.tree.slots-fat"]
timeout_secs = 60          # default: the image provider's command timeout
```

A check is a program and its arguments, run without a shell from the workspace root after every assembly step (trees, files, filesystems, disks, archives, OCI images), so it sees the finished trees and outputs at the paths the steps wrote them, RAM work dirs included. Assembly tokens are expanded in every argument: `$assembly.tree.<id>`, `$assembly.work`, `$assembly.out`, `$provider.images`, `$provider.buildroot_output` and the rest, plus `@self` and `${...}` as in any config value. A non-zero exit fails the assembly, and so the build, with the command's stdout and stderr tails in the error; a passing check's output lines go to the operation log. Validation requires a unique `id`, a non-empty `command`, and trees that exist for every `$assembly.tree.<id>` an argument names. A check runs whenever the assembly does: when the assembly is reused, its checks are not run again (editing the script itself does not re-run the assembly).
