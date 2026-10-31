//! `gaia migrate`'s rewrite of one v2 file into v3, as text: comments, key
//! order and formatting stay as written. Whether the rewrite builds the same
//! thing is checked by resolving the build before and after (see the app's
//! migrate command); this module only spells the file in v3.
//!
//! - Tokens are respelled everywhere in the file, comments included, so
//!   the comments that explain a value keep naming it correctly.
//! - `[image.output]` becomes `[outputs]`, `[providers.buildroot.ccache]`
//!   `[cache.ccache]` and `[providers.buildroot.package_cache]`
//!   `[cache.packages]`.
//! - The keys that moved out of `[workspace]`, `[providers.buildroot]` and
//!   `[providers.rust]` are cut, with the comment lines right above them,
//!   and added to `[outputs]` or `[cache]` (made after the table they came
//!   from when the file has none).
//! - `config = 3` is added before the first top-level key.
//!
//! A v2 key written some other way (a dotted key such as
//! `workspace.out_dir = ...` at the top level) is left as is; the v3 reader
//! then rejects the file at that line, and the migration is undone.

use toml_parser::parser::{EventReceiver, parse_document};
use toml_parser::{ErrorSink, Source, Span};

use super::VERSION_KEY;
use super::tokens::alias_is_reserved;

/// A v2 file in v3 spelling, or why it cannot be rewritten.
pub fn migrate_config_text(text: &str) -> Result<String, String> {
    if let Ok(value) = toml::from_str::<toml::Table>(text)
        && value.contains_key(VERSION_KEY)
    {
        return Ok(text.to_string());
    }
    let respelled = respell_tokens(text)?;
    restructure(&respelled)
}

// Tokens.

fn is_name_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
}

/// The v3 spelling of every v2 token in `text`.
fn respell_tokens(text: &str) -> Result<String, String> {
    let mut output = String::with_capacity(text.len() + text.len() / 8);
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        let previous = text[..index].chars().next_back();
        if let Some((written, length)) = respell_at(rest, previous)? {
            output.push_str(&written);
            index += length;
        } else {
            let character = rest.chars().next().expect("index is inside text");
            output.push(character);
            index += character.len_utf8();
        }
    }
    Ok(output
        .replace("${workspace.out_dir}", "${outputs.dir}")
        .replace("${inputs.", "${input."))
}

/// The v3 spelling of a v2 token at the start of `rest`, and how many bytes
/// of `rest` it replaces. `previous` is the character before it.
fn respell_at(rest: &str, previous: Option<char>) -> Result<Option<(String, usize)>, String> {
    for namespace in ["$provider.", "$assembly."] {
        if let Some(after_namespace) = rest.strip_prefix(namespace) {
            let name_length = after_namespace
                .find(|character: char| !(is_name_char(character) || character == '.'))
                .unwrap_or(after_namespace.len());
            // A sentence may end right after a token in a comment.
            let name = after_namespace[..name_length].trim_end_matches(['.', '-']);
            if name.is_empty() {
                return Ok(None);
            }
            let length = namespace.len() + name.len();
            return Ok(Some((format!("${{{}{name}}}", &namespace[1..]), length)));
        }
    }
    if !rest.starts_with('@') || previous.is_some_and(is_name_char) {
        return Ok(None);
    }
    let name_length = rest[1..]
        .find(|character: char| !is_name_char(character))
        .unwrap_or(rest.len() - 1);
    let name = &rest[1..1 + name_length];
    let after = &rest[1 + name_length..];
    match name {
        "self" => Ok(Some(("${self}".into(), 5))),
        "source" if after.starts_with(':') => {
            let id_length = after[1..]
                .find(|character: char| !is_name_char(character))
                .unwrap_or(after.len() - 1);
            if id_length == 0 {
                return Ok(None);
            }
            let id = &after[1..1 + id_length];
            Ok(Some((
                format!("${{source.{id}}}"),
                1 + name_length + 1 + id_length,
            )))
        }
        alias if !alias.is_empty() && after.starts_with('/') => {
            if alias_is_reserved(alias) {
                return Err(format!(
                    "the named path alias '{alias}' would read as `${{{alias}}}`, which config v3 \
                     reserves; rename the alias first"
                ));
            }
            Ok(Some((format!("${{{alias}}}"), 1 + name_length)))
        }
        _ => Ok(None),
    }
}

// Structure.

/// A table header or a top-level key-value pair, by the lines it covers.
#[derive(Debug)]
enum Item {
    Header {
        line: usize,
        array: bool,
        path: String,
    },
    KeyValue {
        first_line: usize,
        last_line: usize,
        key: String,
    },
}

/// Collects the [`Item`]s of a document from `toml_parser`'s events.
struct Items<'text> {
    text: &'text str,
    items: Vec<Item>,
    header: Option<(bool, usize, Vec<String>)>,
    key: Vec<String>,
    key_start: Option<usize>,
    /// Where the pending top-level value began, while its value is read.
    value_key: Option<(usize, String)>,
    nesting: usize,
}

impl Items<'_> {
    fn line(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .matches('\n')
            .count()
    }

    fn key_text(&self, span: Span) -> String {
        self.text[span.start()..span.end()].trim().to_string()
    }

    fn value_ends(&mut self, end: usize) {
        if self.nesting == 0
            && let Some((start, key)) = self.value_key.take()
        {
            let item = Item::KeyValue {
                first_line: self.line(start),
                last_line: self.line(end.saturating_sub(1)),
                key,
            };
            self.items.push(item);
        }
    }
}

impl EventReceiver for Items<'_> {
    fn std_table_open(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        self.header = Some((false, span.start(), Vec::new()));
    }

    fn std_table_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.close_header();
    }

    fn array_table_open(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        self.header = Some((true, span.start(), Vec::new()));
    }

    fn array_table_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.close_header();
    }

    fn inline_table_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.nesting += 1;
        true
    }

    fn inline_table_close(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        self.nesting = self.nesting.saturating_sub(1);
        self.value_ends(span.end());
    }

    fn array_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.nesting += 1;
        true
    }

    fn array_close(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        self.nesting = self.nesting.saturating_sub(1);
        self.value_ends(span.end());
    }

    fn simple_key(
        &mut self,
        span: Span,
        _kind: Option<toml_parser::decoder::Encoding>,
        _error: &mut dyn ErrorSink,
    ) {
        if self.nesting > 0 {
            return;
        }
        let text = self.key_text(span);
        match &mut self.header {
            Some((_, _, segments)) => segments.push(text),
            None => {
                if self.key.is_empty() {
                    self.key_start = Some(span.start());
                }
                self.key.push(text);
            }
        }
    }

    fn key_val_sep(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        if self.nesting > 0 {
            return;
        }
        let key = std::mem::take(&mut self.key).join(".");
        if let Some(start) = self.key_start.take() {
            self.value_key = Some((start, key));
        }
    }

    fn scalar(
        &mut self,
        span: Span,
        _kind: Option<toml_parser::decoder::Encoding>,
        _error: &mut dyn ErrorSink,
    ) {
        if self.header.is_none() && self.key_start.is_none() {
            self.value_ends(span.end());
        }
    }
}

impl Items<'_> {
    fn close_header(&mut self) {
        if let Some((array, start, segments)) = self.header.take() {
            let line = self.line(start);
            self.items.push(Item::Header {
                line,
                array,
                path: segments.join("."),
            });
        }
    }
}

fn items(text: &str) -> Vec<Item> {
    let tokens = Source::new(text).lex().into_vec();
    let mut items = Items {
        text,
        items: Vec::new(),
        header: None,
        key: Vec::new(),
        key_start: None,
        value_key: None,
        nesting: 0,
    };
    parse_document(&tokens, &mut items, &mut ());
    items.items
}

/// Where a v2 key of a table goes in v3: the v3 table and key.
fn moved_key(table: &str, key: &str) -> Option<(&'static str, &'static str)> {
    match (table, key) {
        ("workspace", "out_dir") => Some(("outputs", "dir")),
        ("workspace", "export_dir") => Some(("outputs", "export_dir")),
        ("providers.buildroot", "download_dir") => Some(("cache", "downloads")),
        ("providers.buildroot", "ccache") => Some(("cache", "ccache")),
        ("providers.buildroot", "package_cache") => Some(("cache", "packages")),
        ("providers.rust", "sccache") => Some(("cache", "sccache")),
        ("providers.rust", "shared_target_dir") => Some(("cache", "shared_target_dir")),
        _ => None,
    }
}

/// A v2 table's v3 header, and how its keys are renamed.
fn renamed_table(path: &str) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    match path {
        "image.output" => Some((
            "outputs",
            &[("collect_dir", "images_dir"), ("emit_report", "report")],
        )),
        "providers.buildroot.ccache" => Some(("cache.ccache", &[])),
        "providers.buildroot.package_cache" => Some(("cache.packages", &[])),
        _ => None,
    }
}

/// A table of the file: its header line (none for the top of the file) and
/// the lines up to the next header.
struct Section {
    header: Option<String>,
    path: String,
    array: bool,
    lines: Vec<String>,
    /// A key was moved out of it: dropped when nothing else is left.
    emptied: bool,
}

/// Moves the v2 tables and keys to their v3 places, and adds `config = 3`.
fn restructure(text: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let items = items(text);

    // Which item starts each line, to walk the file line by line.
    let mut headers = std::collections::BTreeMap::new();
    let mut values = std::collections::BTreeMap::new();
    for item in &items {
        match item {
            Item::Header { line, array, path } => {
                headers.insert(*line, (*array, path.clone()));
            }
            Item::KeyValue {
                first_line,
                last_line,
                key,
            } => {
                values.insert(*first_line, (*last_line, key.clone()));
            }
        }
    }

    let mut sections = vec![Section {
        header: None,
        path: String::new(),
        array: false,
        lines: Vec::new(),
        emptied: false,
    }];
    // Moved entries by v3 table, and the section each first came from.
    let mut moved: Vec<(&'static str, usize, Vec<String>)> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if let Some((array, path)) = headers.get(&index) {
            let (header, path) = match (*array, renamed_table(path)) {
                (false, Some((v3, _))) => {
                    (lines[index].replacen(path.as_str(), v3, 1), path.clone())
                }
                _ => (lines[index].to_string(), path.clone()),
            };
            if !array
                && path.starts_with("providers.")
                && (path.ends_with(".ccache") || path.ends_with(".package_cache"))
                && renamed_table(&path).is_none()
            {
                return Err(format!(
                    "[{path}] has no effect (only Buildroot reads a ccache or package cache); remove it before migrating"
                ));
            }
            sections.push(Section {
                header: Some(header),
                path,
                array: *array,
                lines: Vec::new(),
                emptied: false,
            });
            index += 1;
            continue;
        }
        let section = sections.len() - 1;
        if let Some((last_line, key)) = values.get(&index) {
            let table = sections[section].path.clone();
            let array = sections[section].array;
            let target = if array { None } else { moved_key(&table, key) };
            if let Some((v3_table, v3_key)) = target {
                // The comment lines right above the key go with it.
                sections[section].emptied = true;
                let body = &mut sections[section].lines;
                let mut comments = Vec::new();
                while body
                    .last()
                    .is_some_and(|line| line.trim_start().starts_with('#'))
                {
                    comments.insert(0, body.pop().expect("checked"));
                }
                let mut entry = comments;
                entry.push(lines[index].replacen(key.as_str(), v3_key, 1));
                entry.extend(
                    lines[index + 1..=*last_line]
                        .iter()
                        .map(ToString::to_string),
                );
                match moved.iter_mut().find(|(table, _, _)| *table == v3_table) {
                    Some((_, _, entries)) => entries.extend(entry),
                    None => moved.push((v3_table, section, entry)),
                }
                index = last_line + 1;
                continue;
            }
            let renames = if array {
                None
            } else {
                renamed_table(&table).map(|(_, renames)| renames)
            };
            let first = match renames.and_then(|renames| renames.iter().find(|(v2, _)| v2 == key)) {
                Some((v2, v3)) => lines[index].replacen(v2, v3, 1),
                None => lines[index].to_string(),
            };
            sections[section].lines.push(first);
            for line in &lines[index + 1..=*last_line] {
                sections[section].lines.push((*line).to_string());
            }
            index = last_line + 1;
            continue;
        }
        sections[section].lines.push(lines[index].to_string());
        index += 1;
    }

    // Each moved entry joins its v3 table, or a new one after the table the
    // first entry came from. Later insertions shift the indexes, so go from
    // the last source section back.
    moved.sort_by_key(|(_, source, _)| std::cmp::Reverse(*source));
    for (table, source, mut entries) in moved {
        if !entries.last().is_some_and(|line| line.ends_with('\n')) {
            entries.push("\n".into());
        }
        let existing = sections.iter().position(|section| {
            !section.array && section.header.is_some() && section.path == table
        });
        let existing = existing.or_else(|| renamed_table_target(&sections, table));
        match existing {
            Some(position) => append_body(&mut sections[position].lines, entries),
            None => {
                let mut lines = entries;
                lines.push("\n".into());
                ensure_blank_line_before(&mut sections[source].lines);
                sections.insert(
                    source + 1,
                    Section {
                        header: Some(format!("[{table}]\n")),
                        path: table.to_string(),
                        array: false,
                        lines,
                        emptied: false,
                    },
                );
            }
        }
    }

    // A table that only held moved keys is dropped.
    sections.retain(|section| {
        !section.emptied
            || section.header.is_none()
            || section.lines.iter().any(|line| !line.trim().is_empty())
    });

    add_version_key(&mut sections[0].lines);
    let mut output = String::with_capacity(text.len() + 64);
    for section in sections {
        if let Some(header) = section.header {
            output.push_str(&header);
        }
        for line in section.lines {
            output.push_str(&line);
        }
    }
    Ok(output)
}

/// The section a renamed v2 header became, when it holds the v3 table.
fn renamed_table_target(sections: &[Section], table: &str) -> Option<usize> {
    sections.iter().position(|section| {
        !section.array && renamed_table(&section.path).is_some_and(|(v3, _)| v3 == table)
    })
}

/// Appends `entries` to a table's lines, before its trailing blank lines.
fn append_body(body: &mut Vec<String>, entries: Vec<String>) {
    let mut blanks = Vec::new();
    while body.last().is_some_and(|line| line.trim().is_empty()) {
        blanks.push(body.pop().expect("checked"));
    }
    if !body.last().is_none_or(|line| line.ends_with('\n')) {
        body.push("\n".into());
    }
    body.extend(entries);
    body.extend(blanks);
}

fn ensure_blank_line_before(body: &mut Vec<String>) {
    if !body.last().is_some_and(|line| line.trim().is_empty()) {
        if !body.last().is_none_or(|line| line.ends_with('\n')) {
            body.push("\n".into());
        }
        body.push("\n".into());
    }
}

/// `config = 3` before the first top-level key, or after the leading
/// comments when the file has no top-level key.
fn add_version_key(top: &mut Vec<String>) {
    let line = format!("{VERSION_KEY} = 3\n");
    let first_key = top.iter().position(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with('#')
    });
    if let Some(position) = first_key {
        top.insert(position, line);
        return;
    }
    let comments = top
        .iter()
        .take_while(|line| line.trim_start().starts_with('#'))
        .count();
    if let Some(last) = comments.checked_sub(1).map(|index| &mut top[index])
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
    while top.get(comments).is_some_and(|line| line.trim().is_empty()) {
        top.remove(comments);
    }
    let mut added = Vec::new();
    if comments > 0 {
        added.push("\n".to_string());
    }
    added.push(line);
    added.push("\n".to_string());
    top.splice(comments..comments, added);
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
