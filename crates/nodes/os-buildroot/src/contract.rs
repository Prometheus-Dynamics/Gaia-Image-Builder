//! A kernel contract: the options a boot path needs built into the kernel,
//! read from a file its owner keeps, and checked against the kernel's final
//! configuration before the kernel compiles.
//!
//! A root that an initramfs opens without loading a module needs the whole
//! chain to it in the kernel itself: an option at `m` builds and boots to
//! nothing. The list of that chain belongs to the program that opens the
//! root, so it is read from its source at the locked commit and no copy is
//! kept here: a later commit that adds an option tightens the build.
//!
//! The file, TOML, `version = 2`, names written without `CONFIG_`:
//!
//! ```toml
//! version = 2
//! [always]
//! all = ["BLK_DEV_DM"]                       # every image
//! [kind.disk]
//! all = ["EXT4_FS"]                          # images of this kind
//! one_of = [["MMC_BLOCK", "BLK_DEV_NVME"]]   # at least one of each list
//! [firmware.rpi-tryboot]
//! all = ["BCM_VCIO"]                         # boards of this boot firmware
//! [optional]
//! all = ["VFAT_FS"]                          # named, not enforced
//! [initramfs]
//! gzip = "RD_GZIP"                           # by the initramfs's compression
//! [boot_check]
//! checked = ["BLK_DEV_DM"]                   # what the program checks itself
//! ```
//!
//! Another version, or a key this reader does not know at any level, is
//! refused: it means a newer file, and guessing what it asks for would pass
//! a kernel that does not meet it.

use gaia_types::KernelContract;
use serde::Deserialize;
use std::collections::BTreeMap;

/// The version this reader knows.
const VERSION: i64 = 2;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[allow(dead_code)]
    version: i64,
    always: All,
    kind: BTreeMap<String, Kind>,
    firmware: BTreeMap<String, All>,
    #[allow(dead_code)]
    optional: All,
    #[allow(dead_code)]
    initramfs: BTreeMap<String, String>,
    #[allow(dead_code)]
    boot_check: BootCheck,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct All {
    all: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kind {
    all: Vec<String>,
    one_of: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BootCheck {
    #[allow(dead_code)]
    checked: Vec<String>,
}

/// What the kernel's final `.config` (`config`) lacks of the contract file
/// `text` as `want` reads it, one line each; empty when it is met.
///
/// # Errors
///
/// A file that is not the version this reader knows, has a key it does not
/// know, or has no group for the image's kind.
pub fn problems(text: &str, config: &str, want: &KernelContract) -> Result<Vec<String>, String> {
    let file = &want.path;
    let value: toml::Value =
        toml::from_str(text).map_err(|e| format!("{file}: {}", e.message()))?;
    match value.get("version").and_then(toml::Value::as_integer) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(format!(
                "{file}: version {other}; this build reads version {VERSION}"
            ));
        }
        None => return Err(format!("{file}: no `version`")),
    }
    let contract: File = value
        .try_into()
        .map_err(|e: toml::de::Error| format!("{file}: {}", e.message()))?;
    let kind = contract.kind.get(&want.kind).ok_or_else(|| {
        let known: Vec<&str> = contract.kind.keys().map(String::as_str).collect();
        format!(
            "{file}: no `kind.{}`; its kinds are {}",
            want.kind,
            known.join(", ")
        )
    })?;

    let state = |symbol: &str| -> Option<&str> {
        let name = if symbol.starts_with("CONFIG_") {
            symbol.to_string()
        } else {
            format!("CONFIG_{symbol}")
        };
        config.lines().find_map(|line| {
            line.trim()
                .strip_prefix(name.as_str())?
                .strip_prefix('=')
                .map(str::trim)
        })
    };
    let shown = |symbol: &str| match state(symbol) {
        Some(value) => format!("={value}"),
        None => " is not set".to_string(),
    };
    let built_in = |symbol: &str| state(symbol) == Some("y");
    let name = |symbol: &str| {
        if symbol.starts_with("CONFIG_") {
            symbol.to_string()
        } else {
            format!("CONFIG_{symbol}")
        }
    };

    // The boot firmware's own group, for a board that has one.
    let none = Vec::new();
    let firmware = if want.firmware.is_empty() {
        &none
    } else {
        &contract
            .firmware
            .get(&want.firmware)
            .ok_or_else(|| {
                let known: Vec<&str> = contract.firmware.keys().map(String::as_str).collect();
                format!(
                    "{file}: no `firmware.{}`; its boot firmwares are {}",
                    want.firmware,
                    known.join(", ")
                )
            })?
            .all
    };
    let mut missing = Vec::new();
    let groups = [
        ("always.all".to_string(), &contract.always.all),
        (format!("kind.{}.all", want.kind), &kind.all),
        (format!("firmware.{}.all", want.firmware), firmware),
    ];
    for (group, symbols) in &groups {
        for symbol in symbols.iter().filter(|symbol| !built_in(symbol)) {
            missing.push(format!(
                "{}{}, and must be =y ({file}: {group})",
                name(symbol),
                shown(symbol)
            ));
        }
    }
    for list in &kind.one_of {
        if !list.iter().any(|symbol| built_in(symbol)) {
            let all: Vec<String> = list
                .iter()
                .map(|symbol| format!("{}{}", name(symbol), shown(symbol)))
                .collect();
            missing.push(format!(
                "one of these must be =y: {} ({file}: kind.{}.one_of)",
                all.join(", "),
                want.kind
            ));
        }
    }
    if want.storage_drivers.is_empty() {
        missing.push(format!(
            "the board {} names no storage drivers (`[kernel] storage_drivers`), so the storage it boots from cannot be checked",
            want.board
        ));
    }
    for symbol in want
        .storage_drivers
        .iter()
        .filter(|symbol| !built_in(symbol))
    {
        missing.push(format!(
            "{}{}, and must be =y (the board {}'s storage_drivers)",
            name(symbol),
            shown(symbol),
            want.board
        ));
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTRACT: &str = r#"
version = 2
[always]
all = ["BLK_DEV_DM", "DM_VERITY"]
[kind.disk]
all = ["EXT4_FS"]
one_of = [["MMC_BLOCK", "BLK_DEV_NVME"]]
[kind.mtd]
all = ["MTD_UBI"]
one_of = []
[firmware.rom-a]
all = ["MAILBOX", "ROM_A_FW"]
[optional]
all = ["VFAT_FS"]
[initramfs]
gzip = "RD_GZIP"
none = ""
[boot_check]
checked = ["BLK_DEV_DM"]
"#;

    fn want(kind: &str, storage: &[&str]) -> KernelContract {
        KernelContract {
            input: "contract.init".into(),
            path: "contracts/slot-kernel.toml".into(),
            asked_by: "init".into(),
            board: "demo".into(),
            kind: kind.into(),
            firmware: String::new(),
            storage_drivers: storage.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_kernel_with_the_chain_built_in_meets_it() {
        let config = "CONFIG_BLK_DEV_DM=y\nCONFIG_DM_VERITY=y\nCONFIG_EXT4_FS=y\nCONFIG_MMC_BLOCK=y\nCONFIG_MMC_HOST=y\n";
        assert_eq!(
            problems(CONTRACT, config, &want("disk", &["CONFIG_MMC_HOST"])),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_module_an_unset_option_and_a_missing_alternative_are_each_named() {
        let config = "CONFIG_BLK_DEV_DM=m\n# CONFIG_DM_VERITY is not set\nCONFIG_EXT4_FS=y\nCONFIG_MMC_BLOCK=m\nCONFIG_MMC_HOST=m\n";
        let found = problems(CONTRACT, config, &want("disk", &["CONFIG_MMC_HOST"])).expect("read");
        assert_eq!(
            found,
            [
                "CONFIG_BLK_DEV_DM=m, and must be =y (contracts/slot-kernel.toml: always.all)",
                "CONFIG_DM_VERITY is not set, and must be =y (contracts/slot-kernel.toml: always.all)",
                "one of these must be =y: CONFIG_MMC_BLOCK=m, CONFIG_BLK_DEV_NVME is not set (contracts/slot-kernel.toml: kind.disk.one_of)",
                "CONFIG_MMC_HOST=m, and must be =y (the board demo's storage_drivers)",
            ]
        );
    }

    #[test]
    fn the_kind_picks_the_group_and_a_board_without_storage_drivers_is_named() {
        // An mtd image needs neither ext4 nor a block storage driver.
        let config = "CONFIG_BLK_DEV_DM=y\nCONFIG_DM_VERITY=y\nCONFIG_MTD_UBI=y\nCONFIG_NAND=y\n";
        assert_eq!(
            problems(CONTRACT, config, &want("mtd", &["CONFIG_NAND"])),
            Ok(Vec::new())
        );
        let found = problems(CONTRACT, config, &want("mtd", &[])).expect("read");
        assert_eq!(found.len(), 1);
        assert!(
            found[0].contains("the board demo names no storage drivers"),
            "{found:?}"
        );
        let err = problems(CONTRACT, config, &want("tape", &["CONFIG_NAND"])).expect_err("kind");
        assert_eq!(
            err,
            "contracts/slot-kernel.toml: no `kind.tape`; its kinds are disk, mtd"
        );
    }

    #[test]
    fn another_version_or_an_unknown_key_is_refused() {
        let w = want("disk", &["CONFIG_X"]);
        let newer = CONTRACT.replace("version = 2", "version = 3");
        assert_eq!(
            problems(&newer, "", &w),
            Err("contracts/slot-kernel.toml: version 3; this build reads version 2".into())
        );
        let none = CONTRACT.replace("version = 2", "");
        assert_eq!(
            problems(&none, "", &w),
            Err("contracts/slot-kernel.toml: no `version`".into())
        );
        // A key at the top, and one inside a group.
        for extra in [
            CONTRACT.replace("[always]", "[later]\nall = []\n[always]"),
            CONTRACT.replace("one_of = []", "one_of = []\nnone_of = []"),
        ] {
            let err = problems(&extra, "", &w).expect_err("unknown key");
            assert!(err.contains("unknown field"), "{err}");
        }
        // A group that is missing is refused too: every key is required.
        let short = CONTRACT.replace("[optional]\nall = [\"VFAT_FS\"]\n", "");
        let err = problems(&short, "", &w).expect_err("missing key");
        assert!(err.contains("missing field `optional`"), "{err}");
    }

    #[test]
    fn a_boards_boot_firmware_adds_its_own_group() {
        let config = "CONFIG_BLK_DEV_DM=y\nCONFIG_DM_VERITY=y\nCONFIG_EXT4_FS=y\nCONFIG_MMC_BLOCK=y\nCONFIG_MMC_HOST=y\nCONFIG_MAILBOX=y\nCONFIG_ROM_A_FW=m\n";
        // A board with no boot firmware of its own: the group is not read.
        assert_eq!(
            problems(CONTRACT, config, &want("disk", &["CONFIG_MMC_HOST"])),
            Ok(Vec::new())
        );
        let mut with = want("disk", &["CONFIG_MMC_HOST"]);
        with.firmware = "rom-a".into();
        assert_eq!(
            problems(CONTRACT, config, &with),
            Ok(vec![
                "CONFIG_ROM_A_FW=m, and must be =y (contracts/slot-kernel.toml: firmware.rom-a.all)"
                    .to_string()
            ])
        );
        with.firmware = "rom-b".into();
        assert_eq!(
            problems(CONTRACT, config, &with),
            Err(
                "contracts/slot-kernel.toml: no `firmware.rom-b`; its boot firmwares are rom-a"
                    .into()
            )
        );
    }
}
