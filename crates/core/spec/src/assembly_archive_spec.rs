use crate::AssemblyPathTemplate;

/// A deterministic ustar archive assembled after disks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyArchiveSpec {
    pub id: String,
    pub output: AssemblyPathTemplate,
    /// Members in archive order.
    pub members: Vec<AssemblyArchiveMemberSpec>,
}

/// One archive member: either a file (`src`) or a generated `KEY=value`
/// file (`entries`). Exactly one of the two must be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyArchiveMemberSpec {
    pub name: String,
    pub src: Option<AssemblyPathTemplate>,
    pub entries: Option<Vec<(String, String)>>,
}

impl AssemblyArchiveMemberSpec {
    pub fn source(&self) -> Option<AssemblyArchiveMemberSourceSpec<'_>> {
        match (&self.src, &self.entries) {
            (Some(src), None) => Some(AssemblyArchiveMemberSourceSpec::File(src)),
            (None, Some(entries)) => Some(AssemblyArchiveMemberSourceSpec::Generated(entries)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyArchiveMemberSourceSpec<'a> {
    File(&'a AssemblyPathTemplate),
    Generated(&'a [(String, String)]),
}

/// An `${assembly.sha256:<path template>}` reference inside a generated
/// archive entry value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyDigestToken {
    pub path: AssemblyPathTemplate,
}

const DIGEST_TOKEN_PREFIX: &str = "assembly.sha256:";

/// Splits a generated entry value into literal text and digest tokens,
/// replacing each token with `digest(token)`. Any other `${...}` token is an
/// error: normal config interpolation has already run, so it is unresolved.
pub fn assembly_digest_tokens(
    value: &str,
    mut digest: impl FnMut(&AssemblyDigestToken) -> Result<String, String>,
) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let remainder = &rest[start + 2..];
        let Some(end) = remainder.find('}') else {
            return Err(format!("unterminated '${{' in '{value}'"));
        };
        let token = &remainder[..end];
        let Some(path) = token.strip_prefix(DIGEST_TOKEN_PREFIX) else {
            return Err(format!(
                "unsupported token '${{{token}}}' in '{value}'; only '${{assembly.sha256:<path>}}' is resolved at assembly time"
            ));
        };
        if path.trim().is_empty() {
            return Err(format!("'${{{token}}}' needs a path in '{value}'"));
        }
        output.push_str(&digest(&AssemblyDigestToken {
            path: AssemblyPathTemplate::new(path.trim()),
        })?);
        rest = &remainder[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_tokens_are_replaced_and_literals_kept() {
        let mut seen = Vec::new();
        let rendered = assembly_digest_tokens(
            "a=${assembly.sha256:$assembly.out/boot.vfat}/b=${assembly.sha256: x.img }",
            |token| {
                seen.push(token.path.as_str().to_string());
                Ok("HASH".into())
            },
        )
        .expect("rendered");
        assert_eq!(rendered, "a=HASH/b=HASH");
        assert_eq!(seen, vec!["$assembly.out/boot.vfat", "x.img"]);
        assert_eq!(
            assembly_digest_tokens("plain", |_| Ok(String::new())).expect("plain"),
            "plain"
        );
    }

    #[test]
    fn digest_tokens_reject_unknown_unterminated_and_empty_tokens() {
        for value in [
            "${build.version}",
            "${assembly.md5:x}",
            "${assembly.sha256:x",
            "${assembly.sha256: }",
        ] {
            assert!(
                assembly_digest_tokens(value, |_| Ok(String::new())).is_err(),
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
    }
}
