//! Reading one file: its version, then its keys, with errors that name the
//! file, line and key, and suggest the key meant for an unknown one.

use crate::Error;
use serde::de::DeserializeOwned;
use std::path::Path;

/// The version every description format is at.
pub const VERSION: i64 = 1;

/// Reads the file at `path` as `T`.
pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::new(path, "", format!("cannot read: {error}")))?;
    parse(path, &text)
}

/// Parses `text`, the text of the file at `path`, as `T`.
pub(crate) fn parse<T: DeserializeOwned>(path: &Path, text: &str) -> Result<T, Error> {
    let line_of = |offset: usize| text[..offset.min(text.len())].matches('\n').count() + 1;
    let table: toml::Table = toml::from_str(text).map_err(|error| {
        Error::new(path, "", error.message().to_string())
            .at_line(error.span().map(|span| line_of(span.start)))
    })?;
    match table.get("version") {
        None => {
            return Err(Error::new(
                path,
                "version",
                format!("missing; write `version = {VERSION}`"),
            ));
        }
        Some(toml::Value::Integer(VERSION)) => {}
        Some(other) => {
            return Err(Error::new(
                path,
                "version",
                format!("{other} is not a version of this format; it is at {VERSION}"),
            )
            .at_line(line_of_key(text, "version")));
        }
    }
    let deserializer = toml::de::Deserializer::parse(text).map_err(|error| {
        Error::new(path, "", error.message().to_string())
            .at_line(error.span().map(|span| line_of(span.start)))
    })?;
    serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let key = key_path(&error.path().to_string());
        let inner = error.inner();
        let line = inner.span().map(|span| line_of(span.start));
        let message = inner.message().to_string();
        match unknown_field(&message) {
            Some((field, expected)) => {
                let key = if key.is_empty() {
                    field.clone()
                } else if key.ends_with(&field) {
                    key
                } else {
                    format!("{key}.{field}")
                };
                let mut message = "unknown key".to_string();
                if let Some(near) = closest(&field, &expected) {
                    let full = match key.rsplit_once('.') {
                        Some((parent, _)) => format!("{parent}.{near}"),
                        None => near.to_string(),
                    };
                    message.push_str(&format!("; did you mean `{full}`?"));
                } else if !expected.is_empty() {
                    message.push_str(&format!("; the keys here are {}", expected.join(", ")));
                }
                Error::new(path, key, message).at_line(line)
            }
            None => Error::new(path, key, message).at_line(line),
        }
    })
}

/// `serde_path_to_error`'s path, with `?` for an unknown position dropped.
fn key_path(path: &str) -> String {
    if path == "." {
        String::new()
    } else {
        path.replace(".?", "")
    }
}

/// The line `key = ...` is on, at the top of the file.
fn line_of_key(text: &str, key: &str) -> Option<usize> {
    text.lines()
        .position(|line| {
            line.trim_start()
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
        .map(|index| index + 1)
}

/// The field and the expected ones of serde's "unknown field `x`, expected
/// one of `a`, `b`" (or "expected `a`", or "there are no fields").
fn unknown_field(message: &str) -> Option<(String, Vec<String>)> {
    let rest = message.strip_prefix("unknown field `")?;
    let (field, rest) = rest.split_once('`')?;
    let expected = rest
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    Some((field.to_string(), expected))
}

/// The known name nearest `name`, when it is near enough to be a typo.
pub(crate) fn closest<'a>(name: &str, known: &'a [String]) -> Option<&'a str> {
    known
        .iter()
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, candidate)| *distance <= (candidate.len() / 3).max(2))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate.as_str())
}

/// Edit distance, by characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(here + 1)
                .min(previous + usize::from(ca != *cb));
            previous = here;
        }
    }
    row[b.len()]
}
