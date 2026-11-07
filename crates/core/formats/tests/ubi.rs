//! A UBI image built with the crate's encoders, as Gaia lays one out, read
//! back by the image reader and checked byte by byte against the offsets of
//! `ubi-media.h`.

use gaia_formats::crc::ubi_crc;
use gaia_formats::ubi::{
    EcHeader, Geometry, HeaderError, ImageError, LAYOUT_VOLUME_COMPAT, LAYOUT_VOLUME_ID, VidHeader,
    VolumeKind, VtblRecord, encode_peb, encode_volume_table, find_peb_size, read_image,
};
use std::io::Cursor;

const SEQ: u32 = 0x1234_5678;
const PEB: usize = 16 * 1024;

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// 16 KiB PEBs, 512-byte pages: rootfs_a (dynamic, data), rootfs_b (empty),
/// kernel (static, data) and data (autoresize).
fn sample(rootfs: &[u8], kernel: &[u8]) -> Vec<u8> {
    let geometry = Geometry::new(PEB as u64, 512, None, None).unwrap();
    let layout = geometry.layout();
    let leb = geometry.leb_size() as usize;
    let table = encode_volume_table(
        &layout,
        &[
            (
                0,
                VtblRecord::new(4, VolumeKind::Dynamic, "rootfs_a", false),
            ),
            (
                1,
                VtblRecord::new(4, VolumeKind::Dynamic, "rootfs_b", false),
            ),
            (2, VtblRecord::new(1, VolumeKind::Static, "kernel", false)),
            (3, VtblRecord::new(1, VolumeKind::Dynamic, "data", true)),
        ],
    )
    .unwrap();
    let ec = EcHeader {
        erase_counter: 0,
        vid_header_offset: geometry.vid_header_offset,
        data_offset: geometry.data_offset,
        image_seq: SEQ,
    };
    let mut image = Vec::new();
    for lnum in 0..2 {
        let vid = VidHeader {
            compat: LAYOUT_VOLUME_COMPAT,
            ..VidHeader::new(VolumeKind::Dynamic, LAYOUT_VOLUME_ID, lnum)
        };
        image.extend(encode_peb(&layout, &ec, &vid, &table));
    }
    for (lnum, chunk) in rootfs.chunks(leb).enumerate() {
        let vid = VidHeader::new(VolumeKind::Dynamic, 0, lnum as u32);
        image.extend(encode_peb(&layout, &ec, &vid, chunk));
    }
    let used = kernel.len().div_ceil(leb) as u32;
    for (lnum, chunk) in kernel.chunks(leb).enumerate() {
        let vid = VidHeader {
            data_size: chunk.len() as u32,
            used_ebs: used,
            data_crc: ubi_crc(chunk),
            ..VidHeader::new(VolumeKind::Static, 2, lnum as u32)
        };
        image.extend(encode_peb(&layout, &ec, &vid, chunk));
    }
    image
}

#[test]
fn the_bytes_sit_where_ubi_media_h_puts_them() {
    let rootfs: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let image = sample(&rootfs, &[0x5a; 100]);
    assert_eq!(image.len(), 5 * PEB);
    for peb in image.chunks(PEB) {
        // struct ubi_ec_hdr
        assert_eq!(&peb[0..4], b"UBI#");
        assert_eq!(peb[4], 1);
        assert_eq!(&peb[8..16], &[0; 8], "erase counter");
        assert_eq!(be32(peb, 16), 512, "vid_hdr_offset");
        assert_eq!(be32(peb, 20), 1024, "data_offset");
        assert_eq!(be32(peb, 24), SEQ, "image_seq");
        assert_eq!(be32(peb, 60), ubi_crc(&peb[..60]), "hdr_crc");
        assert!(peb[64..512].iter().all(|byte| *byte == 0xff));
        // struct ubi_vid_hdr
        let vid = &peb[512..576];
        assert_eq!(&vid[0..4], b"UBI!");
        assert_eq!(vid[4], 1);
        assert_eq!(vid[6], 0, "copy_flag");
        assert_eq!(&vid[40..48], &[0; 8], "sqnum");
        assert_eq!(be32(vid, 60), ubi_crc(&vid[..60]));
        assert!(peb[576..1024].iter().all(|byte| *byte == 0xff));
    }
    let layout_vid = &image[PEB + 512..][..64];
    assert_eq!((layout_vid[5], layout_vid[7]), (1, 5));
    assert_eq!(be32(layout_vid, 8), 0x7fff_efff);
    assert_eq!(be32(layout_vid, 12), 1);
    // struct ubi_vtbl_record, 89 records in a 15 KiB LEB
    let table = &image[1024..PEB];
    let record = |id: usize| &table[id * 172..][..172];
    assert_eq!(be32(record(0), 0), 4, "reserved_pebs");
    assert_eq!(be32(record(0), 4), 1, "alignment");
    assert_eq!(be32(record(0), 8), 0, "data_pad");
    assert_eq!(
        (record(0)[12], record(0)[13]),
        (1, 0),
        "vol_type, upd_marker"
    );
    assert_eq!(&record(0)[14..16], &[0, 8], "name_len");
    assert_eq!(&record(0)[16..25], b"rootfs_a\0");
    assert_eq!(record(2)[12], 2, "static");
    assert_eq!(record(3)[144], 1, "autoresize flag");
    for id in 0..89 {
        assert_eq!(be32(record(id), 168), ubi_crc(&record(id)[..168]));
    }
    assert_eq!(be32(record(4), 168), 0xf116_c36b, "an empty record");
    assert!(table[89 * 172..].iter().all(|byte| *byte == 0xff));
    // The static LEB: data_size, used_ebs, data_crc.
    let kernel_vid = &image[4 * PEB + 512..][..64];
    assert_eq!(kernel_vid[5], 2);
    assert_eq!(be32(kernel_vid, 20), 100);
    assert_eq!(be32(kernel_vid, 24), 1);
    assert_eq!(be32(kernel_vid, 32), ubi_crc(&[0x5a; 100]));
}

#[test]
fn a_gaia_style_image_reads_back() {
    let rootfs: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let bytes = sample(&rootfs, &[0x5a; 100]);
    let mut cursor = Cursor::new(&bytes);
    assert_eq!(find_peb_size(&mut cursor).unwrap(), PEB as u32);
    let image = read_image(&mut cursor, PEB as u32).unwrap();
    assert_eq!(
        image.layout,
        Geometry::new(PEB as u64, 512, None, None).unwrap().layout()
    );
    assert_eq!((image.pebs, image.image_seq), (5, SEQ));
    let volumes: Vec<(&str, u32, u32, Option<u32>)> = image
        .volumes
        .iter()
        .map(|v| (v.name.as_str(), v.reserved_lebs, v.image_lebs, v.first_peb))
        .collect();
    assert_eq!(
        volumes,
        [
            ("rootfs_a", 4, 2, Some(2)),
            ("rootfs_b", 4, 0, None),
            ("kernel", 1, 1, Some(4)),
            ("data", 1, 0, None),
        ]
    );
    assert_eq!(image.volume("kernel").unwrap().static_bytes, Some(100));
    assert!(image.volume("data").unwrap().autoresize);
    assert_eq!(image.reserved_pebs(), 2 + 2 + 10);
}

#[test]
fn damage_is_named() {
    let read = |bytes: &[u8]| read_image(&mut Cursor::new(bytes), PEB as u32).unwrap_err();
    let mut bytes = sample(&[1; 100], &[2; 10]);
    bytes[PEB + 4096] ^= 1; // the second table copy
    assert!(matches!(read(&bytes), ImageError::TablesDiffer));

    let mut bytes = sample(&[1; 100], &[2; 10]);
    bytes[2 * PEB + 512 + 13] ^= 1; // a VID header's lnum
    assert!(matches!(
        read(&bytes),
        ImageError::Header {
            peb: 2,
            error: HeaderError::Crc { .. }
        }
    ));

    let mut bytes = sample(&[1; 100], &[2; 10]);
    bytes[3 * PEB + 1024] ^= 1; // the static volume's data
    assert!(matches!(
        read(&bytes),
        ImageError::StaticDataCrc { peb: 3, .. }
    ));

    let mut bytes = sample(&[1; 100], &[2; 10]);
    bytes[2 * PEB..3 * PEB].fill(0xff);
    assert!(matches!(read(&bytes), ImageError::ErasedPeb { peb: 2 }));

    let error = read(&sample(&[1; 100], &[2; 10])[..1000]);
    assert!(error.to_string().contains("whole number"), "{error}");
}
