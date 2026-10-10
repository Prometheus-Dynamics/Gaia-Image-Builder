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

    let (needed, search_paths) = match strtab_vaddr {
        Some(vaddr) => {
            let offset = vaddr_to_offset(&loads, vaddr)?;
            // The string table has no length in the dynamic section that
            // this reader needs, so it is read to the end of the file.
            let table = bytes
                .get(usize::try_from(offset).map_err(|_| "ELF offset out of range")?..)
                .ok_or_else(|| "truncated ELF file".to_string())?;
            needed_and_paths(table)?
        }
        None if needed_offsets.is_empty() => (Vec::new(), Vec::new()),
        None => return Err("dynamic section has DT_NEEDED but no DT_STRTAB".into()),
    };

    Ok(ElfObject {
        is_64bit,
        little_endian,
        machine,
        interpreter,
        needed,
        search_paths,
    })
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

#[cfg(test)]
#[path = "elf_tests.rs"]
pub(crate) mod tests;
