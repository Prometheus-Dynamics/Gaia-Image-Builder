//! A small ELF reader for the parts a dynamic loader needs to find a
//! program's libraries: the program interpreter (`PT_INTERP`), the
//! `DT_NEEDED` names and the `DT_RUNPATH` / `DT_RPATH` search entries.
//!
//! It reads the file bytes only and never runs anything, so it can inspect
//! a binary of another architecture (a cross-compiled target binary on a
//! host). ELF32 and ELF64, little and big endian, are supported. Every read
//! is bounds-checked, so a truncated or corrupt file is an error, not a panic.

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_RPATH: u64 = 15;
const DT_RUNPATH: u64 = 29;
const DT_VERDEF: u64 = 0x6fff_fffc;
const DT_VERDEFNUM: u64 = 0x6fff_fffd;
const DT_VERNEED: u64 = 0x6fff_fffe;
const DT_VERNEEDNUM: u64 = 0x6fff_ffff;

/// One symbol version the object needs from a library: `version` (such as
/// `GLIBC_2.17`) of the library named `file` (such as `libc.so.6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionNeed {
    pub file: String,
    pub version: String,
}

/// What the loader needs from one ELF object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfObject {
    /// 64-bit (`ELFCLASS64`) or 32-bit.
    pub is_64bit: bool,
    /// Little-endian (`ELFDATA2LSB`) or big-endian.
    pub little_endian: bool,
    /// `e_machine`: the target architecture.
    pub machine: u16,
    /// The program interpreter path, for a dynamically linked program.
    pub interpreter: Option<String>,
    /// `DT_NEEDED` names, in file order.
    pub needed: Vec<String>,
    /// `DT_RUNPATH` entries, or `DT_RPATH` when there is no RUNPATH: the
    /// directories this object's own dependencies are searched in first.
    pub search_paths: Vec<String>,
    /// `p_align` of every `PT_LOAD` segment, in file order. A loader on a
    /// kernel with larger pages needs each to be a multiple of its page size.
    pub load_alignments: Vec<u64>,
    /// The symbol versions this object needs (`DT_VERNEED`), in file order.
    pub version_needs: Vec<VersionNeed>,
    /// The symbol versions this object defines (`DT_VERDEF`), in file order.
    /// A C library lists its `GLIBC_x.y` versions here.
    pub version_defs: Vec<String>,
}

impl ElfObject {
    /// A binary with no interpreter and no `DT_NEEDED` entry is static.
    pub fn is_dynamic(&self) -> bool {
        self.interpreter.is_some() || !self.needed.is_empty()
    }

    /// The architecture a library must match to be loadable by this object.
    pub fn architecture(&self) -> (bool, bool, u16) {
        (self.is_64bit, self.little_endian, self.machine)
    }
}

#[derive(Clone, Copy)]
struct Layout {
    is_64bit: bool,
    little_endian: bool,
}

impl Layout {
    fn word(self, bytes: &[u8]) -> Result<u64, String> {
        Ok(match (self.is_64bit, self.little_endian) {
            (true, true) => u64::from_le_bytes(array(bytes)?),
            (true, false) => u64::from_be_bytes(array(bytes)?),
            (false, true) => u64::from(u32::from_le_bytes(array(bytes)?)),
            (false, false) => u64::from(u32::from_be_bytes(array(bytes)?)),
        })
    }

    fn half(self, bytes: &[u8]) -> Result<u16, String> {
        Ok(if self.little_endian {
            u16::from_le_bytes(array(bytes)?)
        } else {
            u16::from_be_bytes(array(bytes)?)
        })
    }

    fn word32(self, bytes: &[u8]) -> Result<u32, String> {
        Ok(if self.little_endian {
            u32::from_le_bytes(array(bytes)?)
        } else {
            u32::from_be_bytes(array(bytes)?)
        })
    }

    /// The width of an address or file offset field.
    fn addr_size(self) -> usize {
        if self.is_64bit { 8 } else { 4 }
    }
}

fn array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], String> {
    bytes
        .get(..N)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| "truncated ELF file".to_string())
}

fn slice(bytes: &[u8], start: u64, len: u64) -> Result<&[u8], String> {
    let start = usize::try_from(start).map_err(|_| "ELF offset out of range".to_string())?;
    let len = usize::try_from(len).map_err(|_| "ELF size out of range".to_string())?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| "ELF offset out of range".to_string())?;
    bytes
        .get(start..end)
        .ok_or_else(|| "truncated ELF file".to_string())
}

/// One `PT_LOAD` segment: where a virtual address lives in the file.
struct LoadSegment {
    vaddr: u64,
    offset: u64,
    filesz: u64,
    align: u64,
}

/// Parses an ELF object's loader-visible metadata from its file bytes.
pub fn parse_elf(bytes: &[u8]) -> Result<ElfObject, String> {
    if bytes.len() < 16 || &bytes[..4] != ELF_MAGIC {
        return Err("not an ELF file".into());
    }
    let is_64bit = match bytes[4] {
        1 => false,
        2 => true,
        other => return Err(format!("unknown ELF class {other}")),
    };
    let little_endian = match bytes[5] {
        1 => true,
        2 => false,
        other => return Err(format!("unknown ELF data encoding {other}")),
    };
    let layout = Layout {
        is_64bit,
        little_endian,
    };
    // e_machine sits at the same offset in both classes.
    let machine = layout.half(slice(bytes, 0x12, 2)?)?;
    let (phoff, phentsize, phnum) = if is_64bit {
        (
            layout.word(slice(bytes, 0x20, 8)?)?,
            u64::from(layout.half(slice(bytes, 0x36, 2)?)?),
            u64::from(layout.half(slice(bytes, 0x38, 2)?)?),
        )
    } else {
        (
            u64::from(layout.word32(slice(bytes, 0x1c, 4)?)?),
            u64::from(layout.half(slice(bytes, 0x2a, 2)?)?),
            u64::from(layout.half(slice(bytes, 0x2c, 2)?)?),
        )
    };

    let mut loads = Vec::new();
    let mut dynamic = None;
    let mut interpreter = None;
    for index in 0..phnum {
        let header = slice(bytes, phoff + index * phentsize, phentsize)?;
        let (p_type, p_offset, p_vaddr, p_filesz) = program_header_fields(layout, header)?;
        match p_type {
            PT_LOAD => loads.push(LoadSegment {
                vaddr: p_vaddr,
                offset: p_offset,
                filesz: p_filesz,
                align: program_header_align(layout, header)?,
            }),
            PT_DYNAMIC => dynamic = Some((p_offset, p_filesz)),
            PT_INTERP => {
                let raw = slice(bytes, p_offset, p_filesz)?;
                let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
                interpreter = Some(text(&raw[..end])?);
            }
            _ => {}
        }
    }

    let mut needed_offsets = Vec::new();
    let mut rpath_offset = None;
    let mut runpath_offset = None;
    let mut strtab_vaddr = None;
    let mut verneed = None;
    let mut verdef = None;
    if let Some((offset, size)) = dynamic {
        let entry_size = if is_64bit { 16 } else { 8 };
        let count = size / entry_size;
        for index in 0..count {
            let entry = slice(bytes, offset + index * entry_size, entry_size)?;
            let (tag, value) = if is_64bit {
                (layout.word(&entry[..8])?, layout.word(&entry[8..])?)
            } else {
                (
                    u64::from(layout.word32(&entry[..4])?),
                    u64::from(layout.word32(&entry[4..])?),
                )
            };
            match tag {
                DT_NULL => break,
                DT_NEEDED => needed_offsets.push(value),
                DT_RPATH => rpath_offset = Some(value),
                DT_RUNPATH => runpath_offset = Some(value),
                DT_STRTAB => strtab_vaddr = Some(value),
                DT_VERNEED => verneed = Some((value, verneed.map_or(0, |(_, count)| count))),
                DT_VERNEEDNUM => verneed = Some((verneed.map_or(0, |(vaddr, _)| vaddr), value)),
                DT_VERDEF => verdef = Some((value, verdef.map_or(0, |(_, count)| count))),
                DT_VERDEFNUM => verdef = Some((verdef.map_or(0, |(vaddr, _)| vaddr), value)),
                _ => {}
            }
        }
    }

    let needed_and_paths = |string_table: &[u8]| -> Result<(Vec<String>, Vec<String>), String> {
        let needed = needed_offsets
            .iter()
            .map(|offset| string_at(string_table, *offset).and_then(text))
            .collect::<Result<Vec<_>, _>>()?;
        let raw_paths: &[u8] = match (runpath_offset, rpath_offset) {
            (Some(offset), _) => string_at(string_table, offset)?,
            (None, Some(offset)) => string_at(string_table, offset)?,
            (None, None) => &[],
        };
        let search_paths = text(raw_paths)?
            .split(':')
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();
        Ok((needed, search_paths))
    };

    let (needed, search_paths, version_needs, version_defs) = match strtab_vaddr {
        Some(vaddr) => {
            let offset = vaddr_to_offset(&loads, vaddr)?;
            // The string table has no length in the dynamic section that
            // this reader needs, so it is read to the end of the file.
            let table = bytes
                .get(usize::try_from(offset).map_err(|_| "ELF offset out of range")?..)
                .ok_or_else(|| "truncated ELF file".to_string())?;
            let (needed, search_paths) = needed_and_paths(table)?;
            let version_needs = read_version_needs(bytes, layout, &loads, table, verneed)?;
            let version_defs = read_version_defs(bytes, layout, &loads, table, verdef)?;
            (needed, search_paths, version_needs, version_defs)
        }
        None if needed_offsets.is_empty() => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        None => return Err("dynamic section has DT_NEEDED but no DT_STRTAB".into()),
    };

    Ok(ElfObject {
        is_64bit,
        little_endian,
        machine,
        interpreter,
        needed,
        search_paths,
        load_alignments: loads.iter().map(|segment| segment.align).collect(),
        version_needs,
        version_defs,
    })
}

/// The `DT_VERNEED` table: each library the object needs symbol versions from,
/// and each version of it. `verneed` is `(vaddr, count)`, from the dynamic
/// section; an object without the table needs no versions.
fn read_version_needs(
    bytes: &[u8],
    layout: Layout,
    loads: &[LoadSegment],
    table: &[u8],
    verneed: Option<(u64, u64)>,
) -> Result<Vec<VersionNeed>, String> {
    let Some((vaddr, count)) = verneed.filter(|(vaddr, _)| *vaddr != 0) else {
        return Ok(Vec::new());
    };
    // Elf_Verneed: vn_version(2) vn_cnt(2) vn_file(4) vn_aux(4) vn_next(4);
    // Elf_Vernaux: vna_hash(4) vna_flags(2) vna_other(2) vna_name(4) vna_next(4).
    let mut needs = Vec::new();
    let mut offset = vaddr_to_offset(loads, vaddr)?;
    for _ in 0..count {
        let entry = slice(bytes, offset, 16)?;
        let version_count = layout.half(&entry[2..4])?;
        let file = text(string_at(table, u64::from(layout.word32(&entry[4..8])?))?)?;
        let aux = u64::from(layout.word32(&entry[8..12])?);
        let next = u64::from(layout.word32(&entry[12..16])?);
        let mut aux_offset = offset + aux;
        for _ in 0..version_count {
            let aux_entry = slice(bytes, aux_offset, 16)?;
            let name = u64::from(layout.word32(&aux_entry[8..12])?);
            let version = text(string_at(table, name)?)?;
            needs.push(VersionNeed {
                file: file.clone(),
                version,
            });
            let aux_next = u64::from(layout.word32(&aux_entry[12..16])?);
            if aux_next == 0 {
                break;
            }
            aux_offset += aux_next;
        }
        if next == 0 {
            break;
        }
        offset += next;
    }
    Ok(needs)
}

/// The `DT_VERDEF` table: the version names the object defines. The first
/// name of each entry is its own; later ones are its parents, not listed.
fn read_version_defs(
    bytes: &[u8],
    layout: Layout,
    loads: &[LoadSegment],
    table: &[u8],
    verdef: Option<(u64, u64)>,
) -> Result<Vec<String>, String> {
    let Some((vaddr, count)) = verdef.filter(|(vaddr, _)| *vaddr != 0) else {
        return Ok(Vec::new());
    };
    // Elf_Verdef: vd_version(2) vd_flags(2) vd_ndx(2) vd_cnt(2) vd_hash(4)
    // vd_aux(4) vd_next(4); Elf_Verdaux: vda_name(4) vda_next(4).
    let mut defs = Vec::new();
    let mut offset = vaddr_to_offset(loads, vaddr)?;
    for _ in 0..count {
        let entry = slice(bytes, offset, 20)?;
        let aux = u64::from(layout.word32(&entry[12..16])?);
        let next = u64::from(layout.word32(&entry[16..20])?);
        let aux_entry = slice(bytes, offset + aux, 8)?;
        let name = u64::from(layout.word32(&aux_entry[0..4])?);
        defs.push(text(string_at(table, name)?)?);
        if next == 0 {
            break;
        }
        offset += next;
    }
    Ok(defs)
}

/// `p_align` of a program header: the 8-byte field of ELF64, the 4-byte one of
/// ELF32.
fn program_header_align(layout: Layout, header: &[u8]) -> Result<u64, String> {
    if layout.is_64bit {
        Ok(layout.word(slice(header, 48, 8)?)?)
    } else {
        Ok(u64::from(layout.word32(slice(header, 28, 4)?)?))
    }
}

/// `(p_type, p_offset, p_vaddr, p_filesz)` from one program header.
fn program_header_fields(layout: Layout, header: &[u8]) -> Result<(u32, u64, u64, u64), String> {
    if layout.is_64bit {
        // p_type(4) p_flags(4) p_offset(8) p_vaddr(8) p_paddr(8) p_filesz(8)
        Ok((
            layout.word32(slice(header, 0, 4)?)?,
            layout.word(slice(header, 8, 8)?)?,
            layout.word(slice(header, 16, 8)?)?,
            layout.word(slice(header, 32, 8)?)?,
        ))
    } else {
        // p_type(4) p_offset(4) p_vaddr(4) p_paddr(4) p_filesz(4)
        let width = layout.addr_size() as u64;
        Ok((
            layout.word32(slice(header, 0, 4)?)?,
            layout.word(slice(header, 4, width)?)?,
            layout.word(slice(header, 8, width)?)?,
            layout.word(slice(header, 16, width)?)?,
        ))
    }
}

fn vaddr_to_offset(loads: &[LoadSegment], vaddr: u64) -> Result<u64, String> {
    loads
        .iter()
        .find(|segment| vaddr >= segment.vaddr && vaddr - segment.vaddr < segment.filesz.max(1))
        .map(|segment| segment.offset + (vaddr - segment.vaddr))
        .ok_or_else(|| format!("ELF address 0x{vaddr:x} is not inside a loadable segment"))
}

fn string_at(table: &[u8], offset: u64) -> Result<&[u8], String> {
    let start = usize::try_from(offset).map_err(|_| "ELF string offset out of range")?;
    let rest = table
        .get(start..)
        .ok_or_else(|| "ELF string offset out of range".to_string())?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| "unterminated ELF string".to_string())?;
    Ok(&rest[..end])
}

fn text(raw: &[u8]) -> Result<String, String> {
    std::str::from_utf8(raw)
        .map(str::to_string)
        .map_err(|_| "ELF string is not UTF-8".to_string())
}

/// What stripping an ELF file needs to know: its type, its machine, and
/// whether it still has a symbol table or debug sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElfSections {
    /// `e_type`: 1 relocatable, 2 executable, 3 shared object.
    pub object_type: u16,
    /// `e_machine`: the target architecture.
    pub machine: u16,
    /// A `.symtab` section is present.
    pub has_symtab: bool,
    /// A `.debug_*` or `.zdebug_*` section is present (not `.debug_gdb_scripts`).
    pub has_debug: bool,
}

/// Reads an ELF file's type, machine and symbol and debug sections from its
/// bytes. Bytes without the ELF magic are `Ok(None)`; a truncated or corrupt
/// ELF file is an error.
pub fn elf_sections(bytes: &[u8]) -> Result<Option<ElfSections>, String> {
    elf_sections_from(bytes.len() as u64, |start, len| {
        slice(bytes, start, len).map(<[u8]>::to_vec)
    })
}

/// [`elf_sections`] for a file, read by seeking: only the ELF header, the
/// section table and the section names are read, never the whole file.
pub fn elf_sections_of_file(
    file: &mut (impl std::io::Read + std::io::Seek),
) -> Result<Option<ElfSections>, String> {
    use std::io::SeekFrom;
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|error| format!("failed to size ELF file: {error}"))?;
    elf_sections_from(len, |start, len| {
        file.seek(SeekFrom::Start(start))
            .map_err(|error| format!("failed to seek in ELF file: {error}"))?;
        let mut buffer =
            vec![0u8; usize::try_from(len).map_err(|_| "ELF size out of range".to_string())?];
        file.read_exact(&mut buffer)
            .map_err(|error| format!("ELF file is truncated: {error}"))?;
        Ok(buffer)
    })
}

/// The section scan over `read(start, len)`, which returns exactly `len`
/// bytes at `start` of a file `file_len` bytes long.
fn elf_sections_from(
    file_len: u64,
    mut read: impl FnMut(u64, u64) -> Result<Vec<u8>, String>,
) -> Result<Option<ElfSections>, String> {
    // The ELF64 header is the largest one the fields read here sit in.
    const HEADER_BYTES: u64 = 64;
    let head = read(0, file_len.min(HEADER_BYTES))?;
    if !head.starts_with(ELF_MAGIC) {
        return Ok(None);
    }
    let is_64bit = match head.get(4) {
        Some(1) => false,
        Some(2) => true,
        _ => return Err("unknown ELF class".into()),
    };
    let little_endian = match head.get(5) {
        Some(1) => true,
        Some(2) => false,
        _ => return Err("unknown ELF data encoding".into()),
    };
    let layout = Layout {
        is_64bit,
        little_endian,
    };
    let bytes = &head[..];
    let object_type = layout.half(slice(bytes, 0x10, 2)?)?;
    let machine = layout.half(slice(bytes, 0x12, 2)?)?;
    let (shoff, shentsize, shnum, shstrndx) = if is_64bit {
        (
            layout.word(slice(bytes, 0x28, 8)?)?,
            u64::from(layout.half(slice(bytes, 0x3a, 2)?)?),
            u64::from(layout.half(slice(bytes, 0x3c, 2)?)?),
            u64::from(layout.half(slice(bytes, 0x3e, 2)?)?),
        )
    } else {
        (
            layout.word(slice(bytes, 0x20, 4)?)?,
            u64::from(layout.half(slice(bytes, 0x2e, 2)?)?),
            u64::from(layout.half(slice(bytes, 0x30, 2)?)?),
            u64::from(layout.half(slice(bytes, 0x32, 2)?)?),
        )
    };
    let mut sections = ElfSections {
        object_type,
        machine,
        has_symtab: false,
        has_debug: false,
    };
    if shoff == 0 || shnum == 0 {
        return Ok(Some(sections));
    }
    if shstrndx >= shnum {
        return Err("ELF section name table index is out of range".into());
    }
    // The section table, then the names it points at; the headers index the
    // table from its start.
    let table_len = shnum
        .checked_mul(shentsize)
        .ok_or_else(|| "ELF section table size out of range".to_string())?;
    let table = read(shoff, table_len)?;
    let header = |index: u64| section_header(&table, layout, 0, shentsize, index);
    let names_header = header(shstrndx)?;
    let names = read(names_header.offset, names_header.size)?;
    for index in 0..shnum {
        let name = string_at(&names, u64::from(header(index)?.name))?;
        if name == b".symtab" {
            sections.has_symtab = true;
        } else if (name.starts_with(b".debug_") && name != b".debug_gdb_scripts")
            || name.starts_with(b".zdebug_")
        {
            // `.debug_gdb_scripts` is a GDB auto-load hook that the strip
            // tools leave in place; it is not debug information.
            sections.has_debug = true;
        }
    }
    Ok(Some(sections))
}

/// The fields of one section header that the section scan reads.
struct SectionHeader {
    name: u32,
    offset: u64,
    size: u64,
}

fn section_header(
    bytes: &[u8],
    layout: Layout,
    shoff: u64,
    shentsize: u64,
    index: u64,
) -> Result<SectionHeader, String> {
    let start = index
        .checked_mul(shentsize)
        .and_then(|relative| shoff.checked_add(relative))
        .ok_or_else(|| "ELF section header offset out of range".to_string())?;
    let entry = slice(bytes, start, shentsize)?;
    let name = layout.word32(slice(entry, 0, 4)?)?;
    let (offset, size) = if layout.is_64bit {
        (
            layout.word(slice(entry, 0x18, 8)?)?,
            layout.word(slice(entry, 0x20, 8)?)?,
        )
    } else {
        (
            layout.word(slice(entry, 0x10, 4)?)?,
            layout.word(slice(entry, 0x14, 4)?)?,
        )
    };
    Ok(SectionHeader { name, offset, size })
}

#[cfg(any(test, feature = "test-fixtures"))]
#[path = "elf_tests.rs"]
pub(crate) mod tests;
