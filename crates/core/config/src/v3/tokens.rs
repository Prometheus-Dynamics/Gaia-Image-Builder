//! Config v3's one token syntax, `${...}`, and the v2 spellings it replaces.
//!
//! | v3 | v2 |
//! |---|---|
//! | `${provider.images}` | `$provider.images` |
//! | `${assembly.work}`, `${assembly.out}`, `${assembly.tree.<id>}` | `$assembly.work`, ... |
//! | `${self}` | `@self` |
//! | `${source.<id>}` | `@source:<id>` |
//! | `${<alias>}` (a `[[workspace.named_paths]]` alias) | `@<alias>` |
//! | `${outputs.dir}` | `${workspace.out_dir}` |
//!
//! Everything else in `${...}` is spelled the same in both. The path tokens
//! that were `@...` in v2 must start the value, or an entry of a
//! `:`-separated list, as they had to in v2; in a Buildroot
//! `config_overrides` value, also an entry of a space-separated list, which
//! the provider resolves entry by entry.

/// What reading a v3 string found besides its v2 text: the named-path
/// aliases it used, checked once every layer is merged.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct V3Tokens {
    pub aliases: Vec<String>,
}

/// The first segments a `${...}` token may have. A dotted token outside
/// these is an error where it is written.
const NAMESPACES: &[&str] = &[
    "assembly",
    "build",
    "config",
    "execution",
    "input",
    "interpolation",
    "outputs",
    "preset",
    "product",
    "project",
    "provenance",
    "provider",
    "source",
    "workspace",
];

/// Names a named-path alias cannot take in v3, because `${<name>}` would
/// mean something else.
const RESERVED_ALIASES: &[&str] = &["self", "choice"];

/// Whether `alias` would shadow a namespace or another token as `${alias}`.
pub(crate) fn alias_is_reserved(alias: &str) -> bool {
    NAMESPACES.contains(&alias) || RESERVED_ALIASES.contains(&alias)
}

/// The v2 text of one v3 string. `Err` names the problem: a v2 token, an
/// unknown namespace, or a path token that does not start its value.
pub(crate) fn v3_string_to_v2(text: &str, found: &mut V3Tokens) -> Result<String, String> {
    reject_v2_tokens(text)?;
    translate(text, 0, false, found)
}

/// [`v3_string_to_v2`] for a Buildroot `config_overrides` value, whose path
/// tokens may also start an entry of a space-separated list
/// (`${family}/a.fragment ${assets}/b.fragment`).
pub(crate) fn v3_override_to_v2(text: &str, found: &mut V3Tokens) -> Result<String, String> {
    reject_v2_tokens(text)?;
    translate(text, 0, true, found)
}

/// The v2 spellings a v3 file may not use.
fn reject_v2_tokens(text: &str) -> Result<(), String> {
    for prefix in ["$provider.", "$assembly."] {
        if let Some(start) = text.find(prefix) {
            let token = dollar_token(&text[start..]);
            let name = &token[1..];
            return Err(format!("`{token}` is written `${{{name}}}` in config v3"));
        }
    }
    for entry in entries(text) {
        let entry = entry.1;
        if entry == "@self" || entry.starts_with("@self/") {
            return Err("`@self` is written `${self}` in config v3".into());
        }
        if let Some(rest) = entry.strip_prefix("@source:") {
            let id = rest.split('/').next().unwrap_or(rest);
            return Err(format!(
                "`@source:{id}` is written `${{source.{id}}}` in config v3"
            ));
        }
        if let Some(rest) = entry.strip_prefix('@')
            && let Some((alias, _)) = rest.split_once('/')
            && is_identifier(alias)
        {
            return Err(format!(
                "`@{alias}/` is written `${{{alias}}}/` in config v3"
            ));
        }
    }
    Ok(())
}

/// A v2 `$provider...`/`$assembly...` token at the start of `text`, as v2
/// reads it.
fn dollar_token(text: &str) -> &str {
    let end = text[1..]
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-'))
        })
        .map_or(text.len(), |index| index + 1);
    &text[..end]
}

/// The byte offset and text of each `:`-separated entry, with
/// `@source:<id>` kept whole, as v2's path tokens read a list.
fn entries(text: &str) -> Vec<(usize, &str)> {
    let mut entries: Vec<(usize, &str)> = Vec::new();
    let mut start = 0;
    for (index, _) in text.match_indices(':') {
        if text[start..index] == *"@source" {
            continue;
        }
        entries.push((start, &text[start..index]));
        start = index + 1;
    }
    entries.push((start, &text[start..]));
    entries
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

/// Whether byte `offset` of `text` starts the value or an entry of a list:
/// `:`-separated, or space-separated with `spaces`.
fn starts_entry(text: &str, offset: usize, spaces: bool) -> bool {
    let before = &text[..offset];
    offset == 0
        || before.ends_with(':')
        || (spaces
            && before
                .ends_with(|character: char| character.is_ascii_whitespace() || character == '"'))
}

/// Translates `text`, which sits at `base` in its value (non-zero only
/// inside another token), token by token; nested tokens are translated
/// first, so `${assembly.sha256:${assembly.work}/x}` keeps its outer token
/// with a v2 path inside.
fn translate(
    text: &str,
    base: usize,
    spaces: bool,
    found: &mut V3Tokens,
) -> Result<String, String> {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    let mut consumed = 0;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let Some(length) = token_length(&rest[start..]) else {
            return Err(format!("`{}` has no closing `}}`", &rest[start..]));
        };
        let inner = &rest[start + 2..start + length - 1];
        let after = &rest[start + length..];
        let at_entry_start = base == 0 && starts_entry(text, consumed + start, spaces);
        let inner = translate(inner, base + 1, spaces, found)?;
        output.push_str(&translate_token(&inner, after, at_entry_start, found)?);
        consumed += start + length;
        rest = after;
    }
    output.push_str(rest);
    Ok(output)
}

/// The length of the `${...}` token `text` starts with, counting nested
/// tokens, or `None` when it is not closed.
fn token_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' && bytes.get(index + 1) == Some(&b'{') {
            depth += 1;
            index += 2;
            continue;
        }
        if bytes[index] == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(index + 1);
            }
        }
        index += 1;
    }
    None
}

/// The v2 text of one token whose (translated) inside is `token`; `after`
/// is the text that follows it.
fn translate_token(
    token: &str,
    after: &str,
    at_entry_start: bool,
    found: &mut V3Tokens,
) -> Result<String, String> {
    let written = format!("${{{token}}}");
    if token.starts_with("env:") || token.starts_with("secret:") {
        return Ok(written);
    }
    let path_token = |v2: String| -> Result<String, String> {
        // v2 reads `$provider.<name>` up to the first character that is not
        // part of a name, so one may not follow the token directly.
        if after.starts_with(|character: char| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        }) {
            return Err(format!(
                "`{written}` must be followed by `/` or the end of the value, not `{}`",
                after.chars().next().unwrap_or_default()
            ));
        }
        Ok(v2)
    };
    let starting = |v2: String| -> Result<String, String> {
        if at_entry_start {
            Ok(v2)
        } else {
            Err(format!(
                "`{written}` is a path and must start the value (or an entry of a `:` list)"
            ))
        }
    };
    let Some((namespace, name)) = token.split_once('.') else {
        return match token {
            "self" => starting("@self".into()),
            "choice" => Ok(written),
            alias if is_identifier(alias) => {
                found.aliases.push(alias.to_string());
                starting(format!("@{alias}"))
            }
            _ => Err(format!("`{written}` is not a token")),
        };
    };
    match namespace {
        "provider" if is_identifier(name) => path_token(format!("$provider.{name}")),
        "assembly" if matches!(name, "work" | "out") || name.starts_with("tree.") => {
            path_token(format!("$assembly.{name}"))
        }
        "assembly"
            if ["sha256:", "sha256_tree:", "verity:", "size:"]
                .iter()
                .any(|kind| name.starts_with(kind)) =>
        {
            Ok(written)
        }
        "source" if is_identifier(name) => starting(format!("@source:{name}")),
        "outputs" => match name {
            "dir" => Ok("${workspace.out_dir}".into()),
            _ => Err(format!(
                "`{written}` is not a token; only `${{outputs.dir}}` is"
            )),
        },
        "workspace" if matches!(name, "out_dir" | "export_dir") => Err(format!(
            "`{written}` is `${{outputs.{}}}` in config v3",
            if name == "out_dir" { "dir" } else { name }
        )),
        "inputs" => Err(format!("`{written}` is written `${{input.{name}}}`")),
        namespace if NAMESPACES.contains(&namespace) && namespace != "provider" => Ok(written),
        _ => Err(format!(
            "`{written}` is not a token: `{namespace}` is not a token namespace"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2(text: &str) -> Result<String, String> {
        v3_string_to_v2(text, &mut V3Tokens::default())
    }

    #[test]
    fn path_tokens_become_their_v2_spelling() {
        assert_eq!(
            v2("${provider.images}/Image").unwrap(),
            "$provider.images/Image"
        );
        assert_eq!(
            v2("${assembly.tree.boot}/x").unwrap(),
            "$assembly.tree.boot/x"
        );
        assert_eq!(v2("${self}/board").unwrap(), "@self/board");
        assert_eq!(v2("${source.bsp}/configs").unwrap(), "@source:bsp/configs");
        assert_eq!(
            v2("${self}/a:${source.bsp}/b").unwrap(),
            "@self/a:@source:bsp/b"
        );
        let mut found = V3Tokens::default();
        assert_eq!(
            v3_string_to_v2("${assets}/motd", &mut found).unwrap(),
            "@assets/motd"
        );
        assert_eq!(found.aliases, ["assets"]);
        assert_eq!(
            v2("${outputs.dir}/images/${build.name}").unwrap(),
            "${workspace.out_dir}/images/${build.name}"
        );
    }

    #[test]
    fn value_tokens_keep_their_spelling_with_v2_paths_inside() {
        assert_eq!(
            v2("${assembly.sha256:${assembly.work}/rootfs.erofs}").unwrap(),
            "${assembly.sha256:$assembly.work/rootfs.erofs}"
        );
        for text in [
            "${build.name}-${input.board}",
            "${env:HOME}",
            "${secret:TOKEN}",
            "${source.bsp.commit}",
            "${project.root_dir}/x",
        ] {
            assert_eq!(v2(text).unwrap(), text);
        }
    }

    #[test]
    fn v2_tokens_unknown_namespaces_and_misplaced_paths_are_refused() {
        for (text, expected) in [
            (
                "$provider.images/Image",
                "`$provider.images` is written `${provider.images}`",
            ),
            ("@self/x", "`@self` is written `${self}`"),
            ("@source:bsp/x", "`@source:bsp` is written `${source.bsp}`"),
            ("@assets/motd", "`@assets/` is written `${assets}/`"),
            ("${workspace.out_dir}/x", "is `${outputs.dir}` in config v3"),
            ("${inputs.board}", "is written `${input.board}`"),
            ("${bulid.name}", "`bulid` is not a token namespace"),
            ("a-${assets}/x", "must start the value"),
            ("a ${assets}/x", "must start the value"),
            ("${provider.images}x", "must be followed by `/`"),
            ("${build.name", "has no closing"),
        ] {
            let error = v2(text).expect_err(text);
            assert!(error.contains(expected), "{text}: {error}");
        }
    }

    #[test]
    fn override_values_take_path_tokens_in_space_separated_lists() {
        let mut found = V3Tokens::default();
        assert_eq!(
            v3_override_to_v2("${family}/a.fragment ${assets}/b.fragment", &mut found).unwrap(),
            "@family/a.fragment @assets/b.fragment"
        );
        assert_eq!(found.aliases, ["family", "assets"]);
        assert_eq!(
            v3_override_to_v2("\"${assets}/a ${source.bsp}/b\"", &mut V3Tokens::default()).unwrap(),
            "\"@assets/a @source:bsp/b\""
        );
        assert!(v3_override_to_v2("a-${assets}/x", &mut V3Tokens::default()).is_err());
    }
}
