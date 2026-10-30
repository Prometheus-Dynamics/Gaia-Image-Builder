//! A small flattened device tree (DTB, and DTBO overlays) reader: the header,
//! the structure block and the strings block. It reads what the hardware check
//! needs: node names, properties, and the `compatible` strings of a board or of
//! an overlay's fragments. Bounds are checked everywhere, so a truncated or
//! corrupt file is an error, never a panic.

/// `FDT_MAGIC`, big-endian in the header.
const MAGIC: u32 = 0xd00d_feed;
const HEADER_SIZE: usize = 40;
const TOKEN_BEGIN_NODE: u32 = 1;
const TOKEN_END_NODE: u32 = 2;
const TOKEN_PROP: u32 = 3;
const TOKEN_NOP: u32 = 4;
const TOKEN_END: u32 = 9;
/// Nodes nest a few levels in a real board; this bounds a corrupt file.
const MAX_DEPTH: usize = 128;

/// A node of the tree: its name (`fragment@0`, `__overlay__`, `i2c@7e804000`),
/// its properties in file order, and its child nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Node {
    pub name: String,
    pub props: Vec<(String, Vec<u8>)>,
    pub children: Vec<Node>,
}

impl Node {
    /// The raw value of a property, when the node has it.
    pub(crate) fn prop(&self, name: &str) -> Option<&[u8]> {
        self.props
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_slice())
    }

    /// The first string of a string property (`target-path`).
    pub fn string(&self, name: &str) -> Option<String> {
        self.prop(name)
            .and_then(|value| strings_of(value).into_iter().next())
    }

    /// The `compatible` strings of this node and every node below it.
    pub fn compatibles(&self) -> Vec<String> {
        let mut found = Vec::new();
        self.collect_compatibles(&mut found);
        found
    }

    fn collect_compatibles(&self, found: &mut Vec<String>) {
        if let Some(value) = self.prop("compatible") {
            found.extend(strings_of(value));
        }
        for child in &self.children {
            child.collect_compatibles(found);
        }
    }
}

/// One fragment of a device tree overlay: `fragment@N { target-path; __overlay__ }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fragment {
    pub name: String,
    /// The node the fragment applies to, when it names one by path.
    pub target_path: Option<String>,
    /// The compatibles its `__overlay__` nodes name.
    pub compatibles: Vec<String>,
}

/// The fragments of an overlay (the root's `fragment@*` children). An overlay
/// that names its targets only by phandle (`target`) has no `target-path`.
pub(super) fn overlay_fragments(root: &Node) -> Vec<Fragment> {
    root.children
        .iter()
        .filter(|child| child.name == "fragment" || child.name.starts_with("fragment@"))
        .map(|fragment| {
            let compatibles = fragment
                .children
                .iter()
                .filter(|child| child.name == "__overlay__")
                .flat_map(Node::compatibles)
                .collect();
            Fragment {
                name: fragment.name.clone(),
                target_path: fragment.string("target-path"),
                compatibles,
            }
        })
        .collect()
}

/// Splits a string property (a list of NUL-terminated strings).
pub(super) fn strings_of(value: &[u8]) -> Vec<String> {
    value
        .split(|byte| *byte == 0)
        .filter(|text| !text.is_empty())
        .map(|text| String::from_utf8_lossy(text).into_owned())
        .collect()
}

/// Parses a DTB or DTBO. Errors name what is wrong with the file.
pub(super) fn parse(bytes: &[u8]) -> Result<Node, String> {
    let header = bytes
        .get(..HEADER_SIZE)
        .ok_or("shorter than a device tree header")?;
    if be32(header, 0) != Some(MAGIC) {
        return Err("not a device tree (bad magic)".into());
    }
    let total = be32(header, 4).ok_or("no total size")? as usize;
    let struct_offset = be32(header, 8).ok_or("no structure offset")? as usize;
    let strings_offset = be32(header, 12).ok_or("no strings offset")? as usize;
    let strings_size = be32(header, 32).ok_or("no strings size")? as usize;
    let struct_size = be32(header, 36).ok_or("no structure size")? as usize;
    if total > bytes.len() {
        return Err(format!(
            "truncated: the header says {total} bytes, the file has {}",
            bytes.len()
        ));
    }
    let blob = &bytes[..total];
    let structure = section(blob, struct_offset, struct_size)
        .ok_or("the structure block lies outside the file")?;
    let strings = section(blob, strings_offset, strings_size)
        .ok_or("the strings block lies outside the file")?;
    parse_structure(structure, strings)
}

fn section(blob: &[u8], offset: usize, size: usize) -> Option<&[u8]> {
    blob.get(offset..offset.checked_add(size)?)
}

fn parse_structure(structure: &[u8], strings: &[u8]) -> Result<Node, String> {
    let mut pos = 0usize;
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    loop {
        let token = be32(structure, pos).ok_or("the structure block ends inside a token")?;
        pos += 4;
        match token {
            TOKEN_BEGIN_NODE => {
                let (name, next) = c_string(structure, pos)?;
                pos = align4(next);
                if stack.len() >= MAX_DEPTH {
                    return Err("nodes nest deeper than a device tree can".into());
                }
                stack.push(Node {
                    name,
                    ..Node::default()
                });
            }
            TOKEN_END_NODE => {
                let node = stack.pop().ok_or("an end-of-node with no node open")?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None if root.is_none() => root = Some(node),
                    None => return Err("more than one root node".into()),
                }
            }
            TOKEN_PROP => {
                let len = be32(structure, pos).ok_or("a property with no length")? as usize;
                let name_offset =
                    be32(structure, pos + 4).ok_or("a property with no name")? as usize;
                let value = structure
                    .get(pos + 8..pos + 8 + len)
                    .ok_or("a property runs past the structure block")?;
                pos = align4(pos + 8 + len);
                let (name, _) = c_string(strings, name_offset)?;
                let node = stack.last_mut().ok_or("a property outside any node")?;
                node.props.push((name, value.to_vec()));
            }
            TOKEN_NOP => {}
            TOKEN_END => break,
            other => return Err(format!("unknown token {other} in the structure block")),
        }
    }
    if !stack.is_empty() {
        return Err("the structure block ends with a node still open".into());
    }
    root.ok_or_else(|| "no root node".into())
}

/// A NUL-terminated string at `offset`, and the offset after its NUL.
fn c_string(bytes: &[u8], offset: usize) -> Result<(String, usize), String> {
    let rest = bytes
        .get(offset..)
        .ok_or("a name starts outside its block")?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or("a name is not terminated")?;
    Ok((
        String::from_utf8_lossy(&rest[..end]).into_owned(),
        offset + end + 1,
    ))
}

fn align4(offset: usize) -> usize {
    (offset + 3) & !3
}

fn be32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let word: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(u32::from_be_bytes(word))
}
