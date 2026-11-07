use crate::{AssemblyPathTemplate, ByteSize};

/// The bytes of a UBI erase-counter or volume-ID header.
pub const UBI_HEADER_SIZE: u32 = gaia_formats::ubi::HEADER_SIZE as u32;
/// The bytes of one UBI volume table record.
pub const UBI_VTBL_RECORD_SIZE: u32 = gaia_formats::ubi::VTBL_RECORD_SIZE as u32;
// The UBI format itself is gaia-formats', shared with Atlas.
pub use gaia_formats::ubi::{
    DEFAULT_BAD_PEBS_PER_1024 as UBI_DEFAULT_BAD_PEBS_PER_1024, Geometry as UbiGeometry,
    MAX_VOLUMES as UBI_MAX_VOLUMES, VOLUME_NAME_MAX as UBI_VOLUME_NAME_MAX,
};

/// `[[image.assembly.ubis]]`: a UBI image for raw NAND flash, written
/// after the filesystems and before the disks. Each volume is a slot, a
/// data area or a raw blob; EROFS slots are mounted through `ubiblock`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyUbiSpec {
    pub id: String,
    pub output: AssemblyPathTemplate,
    /// The flash's eraseblock size (`ubinize -p`).
    pub peb_size: String,
    /// The flash's page size (`ubinize -m`).
    pub min_io_size: String,
    /// The flash's sub-page size (`ubinize -s`); unset means no sub-pages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_page_size: Option<String>,
    /// The VID header offset (`ubinize -O`); unset puts it at the sub-page
    /// size, as the kernel does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vid_header_offset: Option<String>,
    /// The image sequence number; unset derives it from the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_seq: Option<u32>,
    /// The size of the whole flash device the image is for. Set, the build
    /// fails when the attached volumes and UBI's reserves do not fit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flash_size: Option<String>,
    /// PEBs UBI keeps for bad blocks per 1024 on the device: the kernel's
    /// `CONFIG_MTD_UBI_BEB_LIMIT` or `ubi.mtd=...,<limit>`.
    pub bad_pebs_per_1024: u32,
    pub volumes: Vec<AssemblyUbiVolumeSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyUbiVolumeSpec {
    pub name: String,
    /// The volume id; unset is the volume's position in the list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    pub kind: AssemblyUbiVolumeKindSpec,
    /// The volume's contents; unset leaves it empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<AssemblyPathTemplate>,
    /// The bytes reserved for the volume, at least its image: a slot sized
    /// for later, larger updates. Unset reserves the image's size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// UBI grows this volume over the free PEBs on first attach.
    pub autoresize: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssemblyUbiVolumeKindSpec {
    #[default]
    Dynamic,
    Static,
}

impl AssemblyUbiVolumeKindSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Static => "static",
        }
    }
}

fn size(field: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<ByteSize>()
        .map(|size| size.bytes())
        .map_err(|error| format!("{field} '{value}': {error}"))
}

impl AssemblyUbiSpec {
    pub fn geometry(&self) -> Result<UbiGeometry, String> {
        UbiGeometry::new(
            size("peb_size", &self.peb_size)?,
            size("min_io_size", &self.min_io_size)?,
            self.sub_page_size
                .as_deref()
                .map(|value| size("sub_page_size", value))
                .transpose()?,
            self.vid_header_offset
                .as_deref()
                .map(|value| size("vid_header_offset", value))
                .transpose()?,
        )
        .map_err(|error| error.to_string())
    }

    pub fn parsed_flash_size(&self) -> Result<Option<u64>, String> {
        self.flash_size
            .as_deref()
            .map(|value| size("flash_size", value))
            .transpose()
    }

    /// Each volume's id: its own, else its position.
    pub fn volume_ids(&self) -> Vec<u32> {
        self.volumes
            .iter()
            .enumerate()
            .map(|(index, volume)| volume.id.unwrap_or(index as u32))
            .collect()
    }

    /// What is wrong with the volume set on `geometry`: too many volumes,
    /// ids past the volume table, names out of range, repeated ids or
    /// names, more than one autoresize volume and sizes that do not parse.
    pub fn volume_problems(&self, geometry: &UbiGeometry) -> Vec<String> {
        let mut problems = Vec::new();
        let records = geometry.vtbl_records();
        if self.volumes.len() > records as usize {
            problems.push(format!(
                "{} volumes are more than the {records} a {}-byte LEB's volume table holds",
                self.volumes.len(),
                geometry.leb_size()
            ));
        }
        if self
            .volumes
            .iter()
            .filter(|volume| volume.autoresize)
            .count()
            > 1
        {
            problems.push("only one volume can be autoresize".into());
        }
        let ids = self.volume_ids();
        for (index, volume) in self.volumes.iter().enumerate() {
            if ids[index] >= records {
                problems.push(format!(
                    "volume '{}' id {} is past the last volume table record ({})",
                    volume.name,
                    ids[index],
                    records - 1
                ));
            }
            if volume.name.is_empty() || volume.name.len() > UBI_VOLUME_NAME_MAX {
                problems.push(format!(
                    "volume name '{}' must be 1 to {UBI_VOLUME_NAME_MAX} bytes",
                    volume.name
                ));
            }
            if let Some(value) = &volume.size
                && let Err(error) = size("size", value)
            {
                problems.push(format!("volume '{}' {error}", volume.name));
            }
            if volume.image.is_none() && volume.size.is_none() && !volume.autoresize {
                problems.push(format!(
                    "volume '{}' has no image, size or autoresize, so it would hold nothing",
                    volume.name
                ));
            }
            for (other_index, other) in self.volumes[..index].iter().enumerate() {
                if ids[other_index] == ids[index] {
                    problems.push(format!(
                        "volumes '{}' and '{}' share id {}",
                        other.name, volume.name, ids[index]
                    ));
                }
                if other.name == volume.name {
                    problems.push(format!("two volumes are named '{}'", volume.name));
                }
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(name: &str, id: Option<u32>, autoresize: bool) -> AssemblyUbiVolumeSpec {
        AssemblyUbiVolumeSpec {
            name: name.into(),
            id,
            kind: AssemblyUbiVolumeKindSpec::Dynamic,
            image: None,
            size: Some("1M".into()),
            autoresize,
        }
    }

    fn ubi(volumes: Vec<AssemblyUbiVolumeSpec>) -> AssemblyUbiSpec {
        AssemblyUbiSpec {
            id: "nand".into(),
            output: "$assembly.out/nand.ubi".into(),
            peb_size: "128K".into(),
            min_io_size: "2K".into(),
            sub_page_size: None,
            vid_header_offset: None,
            image_seq: None,
            flash_size: None,
            bad_pebs_per_1024: UBI_DEFAULT_BAD_PEBS_PER_1024,
            volumes,
        }
    }

    #[test]
    fn geometry_follows_ubinize_defaults_and_rejects_bad_sizes() {
        let nand = ubi(Vec::new()).geometry().expect("geometry");
        assert_eq!((nand.vid_header_offset, nand.data_offset), (2048, 4096));
        assert_eq!(nand.leb_size(), 126_976);
        assert_eq!(nand.vtbl_records(), 128);
        let subpages = UbiGeometry::new(128 * 1024, 2048, Some(512), None).expect("subpages");
        assert_eq!(
            (subpages.vid_header_offset, subpages.data_offset),
            (512, 2048)
        );
        let nor = UbiGeometry::new(64 * 1024, 1, None, Some(64)).expect("nor");
        assert_eq!((nor.vid_header_offset, nor.data_offset), (64, 128));
        assert_eq!(
            UbiGeometry::new(4096, 512, None, None)
                .expect("small")
                .vtbl_records(),
            17
        );

        assert!(UbiGeometry::new(100_000, 2048, None, None).is_err());
        assert!(UbiGeometry::new(4096, 4096, None, None).is_err());
        assert!(UbiGeometry::new(128 * 1024, 2048, Some(4096), None).is_err());
        assert!(UbiGeometry::new(128 * 1024, 2048, None, Some(32)).is_err());
        let mut bad = ubi(Vec::new());
        bad.peb_size = "lots".into();
        assert!(
            bad.geometry()
                .expect_err("unparsed")
                .contains("peb_size 'lots'")
        );
    }

    #[test]
    fn ids_default_to_positions_and_bad_sets_are_reported() {
        let set = ubi(vec![
            volume("a", None, false),
            volume("b", Some(5), true),
            volume("c", None, false),
        ]);
        assert_eq!(set.volume_ids(), [0, 5, 2]);
        let geometry = set.geometry().expect("geometry");
        assert_eq!(set.volume_problems(&geometry), Vec::<String>::new());

        let mut empty = volume("e", None, false);
        empty.size = None;
        let bad = ubi(vec![
            volume("a", Some(1), true),
            volume("a", None, true),
            volume("x", Some(128), false),
            empty,
        ]);
        let problems = bad.volume_problems(&geometry);
        assert_eq!(
            problems,
            [
                "only one volume can be autoresize",
                "volumes 'a' and 'a' share id 1",
                "two volumes are named 'a'",
                "volume 'x' id 128 is past the last volume table record (127)",
                "volume 'e' has no image, size or autoresize, so it would hold nothing",
            ]
        );
    }
}
