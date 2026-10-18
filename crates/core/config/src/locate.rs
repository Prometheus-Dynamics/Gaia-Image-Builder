//! Where things sit in a build file, as `file:line:col`.
//!
//! A parse or type error carries toml's span for the failing position, which
//! is converted to a line and column here. A key is found by reading the
//! file's own tokens with `toml_parser`, the parser `toml` itself uses, and
//! recording where each key and table header starts, by the dotted path the
//! unknown-key check reports (`stage.files[0].owner`).

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use toml_parser::ErrorSink;
use toml_parser::Source;
use toml_parser::Span;
use toml_parser::decoder::Encoding;
use toml_parser::parser::{EventReceiver, parse_document};

/// The 1-based line and column of byte `offset` in `text`. Columns count
/// characters, as an editor shows them.
pub(crate) fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    (line, before[line_start..].chars().count() + 1)
}

/// `path:line:col` of byte `offset` in the file at `path`, whose text is
/// `text`.
pub(crate) fn at_offset(path: &Path, text: &str, offset: usize) -> String {
    let (line, column) = line_col(text, offset);
    format!("{}:{line}:{column}", path.display())
}

/// `path:line:col` of a toml error's span in the file's `text`, or the bare
/// path when toml gave no span.
pub(crate) fn toml_error_location(path: &Path, text: &str, span: Option<Range<usize>>) -> String {
    match span {
        Some(span) => at_offset(path, text, span.start),
        None => path.display().to_string(),
    }
}

/// Where each key and table header of a file starts, by its dotted path.
#[derive(Debug, Default)]
pub(crate) struct KeyLocations {
    offsets: BTreeMap<String, usize>,
}

impl KeyLocations {
    /// Scans a file's text. Text that is not valid TOML yields what could be
    /// read before the error; callers only ask about keys that parsed.
    pub(crate) fn scan(text: &str) -> Self {
        let tokens = Source::new(text).lex().into_vec();
        let mut scanner = Scanner::new(text);
        parse_document(&tokens, &mut scanner, &mut ());
        Self {
            offsets: scanner.offsets,
        }
    }

    /// Byte offset of the key or table at `path`, as the unknown-key check
    /// names it.
    pub(crate) fn offset(&self, path: &str) -> Option<usize> {
        self.offsets.get(path).copied()
    }
}

/// A table header being read: whether it is `[[...]]`, its key segments and
/// where it starts.
struct Header {
    array: bool,
    segments: Vec<String>,
    start: usize,
}

struct Scanner<'text> {
    text: &'text str,
    /// The indexed path of the table the key-values read now belong to.
    table: String,
    header: Option<Header>,
    /// The segments of the key being read, and where it starts.
    key: Vec<String>,
    key_start: Option<usize>,
    /// Depth inside inline tables and arrays, whose keys are not table keys.
    nesting: usize,
    /// Each array of tables (indexed by its parent) and how many elements it
    /// has so far.
    element_counts: BTreeMap<String, usize>,
    offsets: BTreeMap<String, usize>,
}

impl<'text> Scanner<'text> {
    fn new(text: &'text str) -> Self {
        Self {
            text,
            table: String::new(),
            header: None,
            key: Vec::new(),
            key_start: None,
            nesting: 0,
            element_counts: BTreeMap::new(),
            offsets: BTreeMap::new(),
        }
    }

    /// The key text of a key token, without its quotes.
    fn key_text(&self, span: Span) -> String {
        let raw = self.text[span.start()..span.end()].trim();
        let quoted = raw.len() >= 2
            && ((raw.starts_with('"') && raw.ends_with('"'))
                || (raw.starts_with('\'') && raw.ends_with('\'')));
        if quoted {
            raw[1..raw.len() - 1].to_string()
        } else {
            raw.to_string()
        }
    }

    /// The indexed path a sequence of key segments names: each array of
    /// tables on the way is entered at its latest element.
    fn resolve(&self, segments: &[String]) -> String {
        let mut path = String::new();
        for segment in segments {
            let candidate = join(&path, segment);
            path = match self.element_counts.get(&candidate) {
                Some(count) => format!("{candidate}[{}]", count.saturating_sub(1)),
                None => candidate,
            };
        }
        path
    }

    fn close_header(&mut self) {
        let Some(header) = self.header.take() else {
            return;
        };
        let table = if header.array {
            let Some((last, prefix)) = header.segments.split_last() else {
                return;
            };
            let base = join(&self.resolve(prefix), last);
            let count = self.element_counts.entry(base.clone()).or_insert(0);
            let index = *count;
            *count += 1;
            format!("{base}[{index}]")
        } else {
            self.resolve(&header.segments)
        };
        self.offsets.entry(table.clone()).or_insert(header.start);
        self.table = table;
    }
}

fn join(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

impl EventReceiver for Scanner<'_> {
    fn std_table_open(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        self.header = Some(Header {
            array: false,
            segments: Vec::new(),
            start: span.start(),
        });
    }

    fn std_table_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.close_header();
    }

    fn array_table_open(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        self.header = Some(Header {
            array: true,
            segments: Vec::new(),
            start: span.start(),
        });
    }

    fn array_table_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.close_header();
    }

    fn inline_table_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.nesting += 1;
        true
    }

    fn inline_table_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.nesting = self.nesting.saturating_sub(1);
    }

    fn array_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.nesting += 1;
        true
    }

    fn array_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.nesting = self.nesting.saturating_sub(1);
    }

    fn simple_key(&mut self, span: Span, _kind: Option<Encoding>, _error: &mut dyn ErrorSink) {
        if self.nesting > 0 {
            return;
        }
        let text = self.key_text(span);
        match &mut self.header {
            Some(header) => header.segments.push(text),
            None => {
                if self.key.is_empty() {
                    self.key_start = Some(span.start());
                }
                self.key.push(text);
            }
        }
    }

    fn key_val_sep(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        let key = std::mem::take(&mut self.key);
        let Some(start) = self.key_start.take() else {
            return;
        };
        if self.nesting == 0 {
            let path = join(&self.table, &key.join("."));
            self.offsets.entry(path).or_insert(start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line and column of a key's path in `text`.
    fn offset_of(text: &str, path: &str) -> Option<(usize, usize)> {
        KeyLocations::scan(text)
            .offset(path)
            .map(|offset| line_col(text, offset))
    }

    #[test]
    fn line_and_column_count_from_one_and_characters() {
        assert_eq!(line_col("ab\ncd", 0), (1, 1));
        assert_eq!(line_col("ab\ncd", 4), (2, 2));
        assert_eq!(line_col("é=1\nx", 3), (1, 3));
    }

    #[test]
    fn keys_and_tables_are_found_by_their_dotted_path() {
        let text = "build_name = \"a\"\n[workspace]\nroot_dir = \".\"\n\
                    [providers]\njava.gradle_hme = 1\n";
        assert_eq!(offset_of(text, "build_name"), Some((1, 1)));
        assert_eq!(offset_of(text, "workspace"), Some((2, 1)));
        assert_eq!(offset_of(text, "workspace.root_dir"), Some((3, 1)));
        assert_eq!(offset_of(text, "providers.java.gradle_hme"), Some((5, 1)));
    }

    #[test]
    fn array_tables_are_indexed_by_element() {
        let text = "[[stage.files]]\nid = \"a\"\n\
                    [[stage.files]]\nid = \"b\"\nowner = \"root\"\n";
        assert_eq!(offset_of(text, "stage.files[0].id"), Some((2, 1)));
        assert_eq!(offset_of(text, "stage.files[1].owner"), Some((5, 1)));
        assert_eq!(offset_of(text, "stage.files[2].id"), None);
    }

    #[test]
    fn nested_array_tables_follow_their_parent_element() {
        let text = "[[image.assembly.disks]]\nid = \"a\"\n\
                    [[image.assembly.disks.partitions]]\nname = \"p\"\n\
                    [[image.assembly.disks]]\nid = \"b\"\n\
                    [[image.assembly.disks.partitions]]\nname = \"q\"\nlabel = 1\n";
        assert_eq!(
            offset_of(text, "image.assembly.disks[1].partitions[0].label"),
            Some((9, 1))
        );
        assert_eq!(
            offset_of(text, "image.assembly.disks[1].partitions[0].name"),
            Some((8, 1))
        );
        assert_eq!(
            offset_of(text, "image.assembly.disks[0].partitions[0].name"),
            Some((4, 1))
        );
    }

    #[test]
    fn keys_inside_inline_tables_and_arrays_are_not_table_keys() {
        let text = "x = { a = 1 }\ny = [{ b = 2 }]\n";
        assert_eq!(offset_of(text, "x"), Some((1, 1)));
        assert_eq!(offset_of(text, "x.a"), None);
        assert_eq!(offset_of(text, "b"), None);
    }

    #[test]
    fn quoted_keys_are_found_without_their_quotes() {
        let text = "[sources.\"app\"]\n\"my key\" = 1\n";
        assert_eq!(offset_of(text, "sources.app.my key"), Some((2, 1)));
    }
}
