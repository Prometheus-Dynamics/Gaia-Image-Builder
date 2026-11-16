//! Where a built U-Boot keeps its environment on this disk, checked before
//! the disk is written: a `saveenv` must never write over the partition
//! table, the bootloader areas or another partition.
//!
//! The places are read from U-Boot's own final configuration (its `.config`)
//! as its `env/mmc.c` computes them. With `CONFIG_PARTITION_TYPE_GUID`, the
//! copies go at the end of the partition whose type is U-Boot's environment
//! GUID, the first copy last; a partition too small for them is skipped. When
//! no such partition is found (another card, a damaged table) U-Boot falls
//! back to the raw offsets `CONFIG_ENV_OFFSET` and, with a redundant
//! environment, `CONFIG_ENV_OFFSET_REDUND`, unless `CONFIG_ENV_MMC_USE_DT`
//! turns the fallback off. A raw offset is checked wherever it could land:
//! it may only fall inside an environment partition, or on an area outside
//! the table that starts there and is the environment's size: the
//! environment the disk ships.
//!
//! An environment that is not on MMC, or a configuration that is not U-Boot's,
//! is not checked.

use crate::TableScheme;
use crate::layout::{GPT_TAIL_BYTES, Layout, PartitionInput};
use crate::table::{GPT_HEAD_SECTORS, SECTOR};
use std::collections::HashMap;

/// U-Boot's environment partition type GUID (`PARTITION_U_BOOT_ENVIRONMENT`).
pub const ENV_TYPE_GUID: &str = "3de21764-95bd-54bd-a5c3-4abe786f38a8";

/// The `CONFIG_` options of a Kconfig `.config`, by name without the prefix:
/// `y`, a number or a string with its quotes.
fn options(config: &str) -> HashMap<&str, &str> {
    config
        .lines()
        .filter_map(|line| line.strip_prefix("CONFIG_")?.split_once('='))
        .collect()
}

fn number(value: &str) -> Option<u64> {
    match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    }
}

/// Every way U-Boot's environment, as `config` places it, would write over
/// something on the disk `layout` lays out with `partitions` (in the
/// layout's order) and the blobs named `blob_names`. Empty when nothing is
/// in its way, or the environment is not on MMC.
pub fn problems(
    config: &str,
    layout: &Layout,
    partitions: &[PartitionInput],
    blob_names: &[String],
) -> Vec<String> {
    let set = options(config);
    if set.get("ENV_IS_IN_MMC") != Some(&"y") {
        return Vec::new();
    }
    let mut found = Vec::new();
    let Some(size) = set.get("ENV_SIZE").and_then(|v| number(v)) else {
        return vec![
            "U-Boot keeps its environment on MMC, and its config states no CONFIG_ENV_SIZE".into(),
        ];
    };
    let redundant = set.get("ENV_REDUNDANT") == Some(&"y");
    let copies: u64 = if redundant { 2 } else { 1 };
    let len = size.div_ceil(SECTOR) * SECTOR;

    let env_parts: Vec<(usize, u64, u64)> = partitions
        .iter()
        .zip(&layout.partitions)
        .enumerate()
        .filter(|(_, (p, _))| {
            layout.scheme == TableScheme::Gpt && p.table_type.eq_ignore_ascii_case(ENV_TYPE_GUID)
        })
        .map(|(i, (_, place))| (i, place.start, place.size))
        .collect();
    if set.get("PARTITION_TYPE_GUID") == Some(&"y") {
        // One partition holds every copy; with a second one, each holds one.
        let each = if env_parts.len() >= 2 { 1 } else { copies };
        for (i, _, part_size) in &env_parts {
            if each * len > *part_size {
                found.push(format!(
                    "partition '{}' is {part_size} bytes; U-Boot keeps {each} environment cop{} of {len} bytes in it, so it skips the partition and falls back to its raw offsets",
                    partitions[*i].name,
                    if each == 1 { "y" } else { "ies" }
                ));
            }
        }
    }

    if set.get("ENV_MMC_USE_DT") == Some(&"y") {
        return found;
    }
    let mut raw = vec![("CONFIG_ENV_OFFSET", "ENV_OFFSET", "ENV_OFFSET_RELATIVE_END")];
    if redundant {
        raw.push((
            "CONFIG_ENV_OFFSET_REDUND",
            "ENV_OFFSET_REDUND",
            "ENV_OFFSET_REDUND_RELATIVE_END",
        ));
    }
    let head = match layout.scheme {
        TableScheme::Gpt => GPT_HEAD_SECTORS * SECTOR,
        TableScheme::Mbr => SECTOR,
    };
    let hint = "set CONFIG_ENV_MMC_USE_DT=y to turn the fallback off, or move the offset into the environment partition";
    for (label, name, relative) in raw {
        let Some(offset) = set.get(name).and_then(|v| number(v)) else {
            found.push(format!(
                "U-Boot falls back to {label} for its environment, and its config states none"
            ));
            continue;
        };
        if set.get(relative) == Some(&"y") {
            // From the end of the card, where a GPT keeps its backup table.
            let tail = match layout.scheme {
                TableScheme::Gpt => GPT_TAIL_BYTES,
                TableScheme::Mbr => 0,
            };
            if offset < len + tail {
                found.push(format!(
                    "{label} {offset:#x} from the card's end puts {len} bytes of environment over the backup partition table or past the end; {hint}"
                ));
            }
            continue;
        }
        let end = offset.saturating_add(len);
        let inside_env = env_parts
            .iter()
            .any(|(_, start, part_size)| offset >= *start && end <= start + part_size);
        if inside_env {
            continue;
        }
        if offset < head {
            found.push(format!(
                "{label} {offset:#x} puts {len} bytes of environment over the partition table (the first {head} bytes); {hint}"
            ));
            continue;
        }
        for (name, place) in blob_names.iter().zip(&layout.blobs) {
            // An area that starts at the offset and is the environment's own
            // size is the environment as the disk ships it, not data under it.
            if place.start == offset && place.size == size {
                continue;
            }
            if offset < place.start + place.size && place.start < end {
                found.push(format!(
                    "{label} {offset:#x} puts {len} bytes of environment over the area '{name}' at {:#x}; {hint}",
                    place.start
                ));
            }
        }
        for (partition, place) in partitions.iter().zip(&layout.partitions) {
            if offset < place.start + place.size && place.start < end {
                found.push(format!(
                    "{label} {offset:#x} puts {len} bytes of environment over partition '{}' at {:#x}; {hint}",
                    partition.name, place.start
                ));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{BlobInput, layout};
    use gaia_types::PartitionRole;

    const K: u64 = 1024;
    const M: u64 = K * K;

    fn env_part(size: u64) -> PartitionInput {
        PartitionInput {
            name: "uboot-env".into(),
            role: PartitionRole::Raw,
            table_type: ENV_TYPE_GUID.to_uppercase(),
            bootable: false,
            offset: Some(16 * M),
            size: Some(size),
            content_len: None,
        }
    }

    fn root() -> PartitionInput {
        PartitionInput {
            name: "rootfs".into(),
            role: PartitionRole::Rootfs,
            table_type: "0fc63daf-8483-4772-8e79-3d69d8477de4".into(),
            bootable: true,
            offset: None,
            size: Some(64 * M),
            content_len: None,
        }
    }

    /// The family's disk: the loader from 32K, the environment partition at
    /// 16M of `env_size`, a root after it.
    fn check(config: &str, env_size: u64) -> Vec<String> {
        let parts = [env_part(env_size), root()];
        let blobs = [BlobInput {
            name: "u-boot".into(),
            offset: 32 * K,
            len: 10 * M,
        }];
        let placed =
            layout(TableScheme::Gpt, Some(16 * M), None, None, &parts, &blobs).expect("lays out");
        problems(config, &placed, &parts, &["u-boot".into()])
    }

    const MMC: &str =
        "CONFIG_ENV_IS_IN_MMC=y\nCONFIG_ENV_SIZE=0x10000\nCONFIG_PARTITION_TYPE_GUID=y\n";

    #[test]
    fn the_fixed_family_config_passes() {
        let config = format!(
            "{MMC}CONFIG_ENV_REDUNDANT=y\nCONFIG_ENV_MMC_USE_DT=y\nCONFIG_ENV_OFFSET=0x3f8000\nCONFIG_ENV_OFFSET_REDUND=0x0\n"
        );
        assert_eq!(check(&config, 128 * K), Vec::<String>::new());
    }

    #[test]
    fn the_old_fallbacks_over_the_loader_and_the_table_are_refused() {
        let config = format!(
            "{MMC}CONFIG_ENV_REDUNDANT=y\nCONFIG_ENV_OFFSET=0x3f8000\nCONFIG_ENV_OFFSET_REDUND=0x0\n"
        );
        let found = check(&config, 128 * K);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[0].contains("CONFIG_ENV_OFFSET 0x3f8000") && found[0].contains("area 'u-boot'"),
            "{found:?}"
        );
        assert!(
            found[1].contains("CONFIG_ENV_OFFSET_REDUND 0x0")
                && found[1].contains("partition table"),
            "{found:?}"
        );
    }

    #[test]
    fn a_partition_too_small_for_both_copies_is_refused() {
        let config = format!("{MMC}CONFIG_ENV_REDUNDANT=y\nCONFIG_ENV_MMC_USE_DT=y\n");
        let found = check(&config, 64 * K);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("2 environment copies"), "{found:?}");
        // One copy fits.
        let single = format!("{MMC}CONFIG_ENV_MMC_USE_DT=y\n");
        assert!(check(&single, 64 * K).is_empty());
    }

    #[test]
    fn raw_offsets_inside_the_partition_pass_and_over_a_partition_are_refused() {
        let inside = format!(
            "{MMC}CONFIG_ENV_REDUNDANT=y\nCONFIG_ENV_OFFSET=0x1010000\nCONFIG_ENV_OFFSET_REDUND=0x1000000\n"
        );
        assert!(check(&inside, 128 * K).is_empty());
        // 32K before the root (at 17M): its second half runs into it.
        let over = format!("{MMC}CONFIG_ENV_OFFSET=0x10f8000\n");
        let found = check(&over, 64 * K);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("partition 'rootfs'"), "{found:?}");
        // The redundant offset is only read for a redundant environment.
        let single = format!("{MMC}CONFIG_ENV_OFFSET=0x1000000\nCONFIG_ENV_OFFSET_REDUND=0x0\n");
        assert!(check(&single, 64 * K).is_empty());
    }

    /// A disk that ships its environment as areas outside the table: a raw
    /// offset on such an area passes; one byte off, or over a longer area,
    /// is refused, and so is a copy with no area of its own over a loader.
    #[test]
    fn a_raw_offset_on_the_shipped_environment_area_passes() {
        let parts = [root()];
        let names: Vec<String> = ["spl", "env", "env-redund"].map(String::from).into();
        let on = |first: u64, second: u64, env_len: u64, config: &str| {
            let blobs = [
                BlobInput {
                    name: "spl".into(),
                    offset: M,
                    len: 300 * K,
                },
                BlobInput {
                    name: "env".into(),
                    offset: first,
                    len: env_len,
                },
                BlobInput {
                    name: "env-redund".into(),
                    offset: second,
                    len: env_len,
                },
            ];
            let placed = layout(TableScheme::Gpt, Some(4 * M), None, None, &parts, &blobs)
                .expect("lays out");
            problems(config, &placed, &parts, &names)
        };
        let config = "CONFIG_ENV_IS_IN_MMC=y\nCONFIG_ENV_SIZE=0x2000\nCONFIG_ENV_REDUNDANT=y\nCONFIG_ENV_OFFSET=0x300000\nCONFIG_ENV_OFFSET_REDUND=0x320000\n";
        assert_eq!(on(3 * M, 3200 * K, 8 * K, config), Vec::<String>::new());
        // The areas are elsewhere: the offsets land on nothing, which is free.
        assert!(on(2 * M, 2200 * K, 8 * K, config).is_empty());
        // An area there of another size is something else's data.
        let found = on(3 * M, 3200 * K, 16 * K, config);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("area 'env'"), "{found:?}");
        // An area that starts before the offset is written into.
        let found = on(3 * M - 4 * K, 3200 * K, 8 * K, config);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("area 'env' at 0x2ff000"), "{found:?}");
        // The loader is no environment, whatever the offset.
        let over = "CONFIG_ENV_IS_IN_MMC=y\nCONFIG_ENV_SIZE=0x2000\nCONFIG_ENV_OFFSET=0x100000\n";
        let found = on(3 * M, 3200 * K, 8 * K, over);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("area 'spl'"), "{found:?}");
    }

    #[test]
    fn an_offset_from_the_end_clear_of_the_backup_table_passes() {
        let near = format!(
            "{MMC}CONFIG_ENV_MMC_USE_DT=n\nCONFIG_ENV_OFFSET=0x10000\nCONFIG_ENV_OFFSET_RELATIVE_END=y\n"
        );
        assert_eq!(check(&near, 128 * K).len(), 1);
        let clear = format!("{MMC}CONFIG_ENV_OFFSET=0x20000\nCONFIG_ENV_OFFSET_RELATIVE_END=y\n");
        assert!(check(&clear, 128 * K).is_empty());
    }

    #[test]
    fn an_environment_off_mmc_is_not_checked() {
        assert!(check("CONFIG_ENV_IS_IN_FAT=y\nCONFIG_ENV_OFFSET=0x0\n", 64 * K).is_empty());
        assert_eq!(check("CONFIG_ENV_IS_IN_MMC=y\n", 64 * K).len(), 1);
    }
}
