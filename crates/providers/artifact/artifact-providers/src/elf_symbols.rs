//! The dynamic symbol table (`.dynsym`) of an ELF shared library or program:
//! the names it exports to the loader and the names it imports. Read from the
//! section table, which `strip` keeps. Every read is bounds-checked, so a
//! truncated or hostile file is an error, never a panic.

use std::collections::BTreeSet;

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const SHT_DYNSYM: u32 = 11;
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
        return Ok(DynamicSymbols::default());
    }
    if shentsize < if wide { 64 } else { 40 } {
        return Err("ELF section headers are too small".into());
    }
    let header = |index: usize| -> Result<Section, String> {
        let base = index
            .checked_mul(shentsize)
            .and_then(|offset| offset.checked_add(shoff))
            .ok_or("ELF section table out of range")?;
        Ok(if wide {
            Section {
                kind: read.u32(base + 4)?,
                offset: read.word(base + 0x18)?,
                size: read.word(base + 0x20)?,
                link: read.u32(base + 0x28)? as usize,
            }
        } else {
            Section {
                kind: read.u32(base + 4)?,
                offset: read.word(base + 0x10)?,
                size: read.word(base + 0x14)?,
                link: read.u32(base + 0x18)? as usize,
            }
        })
    };
    let mut table = None;
    for index in 0..usize::from(shnum) {
        let section = header(index)?;
        if section.kind == SHT_DYNSYM {
            table = Some(section);
            break;
        }
    }
    let Some(table) = table else {
        return Ok(DynamicSymbols::default());
    };
    if table.link >= usize::from(shnum) {
        return Err("ELF .dynsym names no string table".into());
    }
    let strings = header(table.link)?;
    let strings = range(bytes, strings.offset, strings.size)?;
    let body = range(bytes, table.offset, table.size)?;
    let entry_size = if wide { 24 } else { 16 };
    let mut symbols = DynamicSymbols::default();
    // Entry 0 is the null symbol.
    for entry in body.chunks_exact(entry_size).skip(1) {
        let entry_read = Reader {
            bytes: entry,
            wide,
            little,
        };
        let name_offset = entry_read.u32(0)? as usize;
        let (info, other, shndx) = if wide {
            (entry[4], entry[5], entry_read.u16(6)?)
        } else {
            (entry[12], entry[13], entry_read.u16(14)?)
        };
        let (bind, kind, visibility) = (info >> 4, info & 0xf, other & 0x3);
        if !matches!(bind, STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
            || matches!(kind, STT_SECTION | STT_FILE)
        {
            continue;
        }
        let name = name_at(strings, name_offset)?;
        if name.is_empty() {
            continue;
        }
        if shndx == SHN_UNDEF {
            symbols.imports.push(DynamicImport {
                name,
                weak: bind == STB_WEAK,
            });
        } else if shndx != SHN_ABS && !matches!(visibility, STV_HIDDEN | STV_INTERNAL) {
            symbols.exports.insert(name);
        }
    }
    symbols.imports.sort();
    symbols.imports.dedup();
    Ok(symbols)
}

struct Section {
    kind: u32,
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
        for (name, info, other, shndx) in symbols {
            let offset = strings.len() as u32;
            strings.extend(name.as_bytes());
            strings.push(0);
            put32(&mut entries, offset);
            if wide {
                entries.extend([*info, *other]);
                put16(&mut entries, *shndx);
                entries.extend([0u8; 16]);
            } else {
                entries.extend([0u8; 8]);
                entries.extend([*info, *other]);
                put16(&mut entries, *shndx);
            }
        }
        let header_size = if wide { 64 } else { 52 };
        let section_size = if wide { 64 } else { 40 };
        let entries_at = header_size;
        let strings_at = entries_at + entries.len();
        let table_at = strings_at + strings.len();
        let mut out = vec![0x7f, b'E', b'L', b'F', if wide { 2 } else { 1 }];
        out.push(if little { 1 } else { 2 });
        out.resize(16, 0);
        put16(&mut out, 3);
        put16(&mut out, 0xb7);
        out.resize(if wide { 0x28 } else { 0x20 }, 0);
        put_word(&mut out, table_at);
        out.resize(if wide { 0x3a } else { 0x2e }, 0);
        put16(&mut out, section_size as u16);
        put16(&mut out, 3);
        out.resize(header_size, 0);
        out.extend(&entries);
        out.extend(&strings);
        // Sections: null, .dynsym (link 2), .dynstr.
        let mut section = |kind: u32, offset: usize, size: usize, link: u32| {
            let start = out.len();
            put32(&mut out, 0);
            put32(&mut out, kind);
            if wide {
                out.extend([0u8; 16]);
            } else {
                out.extend([0u8; 8]);
            }
            put_word(&mut out, offset);
            put_word(&mut out, size);
            put32(&mut out, link);
            out.resize(start + section_size, 0);
        };
        section(0, 0, 0, 0);
        section(SHT_DYNSYM, entries_at, entries.len(), 2);
        section(3, strings_at, strings.len(), 0);
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
}
