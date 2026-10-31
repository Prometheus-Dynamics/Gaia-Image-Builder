//! A device tree blob writer for the image check tests: enough of the
//! format for the reader in `fdt`.

/// A device tree blob built node by node.
#[derive(Default)]
pub(super) struct Dtb {
    structure: Vec<u8>,
    strings: Vec<u8>,
}

impl Dtb {
    pub(super) fn begin(&mut self, name: &str) {
        self.word(1);
        self.structure.extend_from_slice(name.as_bytes());
        self.structure.push(0);
        self.align();
    }

    pub(super) fn end(&mut self) {
        self.word(2);
    }

    pub(super) fn strings(&mut self, name: &str, values: &[&str]) {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        self.prop(name, &bytes);
    }

    pub(super) fn prop(&mut self, name: &str, value: &[u8]) {
        let offset = self.name_offset(name);
        self.word(3);
        self.word(value.len() as u32);
        self.word(offset);
        self.structure.extend_from_slice(value);
        self.align();
    }

    fn word(&mut self, value: u32) {
        self.structure.extend_from_slice(&value.to_be_bytes());
    }

    fn align(&mut self) {
        while !self.structure.len().is_multiple_of(4) {
            self.structure.push(0);
        }
    }

    fn name_offset(&mut self, name: &str) -> u32 {
        let mut needle = name.as_bytes().to_vec();
        needle.push(0);
        if let Some(found) = self
            .strings
            .windows(needle.len())
            .position(|window| window == needle.as_slice())
        {
            return found as u32;
        }
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(&needle);
        offset
    }

    pub(super) fn finish(mut self) -> Vec<u8> {
        self.word(9);
        // The header, then one empty memory reservation entry.
        let struct_offset = 40 + 16;
        let strings_offset = struct_offset + self.structure.len();
        let total = strings_offset + self.strings.len();
        let mut out = Vec::with_capacity(total);
        for word in [
            0xd00d_feed,
            total as u32,
            struct_offset as u32,
            strings_offset as u32,
            40,
            17,
            16,
            0,
            self.strings.len() as u32,
            self.structure.len() as u32,
        ] {
            out.extend_from_slice(&word.to_be_bytes());
        }
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&self.structure);
        out.extend_from_slice(&self.strings);
        out
    }
}
