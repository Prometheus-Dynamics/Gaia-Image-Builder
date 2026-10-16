use crate::{AssemblyJsonValue, AssemblyPathTemplate};

/// A deterministic ustar archive assembled after disks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyArchiveSpec {
    pub id: String,
    pub output: AssemblyPathTemplate,
    /// Members in archive order.
    pub members: Vec<AssemblyArchiveMemberSpec>,
}

/// One archive member: a file (`src`), a generated `KEY=value` file
/// (`entries`), or a generated JSON document (`json`). Exactly one of the
/// three must be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyArchiveMemberSpec {
    pub name: String,
    pub src: Option<AssemblyPathTemplate>,
    pub entries: Option<Vec<(String, String)>>,
    pub json: Option<AssemblyJsonValue>,
}

impl AssemblyArchiveMemberSpec {
    pub fn source(&self) -> Option<AssemblyArchiveMemberSourceSpec<'_>> {
        match (&self.src, &self.entries, &self.json) {
            (Some(src), None, None) => Some(AssemblyArchiveMemberSourceSpec::File(src)),
            (None, Some(entries), None) => {
                Some(AssemblyArchiveMemberSourceSpec::Generated(entries))
            }
            (None, None, Some(json)) => Some(AssemblyArchiveMemberSourceSpec::Json(json)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyArchiveMemberSourceSpec<'a> {
    File(&'a AssemblyPathTemplate),
    Generated(&'a [(String, String)]),
    Json(&'a AssemblyJsonValue),
}

/// A `${assembly...}` reference inside a generated archive entry value,
/// resolved at assembly time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblyValueToken {
    /// `${assembly.sha256:<path>}`: the hex sha256 of a file.
    Digest(AssemblyDigestToken),
    /// `${assembly.verity:<path>:<field>}`: one field of a verity transform's
    /// output.
    Verity(AssemblyVerityToken),
    /// `${assembly.size:<path>}`: the size of a file in bytes.
    Size(AssemblySizeToken),
}

impl AssemblyValueToken {
    /// The path template the token reads.
    pub fn path(&self) -> &AssemblyPathTemplate {
        match self {
            Self::Digest(token) => &token.path,
            Self::Verity(token) => &token.path,
            Self::Size(token) => &token.path,
        }
    }

    /// Whether the token's text is a decimal integer, so a JSON value that is
    /// exactly this token is written as a number.
    pub fn is_numeric(&self) -> bool {
        match self {
            Self::Digest(_) => false,
            Self::Verity(token) => token.field.is_numeric(),
            Self::Size(_) => true,
        }
    }
}

/// An `${assembly.size:<path template>}` reference: the file's size in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblySizeToken {
    pub path: AssemblyPathTemplate,
}

/// An `${assembly.sha256:<path template>}` reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyDigestToken {
    pub path: AssemblyPathTemplate,
}

/// An `${assembly.verity:<path template>:<field>}` reference: the `dest` of
/// a verity transform, and what to read from its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyVerityToken {
    pub path: AssemblyPathTemplate,
    pub field: AssemblyVerityField,
}

/// The fields of a verity output an archive entry can name. A consumer
/// needs all of them to open the tree (`veritysetup open`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn is_numeric(self) -> bool {
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
        output.push_str(&resolve(&token)?);
        rest = &remainder[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
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
        "unsupported token '${{{token}}}' in '{value}'; only '${{assembly.sha256:<path>}}', '${{assembly.size:<path>}}' and '${{assembly.verity:<path>:<field>}}' are resolved at assembly time"
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
            name: "m".into(),
            src: Some("x".into()),
            entries: None,
            json: None,
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
    }
}
