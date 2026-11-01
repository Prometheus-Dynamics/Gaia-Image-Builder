//! ELF32 kernel modules (arm32 kernels such as the RV1106's): their symbol
//! table and `__versions` use 32-bit layouts.

use super::module::parse_module;

struct Section {
    name: &'static str,
    kind: u32,
    data: Vec<u8>,
    link: u32,
    entsize: u32,
}

fn section(name: &'static str, kind: u32, data: Vec<u8>) -> Section {
    Section {
        name,
        kind,
        data,
        link: 0,
        entsize: 0,
    }
}

/// A relocatable ELF32 little-endian object (an arm32 module) with the given
/// sections after the null section.
fn build_elf32(sections: Vec<Section>) -> Vec<u8> {
    let mut all: Vec<Section> = vec![section("", 0, Vec::new())];
    all.extend(sections);
    all.push(section(".shstrtab", 3, Vec::new()));
    let mut names = vec![0u8];
    let mut name_offsets = Vec::new();
    for item in &all {
        name_offsets.push(names.len() as u32);
        names.extend_from_slice(item.name.as_bytes());
        names.push(0);
    }
    let shstrndx = all.len() - 1;
    all[shstrndx].data = names;
    let mut body = Vec::new();
    let mut offsets = Vec::new();
    for item in &all {
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offsets.push(52 + body.len() as u32);
        body.extend_from_slice(&item.data);
    }
    while body.len() % 4 != 0 {
        body.push(0);
    }
    let table = 52 + body.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"\x7fELF\x01\x01\x01");
    out.extend_from_slice(&[0; 9]);
    out.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
    out.extend_from_slice(&40u16.to_le_bytes()); // EM_ARM
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // entry
    out.extend_from_slice(&0u32.to_le_bytes()); // phoff
    out.extend_from_slice(&table.to_le_bytes()); // shoff
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.extend_from_slice(&52u16.to_le_bytes()); // ehsize
    out.extend_from_slice(&0u16.to_le_bytes()); // phentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // phnum
    out.extend_from_slice(&40u16.to_le_bytes()); // shentsize
    out.extend_from_slice(&(all.len() as u16).to_le_bytes());
    out.extend_from_slice(&(shstrndx as u16).to_le_bytes());
    out.extend_from_slice(&body);
    for (index, item) in all.iter().enumerate() {
        if index == 0 {
            out.extend_from_slice(&[0; 40]);
            continue;
        }
        out.extend_from_slice(&name_offsets[index].to_le_bytes());
        out.extend_from_slice(&item.kind.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&0u32.to_le_bytes()); // addr
        out.extend_from_slice(&offsets[index].to_le_bytes());
        out.extend_from_slice(&(item.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&item.link.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // info
        out.extend_from_slice(&4u32.to_le_bytes()); // addralign
        out.extend_from_slice(&item.entsize.to_le_bytes());
    }
    out
}

#[test]
fn an_arm32_module_is_read_with_32_bit_symbols_and_versions() {
    let modinfo = "vermagic=5.10.160 SMP mod_unload ARMv7 \0depends=\0";
    // `struct modversion_info` on a 32-bit kernel: a 4-byte CRC, 60 of name.
    let mut versions = 0x1234_5678u32.to_le_bytes().to_vec();
    let mut name = [0u8; 60];
    name[..7].copy_from_slice(b"kmalloc");
    versions.extend_from_slice(&name);
    // A null symbol, then `kmalloc` (undefined) and `local` (defined).
    let strtab = b"\0kmalloc\0local\0".to_vec();
    let mut symtab = vec![0u8; 16];
    for (offset, shndx) in [(1u32, 0u16), (9, 1)] {
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.extend_from_slice(&0u32.to_le_bytes()); // value
        symtab.extend_from_slice(&0u32.to_le_bytes()); // size
        symtab.push(0x10); // GLOBAL, NOTYPE
        symtab.push(0);
        symtab.extend_from_slice(&shndx.to_le_bytes());
    }
    let bytes = build_elf32(vec![
        section(".modinfo", 1, modinfo.as_bytes().to_vec()),
        section("__versions", 1, versions),
        section(".strtab", 3, strtab),
        Section {
            name: ".symtab",
            kind: 2,
            data: symtab,
            link: 3,
            entsize: 16,
        },
    ]);
    let module = parse_module(&bytes, "arm.ko".into(), "arm".into()).expect("ELF32 module");
    assert_eq!(
        module.vermagic.as_deref(),
        Some("5.10.160 SMP mod_unload ARMv7 ")
    );
    assert_eq!(module.imports, vec!["kmalloc".to_string()]);
    assert_eq!(module.versions, vec![("kmalloc".to_string(), 0x1234_5678)]);
}
