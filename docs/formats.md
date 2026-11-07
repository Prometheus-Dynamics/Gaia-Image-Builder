# On-media formats (`gaia-formats`)

`crates/core/formats` (package `gaia-formats`, library `gaia_formats`) holds
the formats Gaia writes and a device reads: UBI images
(`[[image.assembly.ubis]]`) and dm-verity hash trees (`verity` transforms).
Gaia's assembly writes with these types, and Atlas reads, flashes and
updates with the same ones, so the writer and the reader share one
definition. The crate depends on `std` and `sha2` only, and on no other
Gaia crate.

What a device does with the images (Atlas's slot contract: which volume is
which slot, how a slot is selected and committed) is Atlas's and is not in
the crate. The crate writes no report files.

## What is stable

| Part | Defined by | In the crate |
|---|---|---|
| UBI on-media format | the kernel, `drivers/mtd/ubi/ubi-media.h` | `ubi::{EcHeader, VidHeader, VtblRecord}`, `crc::ubi_crc` |
| UBI geometry | `ubinize` and the kernel | `ubi::Geometry::new`, `ubi::PebLayout` |
| UBI reserves | the kernel, `get_bad_peb_limit` | `ubi::{reserved_pebs, bad_peb_reserve, minimum_device_pebs}` |
| Gaia's UBI image layout | Gaia | `ubi::{encode_peb, encode_volume_table, read_image}` |
| veritysetup superblock, version 1 | cryptsetup, `lib/verity/verity.c` | `verity::Superblock` |
| appended verity tree layout | Gaia | `verity::{padded_len, appended_hash_offset, hash_start_block, data_sectors}` |
| default salt and UUID | Gaia | `verity::Derived`, `verity::derived_uuid` |

### UBI

- Every header field is big-endian. Every CRC is `ubi_crc`: the standard
  CRC-32 without its final inversion (`ubi_crc(b"123456789") == 0x340bc6d9`).
- EC header (64 bytes, offset 0 of every PEB): magic `UBI#`, version 1, the
  erase counter (u64 at 8), the VID header offset (16), the data offset
  (20), the image sequence number (24), the CRC of the first 60 bytes (60).
  Gaia writes erase counter 0, as `ubinize` does.
- VID header (64 bytes, at the VID header offset): magic `UBI!`, version 1,
  volume type (5: 1 dynamic, 2 static), copy flag (6), compat (7), volume id
  (8), LEB number (12), data size (20), used LEBs (24), data pad (28), data
  CRC (32), sequence number (u64 at 40), CRC (60). Gaia writes copy flag,
  data pad and sequence number 0; data size, used LEBs and data CRC only for
  static volumes.
- Volume-table record (172 bytes): reserved PEBs (0), alignment (4, Gaia
  writes 1), data pad (8), type (12), update marker (13), name length (u16
  at 14), name (16..144), flags (144, `0x01` autoresize), CRC of the first
  168 bytes (168). An empty record is zeros and its CRC, `0xf116c36b`. A
  record that names a volume reserves at least one PEB.
- Geometry: the PEB size and page size are powers of two, a PEB holds at
  least two pages, the VID header sits at the sub-page size (the page size
  without sub-pages) unless set, and data starts at the next page after the
  VID header. The volume table has one record per possible volume id, as
  many as fit a LEB, at most 128.
- Gaia's layout: the layout volume (id `0x7fffefff`, compat 5) in PEBs 0
  and 1, each holding the whole volume table, then each volume's data LEBs
  in volume order, one PEB per LEB, erased bytes after the data. Volumes
  without data and LEBs reserved past the data take no PEBs in the image.
- Reserves: an attached image needs the layout volume's 2 PEBs, UBI's 2
  working PEBs and every volume's reserved PEBs, plus the bad-block reserve:
  `per_1024` of every 1024 PEBs of the whole device, rounded up (Linux's
  default 20). With `per_1024` 0 the kernel takes its figure from its
  config or the MTD device, which the crate cannot know, so it counts 1.

### dm-verity

- The superblock is 512 bytes at the hash offset, its integers
  little-endian: `verity\0\0`, version 1, hash type (Gaia: 1), the UUID's 16
  bytes, the algorithm NUL-padded to 32 bytes (Gaia: `sha256`), data block
  size, hash block size, data blocks (u64), salt size (u16), the salt
  (zero-padded to 256 bytes). `verity::check` compares one against what Gaia
  recorded and returns every field that differs.
- Appended layout: the data zero-padded to whole data blocks, the
  superblock at the next hash-block boundary (the hash offset), the tree's
  first block one hash block after it (`hash_start_block`). A tree in its
  own file has hash offset 0. Gaia's defaults are 4096-byte data and hash
  blocks.
- Default salt and UUID, from the data image's sha256 (lowercase hex):
  the salt is that hex (32 bytes). The UUID is the first 16 bytes of
  `sha256("gaia dm-verity uuid\n" + hex)`, with byte 6 set to version 4
  (`& 0x0f | 0x40`) and byte 8 to the RFC 4122 variant (`& 0x3f | 0x80`),
  printed 8-4-4-4-12 lowercase. A transform's own `salt` or `uuid` replaces
  the default. The same derivation names ext4 and EROFS filesystems, each
  with its own domain string.

## Versioning

The crate versions with Gaia (`X.Y.Z-dev` on dev). A change to any format,
default or derivation in it changes images on the media, so it needs a
CHANGELOG line that says what changes, for Atlas to follow.
