//! JSON documents generated at assembly time: an archive member or a tree
//! file written from a `json = { ... }` table.
//!
//! Object keys keep the order the config wrote them, so the output is
//! deterministic and reads like the config. Strings are rendered through the
//! `${assembly...}` token renderer ([`assembly_value_tokens`]). A string that
//! is exactly one numeric token (`${assembly.size:...}`, a verity
//! `hash_offset`, `data_blocks`, ...) is written as a JSON number. The output
//! is pretty-printed with two-space indentation and ends with a newline.
//!
//! A string that is exactly `${assembly.sha256_tree:<tree or path>}` is
//! written as an object mapping each regular file's path under that tree to
//! its sha256 hex, keys sorted (see [`AssemblyJsonValue::render_with_trees`]).

use crate::{
    AssemblyPathTemplate, AssemblyValueToken, assembly_value_tokens, whole_tree_digest_token,
};

/// Resolves a `${assembly.sha256_tree:...}` value: each regular file's path
/// under the tree, with its sha256 hex.
pub type TreeDigestResolver<'a> =
    dyn FnMut(&AssemblyPathTemplate) -> Result<Vec<(String, String)>, String> + 'a;

/// One JSON value of a generated document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum AssemblyJsonValue {
    String(String),
    Integer(i64),
    Bool(bool),
    Array(Vec<AssemblyJsonValue>),
    /// Members in the order the config wrote them.
    Object(Vec<(String, AssemblyJsonValue)>),
}

impl AssemblyJsonValue {
    /// Every string value, at any depth, in document order. Keys are not
    /// included: tokens are only rendered in values.
    pub fn strings(&self) -> Vec<&str> {
        let mut found = Vec::new();
        self.collect_strings(&mut found);
        found
    }

    fn collect_strings<'a>(&'a self, found: &mut Vec<&'a str>) {
        match self {
            Self::String(text) => found.push(text),
            Self::Integer(_) | Self::Bool(_) => {}
            Self::Array(items) => {
                for item in items {
                    item.collect_strings(found);
                }
            }
            Self::Object(members) => {
                for (_, value) in members {
                    value.collect_strings(found);
                }
            }
        }
    }

    /// The document text. `resolve` supplies each `${assembly...}` token's
    /// value; an error stops the render. A tree digest token is an error
    /// here: use [`Self::render_with_trees`].
    pub fn render(
        &self,
        resolve: &mut dyn FnMut(&AssemblyValueToken) -> Result<String, String>,
    ) -> Result<String, String> {
        self.render_with_trees(resolve, &mut |path| {
            Err(format!(
                "'${{assembly.sha256_tree:{}}}' needs a tree resolver",
                path.as_str()
            ))
        })
    }

    /// The document text. `resolve` supplies each `${assembly...}` token's
    /// value and `trees` each tree digest's members (path, sha256), which
    /// are written sorted by path. An error stops the render.
    pub fn render_with_trees(
        &self,
        resolve: &mut dyn FnMut(&AssemblyValueToken) -> Result<String, String>,
        trees: &mut TreeDigestResolver<'_>,
    ) -> Result<String, String> {
        let mut out = String::new();
        let mut resolvers = Resolvers { resolve, trees };
        write_value(self, 0, &mut resolvers, &mut out)?;
        out.push('\n');
        Ok(out)
    }
}

/// The two value sources a render draws on.
struct Resolvers<'a, 'b> {
    resolve: &'a mut dyn FnMut(&AssemblyValueToken) -> Result<String, String>,
    trees: &'a mut TreeDigestResolver<'b>,
}

fn write_value(
    value: &AssemblyJsonValue,
    depth: usize,
    resolvers: &mut Resolvers<'_, '_>,
    out: &mut String,
) -> Result<(), String> {
    match value {
        AssemblyJsonValue::String(text) => match whole_tree_digest_token(text) {
            Some(token) => {
                let mut members = (resolvers.trees)(&token.path)?;
                members.sort_by(|a, b| a.0.cmp(&b.0));
                write_digest_map(&members, depth, out);
            }
            None => write_string(text, resolvers, out)?,
        },
        AssemblyJsonValue::Integer(number) => out.push_str(&number.to_string()),
        AssemblyJsonValue::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        AssemblyJsonValue::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                push_indent(out, depth + 1);
                write_value(item, depth + 1, resolvers, out)?;
                push_separator(out, index, items.len());
            }
            push_indent(out, depth);
            out.push(']');
        }
        AssemblyJsonValue::Object(members) => {
            if members.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push_str("{\n");
            for (index, (key, value)) in members.iter().enumerate() {
                push_indent(out, depth + 1);
                out.push_str(&json_string(key));
                out.push_str(": ");
                write_value(value, depth + 1, resolvers, out)?;
                push_separator(out, index, members.len());
            }
            push_indent(out, depth);
            out.push('}');
        }
    }
    Ok(())
}

/// An object of `path: sha256` members, one per line, at `depth`.
fn write_digest_map(members: &[(String, String)], depth: usize, out: &mut String) {
    if members.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    for (index, (path, sha256)) in members.iter().enumerate() {
        push_indent(out, depth + 1);
        out.push_str(&json_string(path));
        out.push_str(": ");
        out.push_str(&json_string(sha256));
        push_separator(out, index, members.len());
    }
    push_indent(out, depth);
    out.push('}');
}

fn write_string(
    text: &str,
    resolvers: &mut Resolvers<'_, '_>,
    out: &mut String,
) -> Result<(), String> {
    let mut tokens = Vec::new();
    let rendered = assembly_value_tokens(text, |token| {
        tokens.push(token.clone());
        (resolvers.resolve)(token)
    })?;
    if tokens.len() == 1 && is_whole_token(text) && tokens[0].is_numeric() {
        let number = rendered.trim().parse::<i64>().map_err(|_| {
            format!(
                "'{text}' renders to '{rendered}', which is not an integer; a JSON number needs one"
            )
        })?;
        out.push_str(&number.to_string());
    } else {
        out.push_str(&json_string(&rendered));
    }
    Ok(())
}

/// Whether the text is exactly one `${...}` token, with nothing around it.
fn is_whole_token(text: &str) -> bool {
    text.starts_with("${")
        && text.ends_with('}')
        && text.matches("${").count() == 1
        && !text[2..text.len() - 1].contains('}')
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn push_separator(out: &mut String, index: usize, len: usize) {
    if index + 1 < len {
        out.push(',');
    }
    out.push('\n');
}

/// `text` as a JSON string literal.
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AssemblyVerityField;

    fn text(value: &str) -> AssemblyJsonValue {
        AssemblyJsonValue::String(value.into())
    }

    fn object(members: Vec<(&str, AssemblyJsonValue)>) -> AssemblyJsonValue {
        AssemblyJsonValue::Object(
            members
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    /// Renders tokens as `<path>|<field or size>` so the tests can see them.
    fn describe(token: &AssemblyValueToken) -> Result<String, String> {
        match token {
            AssemblyValueToken::Digest(token) => Ok(format!("digest:{}", token.path.as_str())),
            AssemblyValueToken::Size(token) => Ok(format!("{}", size_of(&token.path))),
            AssemblyValueToken::Verity(token) => Ok(match token.field {
                AssemblyVerityField::HashOffset => "4096".into(),
                AssemblyVerityField::DataBlocks => "12".into(),
                other => format!("hex-{}", other.as_str()),
            }),
            AssemblyValueToken::TreeDigest(_) => Err("tree digests need render_with_trees".into()),
        }
    }

    fn size_of(path: &crate::AssemblyPathTemplate) -> u64 {
        path.as_str().len() as u64
    }

    #[test]
    fn keys_keep_their_written_order_and_layout_is_pretty() {
        let document = object(vec![
            ("zeta", AssemblyJsonValue::Integer(1)),
            ("alpha", text("a")),
            (
                "list",
                AssemblyJsonValue::Array(vec![AssemblyJsonValue::Bool(true)]),
            ),
            ("empty_object", object(vec![])),
            ("empty_array", AssemblyJsonValue::Array(vec![])),
        ]);
        let rendered = document.render(&mut describe).expect("render");
        assert_eq!(
            rendered,
            "{\n  \"zeta\": 1,\n  \"alpha\": \"a\",\n  \"list\": [\n    true\n  ],\n  \"empty_object\": {},\n  \"empty_array\": []\n}\n"
        );
    }

    #[test]
    fn numeric_tokens_alone_become_numbers_and_others_stay_strings() {
        let document = object(vec![
            ("offset", text("${assembly.verity:x:hash_offset}")),
            ("blocks", text("${assembly.verity:x:data_blocks}")),
            ("size", text("${assembly.size:$assembly.work/a.img}")),
            ("root", text("${assembly.verity:x:root_hash}")),
            ("mixed", text("n=${assembly.verity:x:data_blocks}")),
        ]);
        let rendered = document.render(&mut describe).expect("render");
        assert!(rendered.contains("\"offset\": 4096,"), "{rendered}");
        assert!(rendered.contains("\"blocks\": 12,"), "{rendered}");
        assert!(rendered.contains("\"size\": 20,"), "{rendered}");
        assert!(
            rendered.contains("\"root\": \"hex-root_hash\","),
            "{rendered}"
        );
        assert!(rendered.contains("\"mixed\": \"n=12\""), "{rendered}");
    }

    #[test]
    fn a_numeric_token_that_is_not_an_integer_is_an_error() {
        let document = text("${assembly.verity:x:root_hash}");
        let mut numeric_root =
            |_: &AssemblyValueToken| -> Result<String, String> { Ok("abc".to_string()) };
        let mut numeric_size = |token: &AssemblyValueToken| -> Result<String, String> {
            match token {
                AssemblyValueToken::Verity(_) => Ok("not-a-number".to_string()),
                _ => Ok(String::new()),
            }
        };
        // root_hash is not numeric: the text is written as a string.
        assert!(
            document
                .render(&mut numeric_root)
                .expect("string")
                .contains("\"abc\"")
        );
        let numeric = text("${assembly.verity:x:hash_offset}");
        let error = numeric.render(&mut numeric_size).expect_err("not integer");
        assert!(error.contains("not an integer"), "{error}");
    }

    #[test]
    fn strings_are_escaped_and_nested_values_render_at_depth() {
        let document = object(vec![(
            "boot",
            object(vec![(
                "files",
                object(vec![(
                    "kernel8.img",
                    text("quote \" slash \\ tab \t line\n ctl \u{1}"),
                )]),
            )]),
        )]);
        let rendered = document.render(&mut describe).expect("render");
        assert_eq!(
            rendered,
            "{\n  \"boot\": {\n    \"files\": {\n      \"kernel8.img\": \"quote \\\" slash \\\\ tab \\t line\\n ctl \\u0001\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn strings_lists_every_value_in_order() {
        let document = object(vec![
            ("a", text("one")),
            (
                "b",
                AssemblyJsonValue::Array(vec![text("two"), AssemblyJsonValue::Integer(3)]),
            ),
        ]);
        assert_eq!(document.strings(), vec!["one", "two"]);
    }

    #[test]
    fn a_tree_digest_renders_its_members_sorted_at_depth() {
        let document = object(vec![
            ("files", text("${assembly.sha256_tree:payload}")),
            ("none", text("${assembly.sha256_tree:empty}")),
        ]);
        let rendered = document
            .render_with_trees(&mut describe, &mut |path| match path.as_str() {
                "payload" => Ok(vec![
                    ("nested/b.txt".into(), "bb".into()),
                    ("a.txt".into(), "aa".into()),
                ]),
                _ => Ok(Vec::new()),
            })
            .expect("render");
        assert_eq!(
            rendered,
            "{\n  \"files\": {\n    \"a.txt\": \"aa\",\n    \"nested/b.txt\": \"bb\"\n  },\n  \"none\": {}\n}\n"
        );
    }

    #[test]
    fn a_tree_digest_is_a_whole_json_value_and_needs_a_resolver() {
        let whole = text("${assembly.sha256_tree:payload}");
        assert!(whole.render(&mut describe).is_err(), "no tree resolver");
        let nested = text("x${assembly.sha256_tree:payload}");
        assert!(
            nested
                .render_with_trees(&mut describe, &mut |_| Ok(Vec::new()))
                .is_err(),
            "not a whole value"
        );
        assert!(whole_tree_digest_token("${assembly.sha256_tree: }").is_none());
        assert_eq!(
            whole_tree_digest_token("${assembly.sha256_tree: $assembly.work/x }")
                .expect("token")
                .path
                .as_str(),
            "$assembly.work/x"
        );
    }

    #[test]
    fn rendering_is_repeatable() {
        let document = object(vec![("k", text("${assembly.size:$assembly.work/x}"))]);
        let first = document.render(&mut describe).expect("first");
        let second = document.render(&mut describe).expect("second");
        assert_eq!(first, second);
    }
}
