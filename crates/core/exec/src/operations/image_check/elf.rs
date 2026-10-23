//! A reader for the parts of a 64-bit little-endian ELF object that a kernel
//! module carries: its section table, the section contents by name, and
//! `.symtab`. Every read is bounds-checked, so a truncated or hostile file is
//! an error, never a panic.

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const ET_REL: u16 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_NOBITS: u32 = 8;
const SYM_SIZE: usize = 24;
const SECTION_HEADER_SIZE: usize = 64;

/// The section index of an undefined symbol.
pub(super) const SHN_UNDEF: u16 = 0;
/// `STB_WEAK`: an undefined weak symbol may stay unresolved.
pub(super) const STB_WEAK: u8 = 2;
pub(super) const STT_SECTION: u8 = 3;
pub(super) const STT_FILE: u8 = 4;

struct SectionHeader<'a> {
    name: &'a str,
    kind: u32,
    offset: usize,
    size: usize,
    link: usize,
}

/// One `.symtab` entry, with its name as text. Entries whose name is not
/// UTF-8 are skipped by [`ElfObject::symbols`].
pub(super) struct Symbol<'a> {
    pub name: &'a str,
    pub bind: u8,
    pub kind: u8,
    pub shndx: u16,
}

pub(super) struct ElfObject<'a> {
    bytes: &'a [u8],
    sections: Vec<SectionHeader<'a>>,
}

impl<'a> ElfObject<'a> {
    /// Reads the section table of a relocatable ELF64 little-endian object.
    pub(super) fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.get(0..4) != Some(ELF_MAGIC.as_slice()) {
            return Err("not an ELF file".into());
        }
        if bytes.get(4) != Some(&2) {
            return Err("not a 64-bit ELF file".into());
        }
        if bytes.get(5) != Some(&1) {
            return Err("not a little-endian ELF file".into());
        }
        if read_u16(bytes, 16)? != ET_REL {
            return Err("not a relocatable object".into());
        }
        let section_table = read_offset(bytes, 0x28)?;
        let entry_size = usize::from(read_u16(bytes, 0x3a)?);
        let count = usize::from(read_u16(bytes, 0x3c)?);
        let names_index = usize::from(read_u16(bytes, 0x3e)?);
        if entry_size < SECTION_HEADER_SIZE {
            return Err("section header entries are too small".into());
        }
        let mut headers = Vec::with_capacity(count);
        for index in 0..count {
            let base = section_table
                .checked_add(
                    index
                        .checked_mul(entry_size)
                        .ok_or("section table overflows")?,
                )
                .ok_or("section table overflows")?;
            headers.push((
                read_u32(bytes, base)?,
                SectionHeader {
                    name: "",
                    kind: read_u32(bytes, base + 4)?,
                    offset: read_offset(bytes, base + 0x18)?,
                    size: read_offset(bytes, base + 0x20)?,
                    link: read_u32(bytes, base + 0x28)? as usize,
                },
            ));
        }
        let names = match headers.get(names_index) {
            Some((_, header)) => range(bytes, header.offset, header.size)?,
            None => &[],
        };
        let sections = headers
            .into_iter()
            .map(|(name_offset, mut header)| {
                header.name = cstr(names, name_offset as usize).unwrap_or("");
                header
            })
            .collect();
        Ok(Self { bytes, sections })
    }

    /// The contents of the first section called `name`. `None` when there is
    /// no such section, or it has no file contents (`SHT_NOBITS`).
    pub(super) fn section(&self, name: &str) -> Option<&'a [u8]> {
        let header = self
            .sections
            .iter()
            .find(|header| header.name == name && header.kind != SHT_NOBITS)?;
        range(self.bytes, header.offset, header.size).ok()
    }

    /// Every `.symtab` entry, in order. Entries with a name that is not UTF-8
    /// are left out. An object with no symbol table has none.
    pub(super) fn symbols(&self) -> Result<Vec<Symbol<'a>>, String> {
        let Some(table) = self
            .sections
            .iter()
            .find(|header| header.kind == SHT_SYMTAB)
        else {
            return Ok(Vec::new());
        };
        let strings = self
            .sections
            .get(table.link)
            .filter(|header| header.kind == SHT_STRTAB)
            .ok_or("symbol table has no string table")?;
        let strings = range(self.bytes, strings.offset, strings.size)?;
        let body = range(self.bytes, table.offset, table.size)?;
        let mut symbols = Vec::with_capacity(body.len() / SYM_SIZE);
        for entry in body.as_chunks::<SYM_SIZE>().0 {
            let Some(name) = cstr(strings, read_u32(entry, 0)? as usize) else {
                continue;
            };
            symbols.push(Symbol {
                name,
                bind: entry[4] >> 4,
                kind: entry[4] & 0xf,
                shndx: read_u16(entry, 6)?,
            });
        }
        Ok(symbols)
    }
}

/// `bytes[offset..offset + len]`, or an error when it runs past the end.
fn range(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], String> {
    offset
        .checked_add(len)
        .and_then(|end| bytes.get(offset..end))
        .ok_or_else(|| "section runs past the end of the file".to_string())
}

/// The NUL-terminated string at `offset` in `table`, when it is UTF-8.
pub(super) fn cstr(table: &[u8], offset: usize) -> Option<&str> {
    let rest = table.get(offset..)?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(rest.len());
    std::str::from_utf8(&rest[..end]).ok()
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, String> {
    let field = range(bytes, at, 2)?;
    Ok(u16::from_le_bytes([field[0], field[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, String> {
    let field = range(bytes, at, 4)?;
    Ok(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

/// A 64-bit file offset or size, as `usize`.
fn read_offset(bytes: &[u8], at: usize) -> Result<usize, String> {
    let field = range(bytes, at, 8)?;
    let mut array = [0u8; 8];
    array.copy_from_slice(field);
    usize::try_from(u64::from_le_bytes(array)).map_err(|_| "offset does not fit".to_string())
}
