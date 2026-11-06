//! The dynamic symbol table (`.dynsym`) of an ELF shared library or program:
//! the names it exports to the loader and the names it imports. Read from the
//! section table, which `strip` keeps. Every read is bounds-checked, so a
//! truncated or hostile file is an error, never a panic.

use std::collections::BTreeSet;

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const SHT_DYNSYM: u32 = 11;
const SHT_NOBITS: u32 = 8;
const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const STB_GLOBAL: u8 = 1;
const STB_WEAK: u8 = 2;
const STB_GNU_UNIQUE: u8 = 10;
const STT_SECTION: u8 = 3;
const STT_FILE: u8 = 4;
const STV_HIDDEN: u8 = 2;
const STV_INTERNAL: u8 = 1;

/// One name the object needs from the libraries it loads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DynamicImport {
    pub name: String,
    /// A weak import may stay unresolved; the program checks for it.
    pub weak: bool,
}

/// What an object's `.dynsym` says it exports and imports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynamicSymbols {
    /// Defined global, weak or unique symbols other programs can bind to.
    /// Absolute symbols are left out: the linker adds one per version node
    /// of a version script (`LIBFOO_1`), and they are no code or data.
    pub exports: BTreeSet<String>,
    /// Undefined global or weak symbols, sorted.
    pub imports: Vec<DynamicImport>,
}

/// Reads the dynamic symbols of an ELF object, 32-bit or 64-bit, either byte
/// order. An object without a `.dynsym` (a static program) has none.
pub fn dynamic_symbols(bytes: &[u8]) -> Result<DynamicSymbols, String> {
    let mut symbols = DynamicSymbols::default();
    for symbol in entries(bytes)? {
        if !matches!(symbol.bind, STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
            || matches!(symbol.kind, STT_SECTION | STT_FILE)
            || symbol.name.is_empty()
        {
            continue;
        }
        if symbol.shndx == SHN_UNDEF {
            symbols.imports.push(DynamicImport {
                name: symbol.name,
                weak: symbol.bind == STB_WEAK,
            });
        } else if symbol.shndx != SHN_ABS && !matches!(symbol.visibility, STV_HIDDEN | STV_INTERNAL)
        {
            symbols.exports.insert(symbol.name);
        }
    }
    symbols.imports.sort();
    symbols.imports.dedup();
    Ok(symbols)
}

/// The value of an exported 4-byte constant (a `u32` in `.rodata`), read
/// from the file in its byte order: `None` when the object does not
/// export `name`. A symbol of another size, or one without file contents
/// (`.bss`), is an error.
pub fn exported_u32(bytes: &[u8], name: &str) -> Result<Option<u32>, String> {
    let Some(symbol) = entries(bytes)?.into_iter().find(|symbol| {
        symbol.name == name
            && matches!(symbol.bind, STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
            && !matches!(symbol.shndx, SHN_UNDEF | SHN_ABS)
    }) else {
        return Ok(None);
    };
    if symbol.size != 4 {
        return Err(format!(
            "{name} is {} bytes, not a 4-byte value",
            symbol.size
        ));
    }
    let (read, sections) = section_table(bytes)?;
    let section = sections
        .get(usize::from(symbol.shndx))
        .ok_or_else(|| format!("{name} is in no section of the file"))?;
    if section.kind == SHT_NOBITS {
        return Err(format!("{name} has no contents in the file (.bss)"));
    }
    let offset = symbol
        .value
        .checked_sub(section.addr)
        .filter(|offset| offset.checked_add(4).is_some_and(|end| end <= section.size))
        .and_then(|offset| offset.checked_add(section.offset))
        .ok_or_else(|| format!("{name} lies outside its section"))?;
    read.u32(offset).map(Some)
}

struct Symbol {
    name: String,
    bind: u8,
    kind: u8,
    visibility: u8,
    shndx: u16,
    value: usize,
    size: usize,
}

/// Every `.dynsym` entry but the null one; none without a `.dynsym`.
fn entries(bytes: &[u8]) -> Result<Vec<Symbol>, String> {
    let (read, sections) = section_table(bytes)?;
    let Some(table) = sections.iter().find(|section| section.kind == SHT_DYNSYM) else {
        return Ok(Vec::new());
    };
    let strings = sections
        .get(table.link)
        .ok_or("ELF .dynsym names no string table")?;
    let strings = range(bytes, strings.offset, strings.size)?;
    let body = range(bytes, table.offset, table.size)?;
    let wide = read.wide;
    let entry_size = if wide { 24 } else { 16 };
    let mut symbols = Vec::new();
    // Entry 0 is the null symbol.
    for entry in body.chunks_exact(entry_size).skip(1) {
        let entry_read = Reader {
            bytes: entry,
            wide,
            little: read.little,
        };
        let (info, other, shndx, value, size) = if wide {
            (
                entry[4],
                entry[5],
                entry_read.u16(6)?,
                entry_read.word(8)?,
                entry_read.word(16)?,
            )
        } else {
            (
                entry[12],
                entry[13],
                entry_read.u16(14)?,
                entry_read.word(4)?,
                entry_read.word(8)?,
            )
        };
        symbols.push(Symbol {
            name: name_at(strings, entry_read.u32(0)? as usize)?,
            bind: info >> 4,
            kind: info & 0xf,
            visibility: other & 0x3,
            shndx,
            value,
            size,
        });
    }
    Ok(symbols)
}

/// The section headers, read from the section table (`strip` keeps it).
fn section_table(bytes: &[u8]) -> Result<(Reader<'_>, Vec<Section>), String> {
    if bytes.get(..4) != Some(ELF_MAGIC.as_slice()) {
        return Err("not an ELF file".into());
    }
    let wide = match bytes.get(4) {
        Some(2) => true,
        Some(1) => false,
        _ => return Err("unknown ELF class".into()),
    };
    let little = match bytes.get(5) {
        Some(1) => true,
        Some(2) => false,
        _ => return Err("unknown ELF data encoding".into()),
    };
    let read = Reader {
        bytes,
        wide,
        little,
    };
    let (shoff, shentsize, shnum) = if wide {
        (read.word(0x28)?, read.u16(0x3a)?, read.u16(0x3c)?)
    } else {
        (read.word(0x20)?, read.u16(0x2e)?, read.u16(0x30)?)
    };
    let shentsize = usize::from(shentsize);
    if shoff == 0 || shnum == 0 {
        return Ok((read, Vec::new()));
    }
    if shentsize < if wide { 64 } else { 40 } {
        return Err("ELF section headers are too small".into());
    }
    let mut sections = Vec::with_capacity(usize::from(shnum));
    for index in 0..usize::from(shnum) {
        let base = index
            .checked_mul(shentsize)
            .and_then(|offset| offset.checked_add(shoff))
            .ok_or("ELF section table out of range")?;
        sections.push(if wide {
            Section {
                kind: read.u32(base + 4)?,
                addr: read.word(base + 0x10)?,
                offset: read.word(base + 0x18)?,
                size: read.word(base + 0x20)?,
                link: read.u32(base + 0x28)? as usize,
            }
        } else {
            Section {
                kind: read.u32(base + 4)?,
                addr: read.word(base + 0x0c)?,
                offset: read.word(base + 0x10)?,
                size: read.word(base + 0x14)?,
                link: read.u32(base + 0x18)? as usize,
            }
        });
    }
    Ok((read, sections))
}

struct Section {
    kind: u32,
    addr: usize,
    offset: usize,
    size: usize,
    link: usize,
}

struct Reader<'a> {
    bytes: &'a [u8],
    wide: bool,
    little: bool,
}

impl Reader<'_> {
    fn field<const N: usize>(&self, at: usize) -> Result<[u8; N], String> {
        range(self.bytes, at, N).map(|slice| slice.try_into().expect("length checked"))
    }

    fn u16(&self, at: usize) -> Result<u16, String> {
        let raw = self.field::<2>(at)?;
        Ok(if self.little {
            u16::from_le_bytes(raw)
        } else {
            u16::from_be_bytes(raw)
        })
    }

    fn u32(&self, at: usize) -> Result<u32, String> {
        let raw = self.field::<4>(at)?;
        Ok(if self.little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    }

    /// An offset or size: 8 bytes in ELF64, 4 in ELF32.
    fn word(&self, at: usize) -> Result<usize, String> {
        let value = if self.wide {
            let raw = self.field::<8>(at)?;
            if self.little {
                u64::from_le_bytes(raw)
            } else {
                u64::from_be_bytes(raw)
            }
        } else {
            u64::from(self.u32(at)?)
        };
        usize::try_from(value).map_err(|_| "ELF offset out of range".to_string())
    }
}

fn range(bytes: &[u8], start: usize, len: usize) -> Result<&[u8], String> {
    start
        .checked_add(len)
        .and_then(|end| bytes.get(start..end))
        .ok_or_else(|| "truncated ELF file".to_string())
}

fn name_at(table: &[u8], offset: usize) -> Result<String, String> {
    let rest = table
        .get(offset..)
        .ok_or_else(|| "ELF symbol name out of range".to_string())?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| "unterminated ELF symbol name".to_string())?;
    String::from_utf8(rest[..end].to_vec()).map_err(|_| "ELF symbol name is not UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ELF64 (or ELF32) object with a `.dynsym` of `symbols`
    /// (name, info, other, shndx) and its string table.
    fn object(wide: bool, little: bool, symbols: &[(&str, u8, u8, u16)]) -> Vec<u8> {
        let symbols = symbols
            .iter()
            .map(|&(name, info, other, shndx)| (name, info, other, shndx, 0, 0))
            .collect::<Vec<_>>();
        object_with(wide, little, &symbols, RODATA_ADDR, &[])
    }

    /// Where the fixture's `.rodata` (section 3) is loaded.
    const RODATA_ADDR: usize = 0x4000;

    /// `object`, with each symbol's value and size, and a `.rodata` section
    /// (index 3) of `rodata` loaded at `rodata_addr`.
    fn object_with(
        wide: bool,
        little: bool,
        symbols: &[(&str, u8, u8, u16, usize, usize)],
        rodata_addr: usize,
        rodata: &[u8],
    ) -> Vec<u8> {
        let mut strings = vec![0u8];
        let mut entries = vec![0u8; if wide { 24 } else { 16 }];
        let put16 = |out: &mut Vec<u8>, value: u16| {
            out.extend(if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            })
        };
        let put32 = |out: &mut Vec<u8>, value: u32| {
            out.extend(if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            })
        };
        let put_word = |out: &mut Vec<u8>, value: usize| {
            if wide {
                let value = value as u64;
                out.extend(if little {
                    value.to_le_bytes()
                } else {
                    value.to_be_bytes()
                });
            } else {
                put32(out, value as u32);
            }
        };
        for (name, info, other, shndx, value, size) in symbols {
            let offset = strings.len() as u32;
            strings.extend(name.as_bytes());
            strings.push(0);
            put32(&mut entries, offset);
            if wide {
                entries.extend([*info, *other]);
                put16(&mut entries, *shndx);
                put_word(&mut entries, *value);
                put_word(&mut entries, *size);
            } else {
                put_word(&mut entries, *value);
                put_word(&mut entries, *size);
                entries.extend([*info, *other]);
                put16(&mut entries, *shndx);
            }
        }
        let header_size = if wide { 64 } else { 52 };
        let section_size = if wide { 64 } else { 40 };
        let entries_at = header_size;
        let strings_at = entries_at + entries.len();
        let rodata_at = strings_at + strings.len();
        let table_at = rodata_at + rodata.len();
        let mut out = vec![0x7f, b'E', b'L', b'F', if wide { 2 } else { 1 }];
        out.push(if little { 1 } else { 2 });
        out.resize(16, 0);
        put16(&mut out, 3);
        put16(&mut out, 0xb7);
        out.resize(if wide { 0x28 } else { 0x20 }, 0);
        put_word(&mut out, table_at);
        out.resize(if wide { 0x3a } else { 0x2e }, 0);
        put16(&mut out, section_size as u16);
        put16(&mut out, 4);
        out.resize(header_size, 0);
        out.extend(&entries);
        out.extend(&strings);
        out.extend(rodata);
        // Sections: null, .dynsym (link 2), .dynstr.
        let mut section = |kind: u32, addr: usize, offset: usize, size: usize, link: u32| {
            let start = out.len();
            put32(&mut out, 0);
            put32(&mut out, kind);
            if wide {
                out.extend([0u8; 8]);
            } else {
                out.extend([0u8; 4]);
            }
            put_word(&mut out, addr);
            put_word(&mut out, offset);
            put_word(&mut out, size);
            put32(&mut out, link);
            out.resize(start + section_size, 0);
        };
        section(0, 0, 0, 0, 0);
        section(SHT_DYNSYM, 0, entries_at, entries.len(), 2);
        section(3, 0, strings_at, strings.len(), 0);
        section(1, rodata_addr, rodata_at, rodata.len(), 0);
        out
    }

    const GLOBAL_FUNC: u8 = (STB_GLOBAL << 4) | 2;
    const WEAK_FUNC: u8 = (STB_WEAK << 4) | 2;
    const LOCAL_FUNC: u8 = 2;

    fn sample(wide: bool, little: bool) -> Vec<u8> {
        object(
            wide,
            little,
            &[
                ("surface_open", GLOBAL_FUNC, 0, 12),
                ("SURFACE_1", (STB_GLOBAL << 4) | 1, 0, SHN_ABS),
                ("hidden_one", GLOBAL_FUNC, STV_HIDDEN, 12),
                ("local_one", LOCAL_FUNC, 0, 12),
                ("malloc", GLOBAL_FUNC, 0, SHN_UNDEF),
                ("__gmon_start__", WEAK_FUNC, 0, SHN_UNDEF),
                ("weak_export", WEAK_FUNC, 0, 12),
            ],
        )
    }

    #[test]
    fn exports_and_imports_are_read_in_every_class_and_byte_order() {
        for (wide, little) in [(true, true), (true, false), (false, true), (false, false)] {
            let symbols = dynamic_symbols(&sample(wide, little)).expect("parse");
            assert_eq!(
                symbols.exports.iter().collect::<Vec<_>>(),
                ["surface_open", "weak_export"],
                "wide {wide} little {little}"
            );
            assert_eq!(
                symbols.imports,
                [
                    DynamicImport {
                        name: "__gmon_start__".into(),
                        weak: true
                    },
                    DynamicImport {
                        name: "malloc".into(),
                        weak: false
                    },
                ]
            );
        }
    }

    #[test]
    fn a_truncated_object_is_an_error_and_not_elf_is_rejected() {
        let bytes = sample(true, true);
        // The string table's section header loses its offset and size.
        assert!(dynamic_symbols(&bytes[..bytes.len() - 40]).is_err());
        assert!(dynamic_symbols(b"#!/bin/sh\n").is_err());
    }

    #[test]
    fn an_object_without_dynsym_has_no_symbols() {
        let mut bytes = sample(true, true);
        // Zero the section count.
        bytes[0x3c] = 0;
        bytes[0x3d] = 0;
        assert_eq!(dynamic_symbols(&bytes), Ok(DynamicSymbols::default()));
    }

    #[test]
    fn an_exported_constant_is_read_in_every_class_and_byte_order() {
        const GLOBAL_OBJECT: u8 = (STB_GLOBAL << 4) | 1;
        for (wide, little) in [(true, true), (true, false), (false, true), (false, false)] {
            let mut rodata = vec![0xaa; 4];
            rodata.extend(if little {
                4u32.to_le_bytes()
            } else {
                4u32.to_be_bytes()
            });
            let at = RODATA_ADDR + 4;
            let bytes = object_with(
                wide,
                little,
                &[
                    ("plugin_version", GLOBAL_OBJECT, 0, 3, at, 4),
                    ("plugin_wide", GLOBAL_OBJECT, 0, 3, at, 8),
                    ("plugin_past", GLOBAL_OBJECT, 0, 3, at + 2, 4),
                    ("plugin_import", GLOBAL_OBJECT, 0, SHN_UNDEF, 0, 0),
                ],
                RODATA_ADDR,
                &rodata,
            );
            assert_eq!(exported_u32(&bytes, "plugin_version"), Ok(Some(4)));
            assert_eq!(exported_u32(&bytes, "plugin_import"), Ok(None));
            assert_eq!(exported_u32(&bytes, "absent"), Ok(None));
            assert!(exported_u32(&bytes, "plugin_wide").is_err());
            assert!(exported_u32(&bytes, "plugin_past").is_err());
        }
    }
}
