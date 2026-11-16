//! The dm-verity hash tree, computed in this process: kernel hash type 1
//! (salted), sha256, 4 KiB data and hash blocks, and no superblock. The
//! format is the one the kernel's dm-verity reads, and `veritysetup format
//! --no-superblock` writes; the tests hold this code to veritysetup's bytes.
//!
//! A data block's hash is sha256 over the salt, then the block. A hash block
//! holds 128 of those hashes (32 bytes each), zero-padded to 4 KiB. The
//! hashes of one level's blocks make the next level's hash blocks, until one
//! block is left: its hash is the root hash. A single data block has no
//! tree: its own hash is the root. In the output the top level comes first
//! and the bottom level (the one that hashes the data) last.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};

/// The data and hash block size.
pub const BLOCK: usize = 4096;
/// A sha256 digest: one hash in a hash block.
const HASH_BYTES: usize = 32;
/// Hashes per hash block.
const FANOUT: usize = BLOCK / HASH_BYTES;

/// The hash of one block: sha256 of the salt, then of the block.
fn block_hash(salt: &[u8], block: &[u8]) -> [u8; HASH_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(block);
    hasher.finalize().into()
}

/// Reads `blocks` data blocks from `data` (exactly `blocks * BLOCK` bytes),
/// writes their hash tree to `tree`, top level first, and returns the root
/// hash and the tree's length in bytes. Zero blocks is an error: there is
/// no root for no data.
pub fn hash_tree<R: Read, W: Write>(
    data: &mut R,
    blocks: u64,
    salt: &[u8],
    tree: &mut W,
) -> Result<([u8; HASH_BYTES], u64), String> {
    if blocks == 0 {
        return Err("no data blocks: a tree covers at least one".into());
    }
    let mut block = vec![0_u8; BLOCK];
    let mut hashes = Vec::with_capacity(usize::try_from(blocks).unwrap_or(usize::MAX));
    for n in 0..blocks {
        data.read_exact(&mut block)
            .map_err(|e| format!("data block {n} of {blocks}: {e}"))?;
        hashes.push(block_hash(salt, &block));
    }
    if blocks == 1 {
        // One block is its own root: no hash block is written.
        return Ok((hashes[0], 0));
    }
    // Each level's hash blocks, bottom level first, until one block is left.
    let mut levels: Vec<Vec<u8>> = Vec::new();
    let root = loop {
        let packed = pack(&hashes);
        let next: Vec<[u8; HASH_BYTES]> = packed
            .chunks(BLOCK)
            .map(|hash_block| block_hash(salt, hash_block))
            .collect();
        let single = next.len() == 1;
        levels.push(packed);
        if single {
            break next[0];
        }
        hashes = next;
    };
    let mut written = 0_u64;
    for level in levels.iter().rev() {
        tree.write_all(level)
            .map_err(|e| format!("write the hash tree: {e}"))?;
        written += level.len() as u64;
    }
    Ok((root, written))
}

/// The hashes packed into zero-padded hash blocks, one block per `FANOUT`
/// hashes. An empty list packs to nothing.
fn pack(hashes: &[[u8; HASH_BYTES]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hashes.len().div_ceil(FANOUT) * BLOCK);
    for group in hashes.chunks(FANOUT) {
        let start = out.len();
        for hash in group {
            out.extend_from_slice(hash);
        }
        out.resize(start + BLOCK, 0);
    }
    out
}

/// Lowercase hex of a root hash or salt.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes `veritysetup` was run on in the golden tests: block `i`
    /// comes from `image_bytes`, the same pattern the node tests use.
    fn pattern(len: usize, seed: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 7 + seed) % 251) as u8).collect()
    }

    fn tree_of(data: &[u8], salt: &[u8]) -> ([u8; 32], Vec<u8>) {
        let mut tree = Vec::new();
        let (root, len) = hash_tree(&mut &data[..], (data.len() / BLOCK) as u64, salt, &mut tree)
            .expect("hashes");
        assert_eq!(len, tree.len() as u64);
        (root, tree)
    }

    #[test]
    fn one_block_has_no_tree_and_its_root_is_its_own_hash() {
        let block = pattern(BLOCK, 3);
        let salt = [0xab_u8; 32];
        let (root, tree) = tree_of(&block, &salt);
        assert!(tree.is_empty());
        assert_eq!(root, block_hash(&salt, &block));
    }

    #[test]
    fn the_salt_comes_before_the_block() {
        // sha256 of the block alone is not the hash: the salt is first.
        let block = pattern(BLOCK, 3);
        let salt = [0xab_u8; 32];
        let plain: [u8; 32] = Sha256::digest(&block).into();
        let mut reversed = Sha256::new();
        reversed.update(&block);
        reversed.update(salt);
        let (root, _) = tree_of(&block, &salt);
        assert_ne!(root, plain);
        assert_ne!(root, <[u8; 32]>::from(reversed.finalize()));
        assert_eq!(root, block_hash(&salt, &block));
    }

    #[test]
    fn two_blocks_make_one_hash_block_padded_with_zeros() {
        let data = pattern(2 * BLOCK, 9);
        let salt = [0x11_u8; 32];
        let (root, tree) = tree_of(&data, &salt);
        assert_eq!(tree.len(), BLOCK);
        let h0 = block_hash(&salt, &data[..BLOCK]);
        let h1 = block_hash(&salt, &data[BLOCK..]);
        assert_eq!(&tree[..32], &h0);
        assert_eq!(&tree[32..64], &h1);
        assert!(tree[64..].iter().all(|byte| *byte == 0));
        assert_eq!(root, block_hash(&salt, &tree));
    }

    #[test]
    fn the_levels_grow_at_the_fanout_and_the_top_comes_first() {
        let salt = [0x22_u8; 32];
        // 128 blocks: one level. 129: two levels (two bottom blocks and one top).
        assert_eq!(tree_of(&pattern(128 * BLOCK, 1), &salt).1.len(), BLOCK);
        let (root, tree) = tree_of(&pattern(129 * BLOCK, 1), &salt);
        assert_eq!(tree.len(), 3 * BLOCK);
        // The first block in the output is the top: its hashes are the
        // root's children, and the root is the hash of that block.
        assert_eq!(root, block_hash(&salt, &tree[..BLOCK]));
        // The top block holds the two bottom blocks' hashes.
        // The output is top first: bottom block 0 is at BLOCK, bottom block 1 at 2 * BLOCK.
        let bottom0 = block_hash(&salt, &tree[BLOCK..2 * BLOCK]);
        let bottom1 = block_hash(&salt, &tree[2 * BLOCK..3 * BLOCK]);
        assert_eq!(&tree[..32], &bottom0);
        assert_eq!(&tree[32..64], &bottom1);
        // 16 385 blocks: three levels (129 bottom, 2 middle, 1 top).
        let (_, tree) = tree_of(&pattern(16_385 * BLOCK, 1), &salt);
        assert_eq!(tree.len(), (129 + 2 + 1) * BLOCK);
    }

    #[test]
    fn zero_blocks_is_an_error() {
        let mut tree = Vec::new();
        let error = hash_tree(&mut &[][..], 0, &[], &mut tree).expect_err("no data");
        assert!(error.contains("at least one"), "{error}");
    }

    #[test]
    fn short_data_is_an_error_not_a_short_tree() {
        let mut tree = Vec::new();
        let short = vec![0_u8; BLOCK + 10];
        let error = hash_tree(&mut &short[..], 2, &[], &mut tree).expect_err("short");
        assert!(error.contains("data block 1 of 2"), "{error}");
    }

    #[test]
    fn hex_is_lowercase_and_full_width() {
        assert_eq!(hex(&[0x0a, 0xff]), "0aff");
    }
}
