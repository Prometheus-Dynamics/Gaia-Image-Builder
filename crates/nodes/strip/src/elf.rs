//! A small ELF reader: what the strip and library passes need from one file.
//!
//! It reads bytes only and runs nothing. ELF32 and ELF64, little and big
//! endian. Every read is bounds-checked, so a truncated or corrupt file is an
//! error, never a panic.

/// `e_type` values the passes tell apart.
pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

const SHT_DYNAMIC: u32 = 6;
const PT_INTERP: u32 = 3;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_SONAME: u64 = 14;

/// What the passes need from one ELF file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elf {
    /// `e_type`: relocatable, executable, shared object, core.
    pub kind: u16,
    /// `e_machine`: the architecture.
    pub machine: u16,
    /// The program interpreter, for a dynamically linked program.
    pub interpreter: Option<String>,
    /// `DT_NEEDED` names, in file order.
    pub needed: Vec<String>,
    /// `DT_SONAME`, when the object sets one.
    pub soname: Option<String>,
}

/// The ELF file's contents, or `Ok(None)` when the bytes are not ELF.
pub fn parse(bytes: &[u8]) -> Result<Option<Elf>, String> {
    if !bytes.starts_with(b"\x7fELF") {
        return Ok(None);
    }
    let is64 = match bytes.get(4) {
        Some(1) => false,
        Some(2) => true,
        _ => return Err("unknown ELF class".into()),
    };
    let big = match bytes.get(5) {
        Some(1) => false,
        Some(2) => true,
        _ => return Err("unknown ELF byte order".into()),
    };
    let r = Reader { bytes, big };
    let kind = r.u16(16)?;
    let machine = r.u16(18)?;
    let (phoff, shoff, phentsize, phnum, shentsize, shnum) = if is64 {
        (
            r.u64(32)?,
            r.u64(40)?,
            u64::from(r.u16(54)?),
            usize::from(r.u16(56)?),
            u64::from(r.u16(58)?),
            usize::from(r.u16(60)?),
        )
    } else {
        (
            u64::from(r.u32(28)?),
            u64::from(r.u32(32)?),
            u64::from(r.u16(42)?),
            usize::from(r.u16(44)?),
            u64::from(r.u16(46)?),
            usize::from(r.u16(48)?),
        )
    };

    let mut interpreter = None;
    for index in 0..phnum {
        let at = phoff + index as u64 * phentsize;
        let (ty, offset, size) = if is64 {
            (r.u32(at)?, r.u64(at + 8)?, r.u64(at + 32)?)
        } else {
            (
                r.u32(at)?,
                u64::from(r.u32(at + 4)?),
                u64::from(r.u32(at + 16)?),
            )
        };
        if ty == PT_INTERP {
            interpreter = Some(r.cstring(offset, size)?);
        }
    }

    let mut sections = Vec::with_capacity(shnum);
    let shentsize_min = if is64 { 64 } else { 40 };
    if shentsize < shentsize_min && shnum > 0 {
        return Err("section header entries are too small".into());
    }
    for index in 0..shnum {
        let at = shoff + index as u64 * shentsize;
        sections.push(if is64 {
            Section {
                kind: r.u32(at + 4)?,
                offset: r.u64(at + 24)?,
                size: r.u64(at + 32)?,
                link: r.u32(at + 40)?,
            }
        } else {
            Section {
                kind: r.u32(at + 4)?,
                offset: u64::from(r.u32(at + 16)?),
                size: u64::from(r.u32(at + 20)?),
                link: r.u32(at + 24)?,
            }
        });
    }

    let mut needed = Vec::new();
    let mut soname = None;
    if let Some(dynamic) = sections.iter().find(|s| s.kind == SHT_DYNAMIC) {
        let strtab = sections
            .get(dynamic.link as usize)
            .ok_or("the dynamic section names a missing string table")?;
        let entry = if is64 { 16 } else { 8 };
        let mut at = dynamic.offset;
        let end = dynamic.offset.checked_add(dynamic.size).ok_or("overflow")?;
        while at + entry as u64 <= end {
            let (tag, value) = if is64 {
                (r.u64(at)?, r.u64(at + 8)?)
            } else {
                (u64::from(r.u32(at)?), u64::from(r.u32(at + 4)?))
            };
            if tag == DT_NULL {
                break;
            }
            if tag == DT_NEEDED || tag == DT_SONAME {
                let text = r.cstring(
                    strtab.offset.checked_add(value).ok_or("overflow")?,
                    strtab.size.saturating_sub(value),
                )?;
                if tag == DT_NEEDED {
                    needed.push(text);
                } else {
                    soname = Some(text);
                }
            }
            at += entry as u64;
        }
    }
    Ok(Some(Elf {
        kind,
        machine,
        interpreter,
        needed,
        soname,
    }))
}

struct Section {
    kind: u32,
    offset: u64,
    size: u64,
    link: u32,
}

struct Reader<'a> {
    bytes: &'a [u8],
    big: bool,
}

impl Reader<'_> {
    fn slice(&self, at: u64, len: usize) -> Result<&[u8], String> {
        let start = usize::try_from(at).map_err(|_| "offset out of range")?;
        let end = start.checked_add(len).ok_or("offset out of range")?;
        self.bytes
            .get(start..end)
            .ok_or_else(|| "truncated ELF file".to_string())
    }

    fn u16(&self, at: u64) -> Result<u16, String> {
        let b: [u8; 2] = self.slice(at, 2)?.try_into().expect("two bytes");
        Ok(if self.big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }

    fn u32(&self, at: u64) -> Result<u32, String> {
        let b: [u8; 4] = self.slice(at, 4)?.try_into().expect("four bytes");
        Ok(if self.big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    fn u64(&self, at: u64) -> Result<u64, String> {
        let b: [u8; 8] = self.slice(at, 8)?.try_into().expect("eight bytes");
        Ok(if self.big {
            u64::from_be_bytes(b)
        } else {
            u64::from_le_bytes(b)
        })
    }

    /// The NUL-terminated string at `at`, within `limit` bytes.
    fn cstring(&self, at: u64, limit: u64) -> Result<String, String> {
        let limit = usize::try_from(limit).map_err(|_| "string out of range")?;
        let bytes = self.slice(at, limit.min(self.bytes.len()))?;
        let end = bytes
            .iter()
            .position(|b| *b == 0)
            .ok_or("an unterminated string in an ELF file")?;
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}
