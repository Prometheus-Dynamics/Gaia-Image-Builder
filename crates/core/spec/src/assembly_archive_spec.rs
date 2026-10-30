use crate::{AssemblyJsonValue, AssemblyPathTemplate, AssemblyTreeId};

/// A deterministic ustar archive assembled after disks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyArchiveSpec {
    pub id: String,
    pub output: AssemblyPathTemplate,
    /// Members in archive order.
    pub members: Vec<AssemblyArchiveMemberSpec>,
}

/// One archive member. Exactly one source must be set:
///
/// - `src`: a file, stored as `name`.
/// - `entries`: a generated `KEY=value` file, stored as `name`.
/// - `json`: a generated JSON document, stored as `name`.
/// - `tree`: every entry under an assembly tree's root, stored under `prefix`.
/// - `src_dir`: every entry under a directory, stored under `prefix`.
/// - `src_glob`: each file the pattern matches, stored as `name` (a directory
///   prefix ending in `/`) followed by the file's own name.
///
/// `prefix` only applies to `tree` and `src_dir`. For those, `name` may be
/// empty; then the member is labelled by its source.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyArchiveMemberSpec {
    pub name: String,
    pub src: Option<AssemblyPathTemplate>,
    pub entries: Option<Vec<(String, String)>>,
    pub json: Option<AssemblyJsonValue>,
    pub tree: Option<AssemblyTreeId>,
    pub src_dir: Option<AssemblyPathTemplate>,
    pub src_glob: Option<AssemblyPathTemplate>,
    /// Archive path prefix for `tree` and `src_dir`: empty or ends in `/`.
    pub prefix: String,
    /// `tree` and `src_glob` only: file names (simple `*` globs allowed)
    /// left out of the set. Directories are never excluded.
    pub exclude: Vec<String>,
}

impl AssemblyArchiveMemberSpec {
    pub fn source(&self) -> Option<AssemblyArchiveMemberSourceSpec<'_>> {
        let set = [
            self.src.is_some(),
            self.entries.is_some(),
            self.json.is_some(),
            self.tree.is_some(),
            self.src_dir.is_some(),
            self.src_glob.is_some(),
        ]
        .iter()
        .filter(|set| **set)
        .count();
        if set != 1 {
            return None;
        }
        if let Some(src) = &self.src {
            return Some(AssemblyArchiveMemberSourceSpec::File(src));
        }
        if let Some(entries) = &self.entries {
            return Some(AssemblyArchiveMemberSourceSpec::Generated(entries));
        }
        if let Some(json) = &self.json {
            return Some(AssemblyArchiveMemberSourceSpec::Json(json));
        }
        if let Some(tree) = &self.tree {
            return Some(AssemblyArchiveMemberSourceSpec::Tree(tree));
        }
        if let Some(src_dir) = &self.src_dir {
            return Some(AssemblyArchiveMemberSourceSpec::Dir(src_dir));
        }
        self.src_glob
            .as_ref()
            .map(AssemblyArchiveMemberSourceSpec::Glob)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AssemblyArchiveMemberSourceSpec<'a> {
    File(&'a AssemblyPathTemplate),
    Generated(&'a [(String, String)]),
    Json(&'a AssemblyJsonValue),
    Tree(&'a AssemblyTreeId),
    Dir(&'a AssemblyPathTemplate),
    Glob(&'a AssemblyPathTemplate),
}

/// A `${assembly...}` reference inside a generated archive entry value,
/// resolved at assembly time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum AssemblyValueToken {
    /// `${assembly.sha256:<path>}`: the hex sha256 of a file.
    Digest(AssemblyDigestToken),
    /// `${assembly.verity:<path>:<field>}`: one field of a verity transform's
    /// output.
    Verity(AssemblyVerityToken),
    /// `${assembly.size:<path>}`: the size of a file in bytes.
    Size(AssemblySizeToken),
    /// `${assembly.sha256_tree:<tree id or path>}`: a JSON object mapping each
    /// regular file's path under the tree to its sha256. Only a whole JSON
    /// value may be one; in text it is an error.
    TreeDigest(AssemblyTreeDigestToken),
}

impl AssemblyValueToken {
    /// The path template the token reads.
    pub fn path(&self) -> &AssemblyPathTemplate {
        match self {
            Self::Digest(token) => &token.path,
            Self::Verity(token) => &token.path,
            Self::Size(token) => &token.path,
            Self::TreeDigest(token) => &token.path,
        }
    }

    /// Whether the token's text is a decimal integer, so a JSON value that is
    /// exactly this token is written as a number.
    pub(crate) fn is_numeric(&self) -> bool {
        match self {
            Self::Digest(_) | Self::TreeDigest(_) => false,
            Self::Verity(token) => token.field.is_numeric(),
            Self::Size(_) => true,
        }
    }
}

/// An `${assembly.size:<path template>}` reference: the file's size in bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblySizeToken {
    pub path: AssemblyPathTemplate,
}

/// An `${assembly.sha256:<path template>}` reference.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyDigestToken {
    pub path: AssemblyPathTemplate,
}

/// An `${assembly.sha256_tree:<tree id or path template>}` reference: the
/// per-file digest map of a tree or directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyTreeDigestToken {
    pub path: AssemblyPathTemplate,
}

/// The tree digest token, when `text` is exactly one
/// `${assembly.sha256_tree:<path>}` with a non-empty path. Only a JSON value
/// may be one (see [`AssemblyValueToken::TreeDigest`]).
pub fn whole_tree_digest_token(text: &str) -> Option<AssemblyTreeDigestToken> {
    let body = text
        .strip_prefix("${")?
        .strip_suffix('}')?
        .strip_prefix(TREE_DIGEST_TOKEN_PREFIX)?;
    if body.contains("${") || body.contains('}') {
        return None;
    }
    let path = body.trim();
    (!path.is_empty()).then(|| AssemblyTreeDigestToken {
        path: AssemblyPathTemplate::new(path),
    })
}

/// An `${assembly.verity:<path template>:<field>}` reference: the `dest` of
/// a verity transform, and what to read from its output.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyVerityToken {
    pub path: AssemblyPathTemplate,
    pub field: AssemblyVerityField,
}

/// The fields of a verity output an archive entry can name. A consumer
/// needs all of them to open the tree (`veritysetup open`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AssemblyVerityField {
    /// The root hash, lower-case hex.
    RootHash,
    /// The salt, lower-case hex.
    Salt,
    /// Byte offset of the verity superblock in the hash device.
    HashOffset,
    /// The number of data blocks covered by the tree.
    DataBlocks,
    /// The data block size in bytes.
    DataBlockSize,
    /// The hash block size in bytes.
    HashBlockSize,
}

impl AssemblyVerityField {
    /// Whether the field is a decimal integer rather than hex text.
    pub(crate) fn is_numeric(self) -> bool {
        !matches!(self, Self::RootHash | Self::Salt)
    }

    pub const ALL: [Self; 6] = [
        Self::RootHash,
        Self::Salt,
        Self::HashOffset,
        Self::DataBlocks,
        Self::DataBlockSize,
        Self::HashBlockSize,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootHash => "root_hash",
            Self::Salt => "salt",
            Self::HashOffset => "hash_offset",
            Self::DataBlocks => "data_blocks",
            Self::DataBlockSize => "data_block_size",
            Self::HashBlockSize => "hash_block_size",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.as_str() == raw)
    }
}

const DIGEST_TOKEN_PREFIX: &str = "assembly.sha256:";
const VERITY_TOKEN_PREFIX: &str = "assembly.verity:";
const SIZE_TOKEN_PREFIX: &str = "assembly.size:";
const TREE_DIGEST_TOKEN_PREFIX: &str = "assembly.sha256_tree:";

/// Splits a generated entry value into literal text and `${assembly...}`
/// tokens, replacing each token with `resolve(token)`. Any other `${...}`
/// token is an error: normal config interpolation has already run, so it is
/// unresolved.
pub fn assembly_value_tokens(
    value: &str,
    mut resolve: impl FnMut(&AssemblyValueToken) -> Result<String, String>,
) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let remainder = &rest[start + 2..];
        let Some(end) = remainder.find('}') else {
            return Err(format!("unterminated '${{' in '{value}'"));
        };
        let token = parse_value_token(&remainder[..end], value)?;
        if matches!(token, AssemblyValueToken::TreeDigest(_)) {
            return Err(format!(
                "'${{{}}}' is a whole JSON value (a file or archive json table); it cannot be part of text in '{value}'",
                &remainder[..end]
            ));
        }
        output.push_str(&resolve(&token)?);
        rest = &remainder[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// The tokens of a JSON string, for readers that do not render: a whole
/// tree digest token is one token; any other string reads as
/// [`assembly_value_tokens`] does.
pub fn assembly_json_string_tokens(
    value: &str,
    mut resolve: impl FnMut(&AssemblyValueToken) -> Result<String, String>,
) -> Result<String, String> {
    match whole_tree_digest_token(value) {
        Some(token) => resolve(&AssemblyValueToken::TreeDigest(token)).map(|_| String::new()),
        None => assembly_value_tokens(value, resolve),
    }
}

fn parse_value_token(token: &str, value: &str) -> Result<AssemblyValueToken, String> {
    if let Some(path) = token.strip_prefix(DIGEST_TOKEN_PREFIX) {
        if path.trim().is_empty() {
            return Err(format!("'${{{token}}}' needs a path in '{value}'"));
        }
        return Ok(AssemblyValueToken::Digest(AssemblyDigestToken {
            path: AssemblyPathTemplate::new(path.trim()),
        }));
    }
    if let Some(path) = token.strip_prefix(SIZE_TOKEN_PREFIX) {
        if path.trim().is_empty() {
            return Err(format!("'${{{token}}}' needs a path in '{value}'"));
        }
        return Ok(AssemblyValueToken::Size(AssemblySizeToken {
            path: AssemblyPathTemplate::new(path.trim()),
        }));
    }
    if let Some(path) = token.strip_prefix(TREE_DIGEST_TOKEN_PREFIX) {
        if path.trim().is_empty() {
            return Err(format!(
                "'${{{token}}}' needs a tree id or path in '{value}'"
            ));
        }
        return Ok(AssemblyValueToken::TreeDigest(AssemblyTreeDigestToken {
            path: AssemblyPathTemplate::new(path.trim()),
        }));
    }
    if let Some(body) = token.strip_prefix(VERITY_TOKEN_PREFIX) {
        let Some((path, field)) = body.rsplit_once(':') else {
            return Err(format!(
                "'${{{token}}}' must be '${{assembly.verity:<path>:<field>}}' in '{value}'"
            ));
        };
        if path.trim().is_empty() {
            return Err(format!("'${{{token}}}' needs a path in '{value}'"));
        }
        let field = AssemblyVerityField::parse(field.trim()).ok_or_else(|| {
            format!(
                "'${{{token}}}' names unknown verity field '{}' in '{value}'; use one of {}",
                field.trim(),
                AssemblyVerityField::ALL
                    .iter()
                    .map(|field| field.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        return Ok(AssemblyValueToken::Verity(AssemblyVerityToken {
            path: AssemblyPathTemplate::new(path.trim()),
            field,
        }));
    }
    Err(format!(
        "unsupported token '${{{token}}}' in '{value}'; only '${{assembly.sha256:<path>}}', '${{assembly.sha256_tree:<tree or path>}}', '${{assembly.size:<path>}}' and '${{assembly.verity:<path>:<field>}}' are resolved at assembly time"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_tokens_are_replaced_and_literals_kept() {
        let mut seen = Vec::new();
        let rendered = assembly_value_tokens(
            "a=${assembly.sha256:$assembly.out/boot.vfat}/b=${assembly.sha256: x.img }",
            |token| {
                seen.push(token.path().as_str().to_string());
                Ok("HASH".into())
            },
        )
        .expect("rendered");
        assert_eq!(rendered, "a=HASH/b=HASH");
        assert_eq!(seen, vec!["$assembly.out/boot.vfat", "x.img"]);
        assert_eq!(
            assembly_value_tokens("plain", |_| Ok(String::new())).expect("plain"),
            "plain"
        );
    }

    #[test]
    fn verity_tokens_name_a_path_and_a_field() {
        let rendered = assembly_value_tokens(
            "r=${assembly.verity:$assembly.out/rootfs.erofs:root_hash}/o=${assembly.verity: $assembly.out/rootfs.erofs :hash_offset}",
            |token| match token {
                AssemblyValueToken::Verity(token) => Ok(format!(
                    "{}|{}",
                    token.path.as_str(),
                    token.field.as_str()
                )),
                _ => Err("other".into()),
            },
        )
        .expect("rendered");
        assert_eq!(
            rendered,
            "r=$assembly.out/rootfs.erofs|root_hash/o=$assembly.out/rootfs.erofs|hash_offset"
        );
        assert_eq!(
            AssemblyVerityField::parse("data_blocks"),
            Some(AssemblyVerityField::DataBlocks)
        );
        assert_eq!(AssemblyVerityField::parse("uuid"), None);
    }

    #[test]
    fn tree_digest_tokens_are_read_only_as_whole_json_values() {
        assert!(
            assembly_value_tokens("${assembly.sha256_tree:payload}", |_| Ok(String::new()))
                .is_err()
        );
        assert!(assembly_value_tokens("${assembly.sha256_tree:}", |_| Ok(String::new())).is_err());
        assert!(
            assembly_json_string_tokens("x${assembly.sha256_tree:payload}", |_| Ok(String::new()))
                .is_err()
        );
        let mut seen = Vec::new();
        assembly_json_string_tokens("${assembly.sha256_tree:payload}", |token| {
            seen.push(token.clone());
            Ok(String::new())
        })
        .expect("whole value");
        assert!(matches!(
            seen.as_slice(),
            [AssemblyValueToken::TreeDigest(token)] if token.path.as_str() == "payload"
        ));
    }

    #[test]
    fn value_tokens_reject_unknown_unterminated_and_empty_tokens() {
        for value in [
            "${build.version}",
            "${assembly.md5:x}",
            "${assembly.sha256:x",
            "${assembly.sha256: }",
            "${assembly.verity:x}",
            "${assembly.verity:x:uuid}",
            "${assembly.verity::root_hash}",
            "${assembly.size: }",
        ] {
            assert!(
                assembly_value_tokens(value, |_| Ok(String::new())).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn archive_member_source_requires_exactly_one_kind() {
        let mut member = AssemblyArchiveMemberSpec {
            tree: None,
            src_dir: None,
            src_glob: None,
            prefix: String::new(),
            exclude: Vec::new(),
            name: "m".into(),
            src: Some("x".into()),
            ..Default::default()
        };
        assert!(matches!(
            member.source(),
            Some(AssemblyArchiveMemberSourceSpec::File(_))
        ));
        member.entries = Some(Vec::new());
        assert_eq!(member.source(), None);
        member.src = None;
        assert!(matches!(
            member.source(),
            Some(AssemblyArchiveMemberSourceSpec::Generated(_))
        ));
        member.entries = None;
        member.json = Some(AssemblyJsonValue::Object(Vec::new()));
        assert!(matches!(
            member.source(),
            Some(AssemblyArchiveMemberSourceSpec::Json(_))
        ));
        member.src = Some("x".into());
        assert_eq!(member.source(), None);
        member.src = None;
        member.json = None;
        member.tree = Some(AssemblyTreeId::new("boot"));
        assert!(matches!(
            member.source(),
            Some(AssemblyArchiveMemberSourceSpec::Tree(_))
        ));
        member.src_dir = Some("x".into());
        assert_eq!(member.source(), None);
        member.tree = None;
        assert!(matches!(
            member.source(),
            Some(AssemblyArchiveMemberSourceSpec::Dir(_))
        ));
        member.src_dir = None;
        member.src_glob = Some("x/*".into());
        assert!(matches!(
            member.source(),
            Some(AssemblyArchiveMemberSourceSpec::Glob(_))
        ));
        member.src_glob = None;
        assert_eq!(member.source(), None);
    }
}
