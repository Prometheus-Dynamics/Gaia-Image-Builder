//! Tests for the ELF reader, and the builder that writes small ELF objects
//! for the runtime library tests.

use super::*;

/// Machine numbers used by the fixtures.
pub(crate) const EM_X86_64: u16 = 62;
pub(crate) const EM_AARCH64: u16 = 183;

/// What a fixture object contains.
#[derive(Debug, Clone)]
pub(crate) struct ElfFixture<'a> {
    pub is_64bit: bool,
    pub little_endian: bool,
    pub machine: u16,
    pub interpreter: Option<&'a str>,
    pub needed: &'a [&'a str],
    pub runpath: Option<&'a str>,
}

impl<'a> ElfFixture<'a> {
    pub(crate) fn dynamic(machine: u16, needed: &'a [&'a str]) -> Self {
        Self {
            is_64bit: true,
            little_endian: true,
            machine,
            interpreter: None,
            needed,
            runpath: None,
        }
    }
}

/// Virtual address of file offset 0: the single PT_LOAD segment maps the
/// whole file here, so reading DT_STRTAB exercises the address mapping.
const VADDR_BASE: u64 = 0x40_0000;

/// Builds an ELF object with a PT_LOAD over the whole file, a PT_DYNAMIC, an
/// optional PT_INTERP, and the dynamic entries the fixture asks for.
pub(crate) fn build_elf(fixture: &ElfFixture<'_>) -> Vec<u8> {
    let is_64 = fixture.is_64bit;
    let little = fixture.little_endian;
    let word = if is_64 { 8 } else { 4 };
    let ehsize = if is_64 { 64 } else { 52 };
    let phentsize = if is_64 { 56 } else { 32 };
    let dynentsize = 2 * word;

    let mut strtab = vec![0u8];
    let mut name_offsets = Vec::new();
    for name in fixture.needed {
        name_offsets.push(strtab.len() as u64);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
    }
    let runpath_offset = fixture.runpath.map(|path| {
        let offset = strtab.len() as u64;
        strtab.extend_from_slice(path.as_bytes());
        strtab.push(0);
        offset
    });

    let mut dynamic: Vec<(u64, u64)> = name_offsets.iter().map(|offset| (1, *offset)).collect();
    if let Some(offset) = runpath_offset {
        dynamic.push((29, offset));
    }
    let interp_bytes = fixture
        .interpreter
        .map(|path| format!("{path}\0").into_bytes());
    let phnum = 2 + usize::from(interp_bytes.is_some());

    let phoff = ehsize as u64;
    let dynamic_offset = phoff + (phnum * phentsize) as u64;
    // The entries, plus the DT_STRTAB and DT_NULL terminators.
    let dynamic_size = ((dynamic.len() + 2) * dynentsize) as u64;
    let strtab_offset = dynamic_offset + dynamic_size;
    let interp_offset = strtab_offset + strtab.len() as u64;
    let total = interp_offset + interp_bytes.as_ref().map_or(0, |bytes| bytes.len()) as u64;
    let vaddr = |offset: u64| VADDR_BASE + offset;

    let mut out = vec![0u8; total as usize];
    let put = |out: &mut Vec<u8>, at: u64, value: u64, width: usize| {
        let bytes = if little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        };
        let slice: &[u8] = if little {
            &bytes[..width]
        } else {
            &bytes[8 - width..]
        };
        out[at as usize..at as usize + width].copy_from_slice(slice);
    };

    // ELF header.
    out[..4].copy_from_slice(b"\x7fELF");
    out[4] = if is_64 { 2 } else { 1 };
    out[5] = if little { 1 } else { 2 };
    out[6] = 1;
    put(&mut out, 16, 3, 2); // e_type: ET_DYN
    put(&mut out, 18, u64::from(fixture.machine), 2);
    put(&mut out, 20, 1, 4); // e_version
    if is_64 {
        put(&mut out, 32, phoff, 8);
        put(&mut out, 52, ehsize as u64, 2);
        put(&mut out, 54, phentsize as u64, 2);
        put(&mut out, 56, phnum as u64, 2);
    } else {
        put(&mut out, 28, phoff, 4);
        put(&mut out, 40, ehsize as u64, 2);
        put(&mut out, 42, phentsize as u64, 2);
        put(&mut out, 44, phnum as u64, 2);
    }

    // Program headers: (type, offset, filesz) for each, vaddr = vaddr(offset).
    let mut headers = vec![(1u64, 0u64, total)];
    headers.push((2, dynamic_offset, dynamic_size));
    if let Some(bytes) = &interp_bytes {
        headers.push((3, interp_offset, bytes.len() as u64));
    }
    for (index, (p_type, offset, filesz)) in headers.iter().enumerate() {
        let at = phoff as usize + index * phentsize;
        let base = at as u64;
        put(&mut out, base, *p_type, 4);
        let vaddr_value = if *p_type == 1 {
            VADDR_BASE
        } else {
            vaddr(*offset)
        };
        if is_64 {
            put(&mut out, base + 8, *offset, 8);
            put(&mut out, base + 16, vaddr_value, 8);
            put(&mut out, base + 24, vaddr_value, 8);
            put(&mut out, base + 32, *filesz, 8);
            put(&mut out, base + 40, *filesz, 8);
        } else {
            put(&mut out, base + 4, *offset, 4);
            put(&mut out, base + 8, vaddr_value, 4);
            put(&mut out, base + 12, vaddr_value, 4);
            put(&mut out, base + 16, *filesz, 4);
            put(&mut out, base + 20, *filesz, 4);
        }
    }

    // Dynamic section.
    let mut entries = dynamic.clone();
    entries.push((5, vaddr(strtab_offset)));
    entries.push((0, 0));
    for (index, (tag, value)) in entries.iter().enumerate() {
        let at = dynamic_offset + (index * dynentsize) as u64;
        put(&mut out, at, *tag, word);
        put(&mut out, at + word as u64, *value, word);
    }

    let strtab_at = strtab_offset as usize;
    out[strtab_at..strtab_at + strtab.len()].copy_from_slice(&strtab);
    if let Some(bytes) = interp_bytes {
        let at = interp_offset as usize;
        out[at..at + bytes.len()].copy_from_slice(&bytes);
    }
    out
}

#[test]
fn parses_dynamic_object_in_all_class_and_byte_order_combinations() {
    for is_64bit in [false, true] {
        for little_endian in [false, true] {
            let bytes = build_elf(&ElfFixture {
                is_64bit,
                little_endian,
                machine: EM_AARCH64,
                interpreter: Some("/lib/ld-test.so.1"),
                needed: &["libc.so.6", "libm.so.6"],
                runpath: Some("$ORIGIN/../usr/lib:/opt/lib"),
            });
            let object = parse_elf(&bytes).expect("fixture parses");
            assert_eq!(object.is_64bit, is_64bit);
            assert_eq!(object.little_endian, little_endian);
            assert_eq!(object.machine, EM_AARCH64);
            assert_eq!(object.interpreter.as_deref(), Some("/lib/ld-test.so.1"));
            assert_eq!(object.needed, vec!["libc.so.6", "libm.so.6"]);
            assert_eq!(
                object.search_paths,
                vec!["$ORIGIN/../usr/lib", "/opt/lib"],
                "is_64bit={is_64bit} little_endian={little_endian}"
            );
            assert!(object.is_dynamic());
        }
    }
}

#[test]
fn static_object_has_no_interpreter_and_no_needed_names() {
    let bytes = build_elf(&ElfFixture::dynamic(EM_X86_64, &[]));
    let object = parse_elf(&bytes).expect("fixture parses");
    assert!(!object.is_dynamic());
    assert!(object.interpreter.is_none());
    assert!(object.search_paths.is_empty());
}

#[test]
fn rpath_is_used_when_there_is_no_runpath() {
    let with_runpath = build_elf(&ElfFixture {
        runpath: Some("/rp"),
        ..ElfFixture::dynamic(EM_X86_64, &["libz.so.1"])
    });
    // Rewrite the DT_RUNPATH tag (29) as DT_RPATH (15): the only change.
    let at = find_tag(&with_runpath, 29).expect("RUNPATH tag present");
    let mut patched = with_runpath.clone();
    patched[at..at + 8].copy_from_slice(&15u64.to_le_bytes());
    let object = parse_elf(&patched).expect("RPATH object parses");
    assert_eq!(object.search_paths, vec!["/rp"]);
}

/// The file offset of the first dynamic entry with `tag`, in a little-endian
/// ELF64 fixture built by [`build_elf`].
fn find_tag(bytes: &[u8], tag: u64) -> Option<usize> {
    let phoff = u64::from_le_bytes(bytes[32..40].try_into().ok()?) as usize;
    let phnum = u16::from_le_bytes(bytes[56..58].try_into().ok()?) as usize;
    (0..phnum).find_map(|index| {
        let header = phoff + index * 56;
        let p_type = u32::from_le_bytes(bytes[header..header + 4].try_into().ok()?);
        if p_type != PT_DYNAMIC {
            return None;
        }
        let offset = u64::from_le_bytes(bytes[header + 8..header + 16].try_into().ok()?) as usize;
        let size = u64::from_le_bytes(bytes[header + 32..header + 40].try_into().ok()?) as usize;
        (offset..offset + size)
            .step_by(16)
            .find(|at| u64::from_le_bytes(bytes[*at..*at + 8].try_into().unwrap()) == tag)
    })
}

#[test]
fn rejects_non_elf_and_truncated_input() {
    assert!(
        parse_elf(b"#!/bin/sh\n")
            .expect_err("script")
            .contains("not an ELF")
    );
    let bytes = build_elf(&ElfFixture {
        interpreter: Some("/lib/ld.so"),
        ..ElfFixture::dynamic(EM_X86_64, &["libc.so.6"])
    });
    for cut in [20, 64, bytes.len() / 2] {
        let error = parse_elf(&bytes[..cut]).expect_err("truncated");
        assert!(
            error.contains("truncated") || error.contains("out of range"),
            "{cut}: {error}"
        );
    }
}
