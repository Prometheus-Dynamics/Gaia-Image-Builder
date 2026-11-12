//! A small ELF reader for the binary check: the program interpreter and the
//! `DT_NEEDED` names of a dynamically linked file.
//!
//! It reads bytes only and runs nothing. ELF32 and ELF64, little and big
//! endian. Every read is bounds-checked, so a truncated or corrupt file is an
//! error, never a panic.

const ELF_MAGIC: &[u8] = b"\x7fELF";
const SHT_DYNAMIC: u32 = 6;
const PT_INTERP: u32 = 3;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;

/// What the check needs from one ELF file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elf {
    /// The program interpreter, for a dynamically linked program.
    pub interpreter: Option<String>,
    /// `DT_NEEDED` names, in file order.
    pub needed: Vec<String>,
}

/// The parsed file, or `Ok(None)` when the bytes are not an ELF file.
pub fn parse(bytes: &[u8]) -> Result<Option<Elf>, String> {
    if !bytes.starts_with(ELF_MAGIC) {
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
        let at = offset(phoff, index, phentsize)?;
        let (ty, offset, size) = if is64 {
            (r.u32(at)?, r.u64(add(at, 8)?)?, r.u64(add(at, 32)?)?)
        } else {
            (
                r.u32(at)?,
                u64::from(r.u32(add(at, 4)?)?),
                u64::from(r.u32(add(at, 16)?)?),
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
        let at = offset(shoff, index, shentsize)?;
        sections.push(if is64 {
            Section {
                kind: r.u32(add(at, 4)?)?,
                offset: r.u64(add(at, 24)?)?,
                size: r.u64(add(at, 32)?)?,
                link: r.u32(add(at, 40)?)?,
            }
        } else {
            Section {
                kind: r.u32(add(at, 4)?)?,
                offset: u64::from(r.u32(add(at, 16)?)?),
                size: u64::from(r.u32(add(at, 20)?)?),
                link: r.u32(add(at, 24)?)?,
            }
        });
    }

    let mut needed = Vec::new();
    if let Some(dynamic) = sections.iter().find(|s| s.kind == SHT_DYNAMIC) {
        let strtab = sections
            .get(dynamic.link as usize)
            .ok_or("the dynamic section names a missing string table")?;
        let entry: u64 = if is64 { 16 } else { 8 };
        let mut at = dynamic.offset;
        let end = dynamic
            .offset
            .checked_add(dynamic.size)
            .ok_or("the dynamic section is out of range")?;
        while add(at, entry)? <= end {
            let (tag, value) = if is64 {
                (r.u64(at)?, r.u64(add(at, 8)?)?)
            } else {
                (u64::from(r.u32(at)?), u64::from(r.u32(add(at, 4)?)?))
            };
            if tag == DT_NULL {
                break;
            }
            if tag == DT_NEEDED {
                let at = strtab
                    .offset
                    .checked_add(value)
                    .ok_or("a library name is out of range")?;
                needed.push(r.cstring(at, strtab.size.saturating_sub(value))?);
            }
            at = add(at, entry)?;
        }
    }
    Ok(Some(Elf {
        interpreter,
        needed,
    }))
}

/// `base` plus `index` entries of `stride` bytes, or an error when that
/// runs past the end of the address space.
fn offset(base: u64, index: usize, stride: u64) -> Result<u64, String> {
    let index = u64::try_from(index).map_err(|_| "offset out of range")?;
    let skip = index.checked_mul(stride).ok_or("offset out of range")?;
    add(base, skip)
}

fn add(base: u64, extra: u64) -> Result<u64, String> {
    base.checked_add(extra)
        .ok_or_else(|| "offset out of range".to_string())
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
        let b: [u8; 2] = self.slice(at, 2)?.try_into().map_err(|_| "truncated")?;
        Ok(if self.big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }

    fn u32(&self, at: u64) -> Result<u32, String> {
        let b: [u8; 4] = self.slice(at, 4)?.try_into().map_err(|_| "truncated")?;
        Ok(if self.big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    fn u64(&self, at: u64) -> Result<u64, String> {
        let b: [u8; 8] = self.slice(at, 8)?.try_into().map_err(|_| "truncated")?;
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
