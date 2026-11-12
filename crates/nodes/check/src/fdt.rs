//! A flattened device tree reader (a DTB, or an overlay's `.dtbo`) for the
//! device tree check. It reads the structure block's node names; properties
//! are skipped, since a node is named, not what it holds. The header and the
//! block are bounds-checked, so a truncated or corrupt file is an error,
//! never a panic.

const MAGIC: u32 = 0xd00d_feed;
const TOKEN_BEGIN_NODE: u32 = 1;
const TOKEN_END_NODE: u32 = 2;
const TOKEN_PROP: u32 = 3;
const TOKEN_NOP: u32 = 4;
const TOKEN_END: u32 = 9;
/// Nodes nest a few levels in a real board; this bounds a corrupt file.
const MAX_DEPTH: usize = 128;

/// A node: its name (`soc`, `i2c@7e804000`; the root's is empty) and its
/// child nodes in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub children: Vec<Node>,
}

impl Node {
    /// Whether the node at `path` (`/soc/i2c@7e804000`, or `/` for the root)
    /// exists below this one. Names match exactly, unit address included.
    pub fn has(&self, path: &str) -> bool {
        let mut node = self;
        for part in path.split('/').filter(|part| !part.is_empty()) {
            match node.children.iter().find(|child| child.name == part) {
                Some(child) => node = child,
                None => return false,
            }
        }
        true
    }
}

/// Parses a DTB, giving its root node.
pub fn parse(blob: &[u8]) -> Result<Node, String> {
    if be32(blob, 0)? != MAGIC {
        return Err("no device tree magic".into());
    }
    let mut at = usize::try_from(be32(blob, 8)?).map_err(|_| "structure block out of range")?;
    let mut open: Vec<Node> = Vec::new();
    let mut root = None;
    loop {
        let token = be32(blob, at)?;
        at += 4;
        match token {
            TOKEN_BEGIN_NODE => {
                let name = cstring(blob, at)?;
                at = align4(at.saturating_add(name.len() + 1));
                if open.len() >= MAX_DEPTH {
                    return Err("nodes nest too deep".into());
                }
                open.push(Node {
                    name,
                    children: Vec::new(),
                });
            }
            TOKEN_END_NODE => {
                let node = open.pop().ok_or("an end of node with no node open")?;
                match open.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None if root.is_none() => root = Some(node),
                    None => return Err("a second root node".into()),
                }
            }
            TOKEN_PROP => {
                let len = usize::try_from(be32(blob, at)?).map_err(|_| "property out of range")?;
                at = align4(at.saturating_add(8 + len));
            }
            TOKEN_NOP => {}
            TOKEN_END => break,
            other => return Err(format!("unknown token {other} in the structure block")),
        }
    }
    if !open.is_empty() {
        return Err("a node is not closed".into());
    }
    root.ok_or_else(|| "no root node".to_string())
}

fn be32(bytes: &[u8], at: usize) -> Result<u32, String> {
    let end = at.checked_add(4).ok_or("offset out of range")?;
    let four: [u8; 4] = bytes
        .get(at..end)
        .ok_or("truncated device tree")?
        .try_into()
        .map_err(|_| "truncated device tree")?;
    Ok(u32::from_be_bytes(four))
}

fn cstring(bytes: &[u8], at: usize) -> Result<String, String> {
    let rest = bytes.get(at..).ok_or("a name runs past the end")?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or("an unterminated name")?;
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

fn align4(at: usize) -> usize {
    at.saturating_add(3) & !3
}
