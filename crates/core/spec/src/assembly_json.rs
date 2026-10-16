//! JSON documents generated at assembly time: an archive member or a tree
//! file written from a `json = { ... }` table.
//!
//! Object keys keep the order the config wrote them, so the output is
//! deterministic and reads like the config. Strings are rendered through the
//! `${assembly...}` token renderer ([`assembly_value_tokens`]). A string that
//! is exactly one numeric token (`${assembly.size:...}`, a verity
//! `hash_offset`, `data_blocks`, ...) is written as a JSON number. The output
//! is pretty-printed with two-space indentation and ends with a newline.

use crate::{AssemblyValueToken, assembly_value_tokens};

/// One JSON value of a generated document.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// value; an error stops the render.
    pub fn render(
        &self,
        resolve: &mut dyn FnMut(&AssemblyValueToken) -> Result<String, String>,
    ) -> Result<String, String> {
        let mut out = String::new();
        write_value(self, 0, resolve, &mut out)?;
        out.push('\n');
        Ok(out)
    }
}

fn write_value(
    value: &AssemblyJsonValue,
    depth: usize,
    resolve: &mut dyn FnMut(&AssemblyValueToken) -> Result<String, String>,
    out: &mut String,
) -> Result<(), String> {
    match value {
        AssemblyJsonValue::String(text) => write_string(text, resolve, out)?,
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
                write_value(item, depth + 1, resolve, out)?;
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
                write_value(value, depth + 1, resolve, out)?;
                push_separator(out, index, members.len());
            }
            push_indent(out, depth);
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(
    text: &str,
    resolve: &mut dyn FnMut(&AssemblyValueToken) -> Result<String, String>,
    out: &mut String,
) -> Result<(), String> {
    let mut tokens = Vec::new();
    let rendered = assembly_value_tokens(text, |token| {
        tokens.push(token.clone());
        resolve(token)
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
    fn rendering_is_repeatable() {
        let document = object(vec![("k", text("${assembly.size:$assembly.work/x}"))]);
        let first = document.render(&mut describe).expect("first");
        let second = document.render(&mut describe).expect("second");
        assert_eq!(first, second);
    }
}
