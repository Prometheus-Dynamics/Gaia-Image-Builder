//! The PEBs an attached image needs: the layout volume, UBI's own working
//! PEBs, every volume's reservation and the bad-block reserve.

use super::{LAYOUT_VOLUME_LEBS, RUNTIME_RESERVED_PEBS};

/// The PEBs an attached image needs before bad-block reserves: the layout
/// volume, UBI's two working PEBs and each volume's reserved LEBs.
pub fn reserved_pebs(volume_reserved_lebs: impl IntoIterator<Item = u32>) -> u64 {
    u64::from(LAYOUT_VOLUME_LEBS)
        + RUNTIME_RESERVED_PEBS
        + volume_reserved_lebs.into_iter().map(u64::from).sum::<u64>()
}

/// The bad-block reserve UBI keeps on a device of `device_pebs` PEBs
/// (`get_bad_peb_limit`): `per_1024` per 1024 PEBs, rounded up. With
/// `per_1024` 0 the kernel takes its figure from its config or the MTD
/// device, which this cannot know; this counts 1 PEB then.
pub fn bad_peb_reserve(device_pebs: u64, per_1024: u64) -> u64 {
    let limit = device_pebs * per_1024 / 1024;
    if limit * 1024 / per_1024.max(1) < device_pebs {
        limit + 1
    } else {
        limit
    }
}

/// The fewest PEBs a flash device needs to attach an image that reserves
/// `reserved_pebs`, counting the bad-block reserve that grows with it.
pub fn minimum_device_pebs(reserved_pebs: u64, per_1024: u64) -> u64 {
    let mut pebs = reserved_pebs;
    while pebs < reserved_pebs + bad_peb_reserve(pebs, per_1024) {
        pebs += 1;
    }
    pebs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smallest_device_covers_the_bad_block_reserve() {
        assert_eq!(bad_peb_reserve(1024, 20), 20);
        assert_eq!(bad_peb_reserve(1000, 20), 20);
        assert_eq!(bad_peb_reserve(1025, 20), 21);
        assert_eq!(bad_peb_reserve(2048, 20), 40);
        assert_eq!(bad_peb_reserve(2048, 0), 1);
        let pebs = minimum_device_pebs(500, 20);
        assert_eq!(pebs, 510);
        assert!(pebs >= 500 + bad_peb_reserve(pebs, 20));
        assert!(pebs - 1 < 500 + bad_peb_reserve(pebs - 1, 20));
        assert_eq!(reserved_pebs([4, 4, 1]), 2 + 2 + 9);
    }
}
