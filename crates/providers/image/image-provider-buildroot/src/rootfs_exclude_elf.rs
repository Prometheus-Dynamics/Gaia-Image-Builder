//! The names an ELF object is loaded by: its `DT_SONAME` and its `DT_NEEDED`
//! entries. The rootfs exclusion check needs only these, so this is a small
//! reader beside the provider; the loader reader of `gaia-plan` (which can
//! also follow search paths and interpreters) is not a dependency of the
//! provider. Bytes that are not an ELF object this reader understands are
//! `None`: the check skips them rather than failing on them.

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_SONAME: u64 = 14;

/// The names one ELF object is loaded by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ElfNames {
    pub(crate) soname: Option<String>,
    pub(crate) needed: Vec<String>,
}

/// Whether `bytes` starts with the ELF magic (the cheap test before a read).
pub(crate) fn has_elf_magic(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(ELF_MAGIC.as_slice())
}

/// The names of an ELF object; an object without a dynamic section has none.
pub(crate) fn elf_names(bytes: &[u8]) -> Option<ElfNames> {
    if !has_elf_magic(bytes) || bytes.len() < 64 {
        return None;
    }
    let is_64bit = match bytes[4] {
        1 => false,
        2 => true,
        _ => return None,
    };
    let little_endian = match bytes[5] {
        1 => true,
        2 => false,
        _ => return None,
    };
    let read = |at: u64, width: usize| read_uint(bytes, at, width, little_endian);
    let (phoff, phentsize, phnum) = if is_64bit {
        (read(0x20, 8)?, read(0x36, 2)?, read(0x38, 2)?)
    } else {
        (read(0x1c, 4)?, read(0x2a, 2)?, read(0x2c, 2)?)
    };

    // (vaddr, offset, filesz) of each loadable segment, and the dynamic section.
    let mut loads = Vec::new();
    let mut dynamic = None;
    for index in 0..phnum {
        let at = phoff.checked_add(index.checked_mul(phentsize)?)?;
        let p_type = read(at, 4)? as u32;
        let (offset, vaddr, filesz) = if is_64bit {
            (read(at + 8, 8)?, read(at + 16, 8)?, read(at + 32, 8)?)
        } else {
            (read(at + 4, 4)?, read(at + 8, 4)?, read(at + 16, 4)?)
        };
        match p_type {
            PT_LOAD => loads.push((vaddr, offset, filesz)),
            PT_DYNAMIC => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let Some((dynamic_offset, dynamic_size)) = dynamic else {
        return Some(ElfNames::default());
    };

    let entry_size: u64 = if is_64bit { 16 } else { 8 };
    let mut needed_offsets = Vec::new();
    let mut soname_offset = None;
    let mut strtab_vaddr = None;
    for index in 0..dynamic_size / entry_size {
        let at = dynamic_offset.checked_add(index * entry_size)?;
        let (tag, value) = if is_64bit {
            (read(at, 8)?, read(at + 8, 8)?)
        } else {
            (read(at, 4)?, read(at + 4, 4)?)
        };
        match tag {
            DT_NULL => break,
            DT_NEEDED => needed_offsets.push(value),
            DT_SONAME => soname_offset = Some(value),
            DT_STRTAB => strtab_vaddr = Some(value),
            _ => {}
        }
    }
    if needed_offsets.is_empty() && soname_offset.is_none() {
        return Some(ElfNames::default());
    }

    // The string table is mapped at DT_STRTAB; its length is not recorded,
    // so strings are read up to their terminators from its start to the end
    // of the file.
    let strtab_vaddr = strtab_vaddr?;
    let (start, offset, _) = loads.iter().copied().find(|(start, _, filesz)| {
        strtab_vaddr >= *start && strtab_vaddr - start < (*filesz).max(1)
    })?;
    let table = bytes.get(usize::try_from(offset + (strtab_vaddr - start)).ok()?..)?;
    let needed = needed_offsets
        .iter()
        .map(|offset| string_at(table, *offset))
        .collect::<Option<Vec<_>>>()?;
    let soname = match soname_offset {
        Some(offset) => Some(string_at(table, offset)?),
        None => None,
    };
    Some(ElfNames { soname, needed })
}

/// An unsigned integer of `width` bytes at `at`, in the object's byte order.
fn read_uint(bytes: &[u8], at: u64, width: usize, little_endian: bool) -> Option<u64> {
    let start = usize::try_from(at).ok()?;
    let raw = bytes.get(start..start.checked_add(width)?)?;
    let value = if little_endian {
        raw.iter()
            .rev()
            .fold(0u64, |value, byte| (value << 8) | u64::from(*byte))
    } else {
        raw.iter()
            .fold(0u64, |value, byte| (value << 8) | u64::from(*byte))
    };
    Some(value)
}

/// The NUL-terminated string at `offset` of `table`, if it is UTF-8.
fn string_at(table: &[u8], offset: u64) -> Option<String> {
    let rest = table.get(usize::try_from(offset).ok()?..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    String::from_utf8(rest[..end].to_vec()).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian ELF64 shared object with the given `DT_NEEDED` names and
    /// `DT_SONAME`: one loadable segment over the whole file, the dynamic
    /// section after the program headers, and the string table last.
    pub(crate) fn elf64_object(needed: &[&str], soname: Option<&str>) -> Vec<u8> {
        let mut strtab = vec![0u8];
        let mut add = |name: &str| {
            let offset = strtab.len() as u64;
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
            offset
        };
        let mut dynamic: Vec<(u64, u64)> =
            needed.iter().map(|name| (DT_NEEDED, add(name))).collect();
        if let Some(name) = soname {
            dynamic.push((DT_SONAME, add(name)));
        }
        let dynamic_offset = 64 + 2 * 56;
        let dynamic_len = (dynamic.len() + 2) * 16;
        let strtab_offset = dynamic_offset + dynamic_len;
        dynamic.push((DT_STRTAB, strtab_offset as u64));
        dynamic.push((DT_NULL, 0));
        let total = strtab_offset + strtab.len();

        let mut out = vec![0u8; total];
        out[..4].copy_from_slice(ELF_MAGIC);
        out[4] = 2; // ELFCLASS64
        out[5] = 1; // little endian
        out[6] = 1; // EV_CURRENT
        out[0x10..0x12].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        out[0x12..0x14].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
        out[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
        out[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        out[0x38..0x3a].copy_from_slice(&2u16.to_le_bytes());
        let mut program_header = |at: usize, p_type: u32, offset: u64, filesz: u64| {
            out[at..at + 4].copy_from_slice(&p_type.to_le_bytes());
            out[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
            out[at + 16..at + 24].copy_from_slice(&offset.to_le_bytes());
            out[at + 32..at + 40].copy_from_slice(&filesz.to_le_bytes());
        };
        program_header(64, PT_LOAD, 0, total as u64);
        program_header(
            64 + 56,
            PT_DYNAMIC,
            dynamic_offset as u64,
            dynamic_len as u64,
        );
        for (index, (tag, value)) in dynamic.iter().enumerate() {
            let at = dynamic_offset + index * 16;
            out[at..at + 8].copy_from_slice(&tag.to_le_bytes());
            out[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
        }
        out[strtab_offset..].copy_from_slice(&strtab);
        out
    }

    #[test]
    fn reads_soname_and_needed_names_of_a_shared_object() {
        let bytes = elf64_object(&["libc.so.6", "libcamera.so.0.7"], Some("libfoo.so.1"));
        assert_eq!(
            elf_names(&bytes),
            Some(ElfNames {
                soname: Some("libfoo.so.1".into()),
                needed: vec!["libc.so.6".into(), "libcamera.so.0.7".into()],
            })
        );
    }

    #[test]
    fn an_object_without_a_dynamic_section_has_no_names() {
        let mut bytes = elf64_object(&[], None);
        // Turn PT_DYNAMIC into PT_NULL: a static object.
        bytes[64 + 56..64 + 60].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(elf_names(&bytes), Some(ElfNames::default()));
    }

    #[test]
    fn text_and_truncated_objects_are_not_elf_names() {
        assert_eq!(elf_names(b"#!/bin/sh\n"), None);
        let bytes = elf64_object(&["libc.so.6"], None);
        assert_eq!(elf_names(&bytes[..bytes.len() - 40]), None);
    }
}
