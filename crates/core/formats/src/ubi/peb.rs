//! Whole PEBs and the volume table, as Gaia's image writer lays them out.

use super::{
    ERASED, EcHeader, HEADER_SIZE, HeaderError, PebLayout, VTBL_RECORD_SIZE, VidHeader, VtblRecord,
};

/// One whole PEB: the EC header, the VID header and the LEB's bytes, with
/// erased (0xff) bytes between them and after the data. `data` is at most
/// a LEB; the header offsets come from `layout`, not from `ec`.
pub fn encode_peb(layout: &PebLayout, ec: &EcHeader, vid: &VidHeader, data: &[u8]) -> Vec<u8> {
    let mut peb = vec![ERASED; layout.peb_size as usize];
    peb[..HEADER_SIZE].copy_from_slice(&ec.encode());
    let vid_at = layout.vid_header_offset as usize;
    peb[vid_at..vid_at + HEADER_SIZE].copy_from_slice(&vid.encode());
    let data_at = layout.data_offset as usize;
    peb[data_at..data_at + data.len()].copy_from_slice(data);
    peb
}

/// The volume table a layout-volume LEB holds: `layout.vtbl_records()`
/// records, each volume's at its id and every other one empty. A volume id
/// past the table is refused as [`HeaderError::Field`].
pub fn encode_volume_table(
    layout: &PebLayout,
    volumes: &[(u32, VtblRecord)],
) -> Result<Vec<u8>, HeaderError> {
    let records = layout.vtbl_records() as usize;
    let empty = VtblRecord::empty();
    let mut table = Vec::with_capacity(records * VTBL_RECORD_SIZE);
    for _ in 0..records {
        table.extend_from_slice(&empty);
    }
    for (id, record) in volumes {
        let at = *id as usize;
        if at >= records {
            return Err(HeaderError::Field {
                what: "volume table",
                field: "volume id",
                value: u64::from(*id),
            });
        }
        table[at * VTBL_RECORD_SIZE..][..VTBL_RECORD_SIZE].copy_from_slice(&record.encode()?);
    }
    Ok(table)
}
