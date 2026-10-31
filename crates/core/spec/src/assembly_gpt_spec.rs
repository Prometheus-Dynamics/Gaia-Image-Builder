//! GPT options of an assembly disk and its partitions: the disk and
//! partition GUIDs, and partition type GUIDs by alias or as raw GUIDs.

use crate::AssemblyDiskPartitionSpec;

/// GPT options of a disk. Unset for MBR disks.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyGptDiskSpec {
    /// The disk GUID; `None` derives it from the disk id.
    pub disk_guid: Option<String>,
}

/// GPT options of a partition. Unset for MBR disks.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyGptPartitionSpec {
    /// The unique partition GUID (the `PARTUUID`); `None` derives it from
    /// the disk GUID and the partition name.
    pub guid: Option<String>,
}

/// A GUID as written, `8-4-4-4-12` hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct GptGuid([u8; 16]);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GptGuidParseError(pub String);

impl std::fmt::Display for GptGuidParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "'{}' must be a GUID of 8-4-4-4-12 hex digits",
            self.0
        )
    }
}

impl GptGuid {
    pub fn parse(raw: &str) -> Result<Self, GptGuidParseError> {
        let invalid = || GptGuidParseError(raw.to_string());
        let text = raw.trim();
        let groups: Vec<&str> = text.split('-').collect();
        if groups.iter().map(|group| group.len()).collect::<Vec<_>>() != [8, 4, 4, 4, 12] {
            return Err(invalid());
        }
        let hex: String = groups.concat();
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte =
                u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
        }
        Ok(Self(bytes))
    }

    /// The GUID as GPT stores it: the first three groups little-endian, the
    /// last two as written.
    pub fn to_disk_bytes(self) -> [u8; 16] {
        let b = self.0;
        [
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13],
            b[14], b[15],
        ]
    }

    /// A version 4 (random) GUID from the first 16 bytes of a digest, so a
    /// derived GUID looks like any other to the tools that read it.
    pub fn from_digest(digest: &[u8]) -> Self {
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(bytes)
    }
}

impl std::fmt::Display for GptGuid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                formatter.write_str("-")?;
            }
            write!(formatter, "{byte:02X}")?;
        }
        Ok(())
    }
}

/// Partition type GUIDs by alias: the common ones and the Discoverable
/// Partitions Specification's root, `/usr` and verity types.
pub(crate) struct GptPartitionType;

impl GptPartitionType {
    pub(crate) const ALIASES: &'static [(&'static str, &'static str)] = &[
        ("linux", "0FC63DAF-8483-4772-8E79-3D69D8477DE4"),
        ("esp", "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"),
        ("bios-boot", "21686148-6449-6E6F-744E-656564454649"),
        (
            "microsoft-basic-data",
            "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7",
        ),
        ("xbootldr", "BC13C2FF-59E6-4262-A352-B275FD6F7172"),
        ("swap", "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F"),
        ("home", "933AC7E1-2EB4-4F13-B844-0E14E2AEF915"),
        ("srv", "3B8F8425-20E0-4F3B-907F-1A25A76F98E8"),
        ("var", "4D21B016-B534-45C2-A9FB-5C16E091FD2D"),
        ("linux-lvm", "E6D6D379-F507-44C2-A23C-238F2A3DF928"),
        ("linux-raid", "A19D880F-05FC-4D3B-A006-743F0F84911E"),
        ("linux-root-x86", "44479540-F297-41B2-9AF7-D131D5F0458A"),
        ("linux-root-x86-64", "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709"),
        ("linux-root-arm", "69DAD710-2CE4-4E3C-B16C-21A1D49ABED3"),
        ("linux-root-arm64", "B921B045-1DF0-41C3-AF44-4C6F280D3FAE"),
        ("linux-root-riscv64", "72EC70A6-CF74-40E6-BD49-4BDA08E8F224"),
        ("linux-usr-x86-64", "8484680C-9521-48C6-9C11-B0720656F69E"),
        ("linux-usr-arm", "7D0359A3-02B3-4F0A-865C-654403E70625"),
        ("linux-usr-arm64", "B0E01050-EE5F-4390-949A-9101B17104E9"),
        ("linux-usr-riscv64", "BEAEC34B-8442-439B-A40B-984381ED097D"),
        (
            "linux-root-verity-x86-64",
            "2C7357ED-EBD2-46D9-AEC1-23D437EC2BF5",
        ),
        (
            "linux-root-verity-arm",
            "7386CDF2-203C-47A9-A498-F2ECCE45A2D6",
        ),
        (
            "linux-root-verity-arm64",
            "DF3300CE-D69F-4C92-978C-9BFB0F38D820",
        ),
        (
            "linux-usr-verity-x86-64",
            "77FF5F63-E7B6-4633-ACF4-1565B864C0E6",
        ),
        (
            "linux-usr-verity-arm",
            "C215D751-7BCD-4649-BE90-6627490A4C05",
        ),
        (
            "linux-usr-verity-arm64",
            "6E11A4E7-FBCA-4DED-B9E9-E1A512BB664E",
        ),
        (
            "linux-root-verity-riscv64",
            "B6ED5582-440B-4209-B8DA-5FF7C419EA3D",
        ),
        (
            "linux-usr-verity-riscv64",
            "8F1056BE-9B05-47C4-81D6-BE53128E5B54",
        ),
    ];

    pub(crate) fn from_alias(alias: &str) -> Option<GptGuid> {
        Self::ALIASES
            .iter()
            .find(|(name, _)| *name == alias)
            .map(|(_, guid)| GptGuid::parse(guid).expect("alias GUIDs are well formed"))
    }
}

/// Why a GPT partition type is not usable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum GptPartitionTypeError {
    InvalidRaw(String),
    UnknownAlias(String),
}

impl std::fmt::Display for GptPartitionTypeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRaw(value) => write!(
                formatter,
                "partition type '{value}' must be a GUID of 8-4-4-4-12 hex digits"
            ),
            Self::UnknownAlias(value) => {
                write!(formatter, "unknown GPT partition type alias '{value}'")
            }
        }
    }
}

impl AssemblyDiskPartitionSpec {
    /// The GPT type GUID: `type` as a raw GUID, `type_alias` by name, or
    /// `linux` when neither is set.
    pub fn gpt_partition_type(&self) -> Result<GptGuid, GptPartitionTypeError> {
        if let Some(kind) = &self.kind {
            return GptGuid::parse(kind)
                .map_err(|_| GptPartitionTypeError::InvalidRaw(kind.clone()));
        }
        let alias = self.type_alias.as_deref().unwrap_or("linux");
        GptPartitionType::from_alias(alias)
            .ok_or_else(|| GptPartitionTypeError::UnknownAlias(alias.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_round_trip_and_store_mixed_endian() {
        let guid = GptGuid::parse("c12a7328-f81f-11d2-ba4b-00a0c93ec93b").expect("guid");
        assert_eq!(guid.to_string(), "C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
        assert_eq!(
            guid.to_disk_bytes(),
            [
                0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e,
                0xc9, 0x3b
            ]
        );
        assert!(GptGuid::parse("c12a7328f81f11d2ba4b00a0c93ec93b").is_err());
        assert!(GptGuid::parse("g12a7328-f81f-11d2-ba4b-00a0c93ec93b").is_err());
    }

    #[test]
    fn every_alias_is_a_well_formed_guid() {
        for (alias, _) in GptPartitionType::ALIASES {
            assert!(GptPartitionType::from_alias(alias).is_some(), "{alias}");
        }
        assert_eq!(GptPartitionType::from_alias("fat32-lba"), None);
    }

    /// Every alias's GUID is a type util-linux knows, where `sfdisk` is
    /// installed: a typo in a GUID makes a partition no tool recognizes.
    #[test]
    fn every_alias_guid_is_one_sfdisk_lists() {
        let Ok(output) = std::process::Command::new("sfdisk")
            .args(["--list-types", "--label", "gpt"])
            .output()
        else {
            eprintln!("skipped: sfdisk is not installed");
            return;
        };
        let listed = String::from_utf8_lossy(&output.stdout).to_ascii_uppercase();
        for (alias, guid) in GptPartitionType::ALIASES {
            assert!(listed.contains(guid), "{alias} {guid}");
        }
    }
}
