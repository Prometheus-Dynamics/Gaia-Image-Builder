//! The lock's text form.

use crate::{Entry, Lock, LockError, VERSION};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;

const HEADER: &str = "# Written by `gaia lock`. Do not edit.\n";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    #[serde(default)]
    source: BTreeMap<String, Raw>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "lowercase")]
enum Raw {
    Git {
        url: String,
        #[serde(rename = "ref")]
        reference: String,
        commit: String,
    },
    Download {
        url: String,
        digest: String,
    },
    Image {
        reference: String,
        digest: String,
    },
    Tree {
        path: String,
        digest: String,
    },
}

pub(crate) fn parse(text: &str) -> Result<Lock, LockError> {
    let file: File = toml::from_str(text).map_err(|error| LockError::Parse(error.to_string()))?;
    if file.version != VERSION {
        return Err(LockError::Parse(format!(
            "lock version {} (this Gaia reads {VERSION})",
            file.version
        )));
    }
    let mut lock = Lock::default();
    for (name, raw) in file.source {
        let entry = match raw {
            Raw::Git {
                url,
                reference,
                commit,
            } => Entry::Git {
                url,
                reference,
                commit,
            },
            Raw::Download { url, digest } => Entry::Download { url, digest },
            Raw::Image { reference, digest } => Entry::Image { reference, digest },
            Raw::Tree { path, digest } => Entry::Tree { path, digest },
        };
        check_resolved(&name, &entry)?;
        lock.entries.insert(name, entry);
    }
    Ok(lock)
}

fn check_resolved(name: &str, entry: &Entry) -> Result<(), LockError> {
    let resolved = entry.resolved();
    let good = match entry {
        Entry::Git { .. } => {
            matches!(resolved.len(), 40 | 64)
                && resolved
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        }
        _ => resolved.starts_with("sha256:") && gaia_store::Digest::parse(resolved).is_some(),
    };
    if good {
        Ok(())
    } else {
        Err(LockError::Parse(format!(
            "source {name}: `{resolved}` is not a {}",
            if entry.kind() == "git" {
                "commit (40 or 64 lowercase hex digits)"
            } else {
                "digest (sha256:<64 lowercase hex digits>)"
            }
        )))
    }
}

pub(crate) fn render(lock: &Lock) -> String {
    let mut out = String::from(HEADER);
    let _ = writeln!(out, "version = {VERSION}");
    for (name, entry) in &lock.entries {
        let _ = writeln!(out, "\n[source.{}]", quoted(name));
        let _ = writeln!(out, "kind = {}", quoted(entry.kind()));
        let fields: [(&str, &str); 3] = match entry {
            Entry::Git {
                url,
                reference,
                commit,
            } => [("url", url), ("ref", reference), ("commit", commit)],
            Entry::Download { url, digest } => [("url", url), ("digest", digest), ("", "")],
            Entry::Image { reference, digest } => {
                [("reference", reference), ("digest", digest), ("", "")]
            }
            Entry::Tree { path, digest } => [("path", path), ("digest", digest), ("", "")],
        };
        for (key, value) in fields.iter().filter(|(key, _)| !key.is_empty()) {
            let _ = writeln!(out, "{key} = {}", quoted(value));
        }
    }
    out
}

/// A TOML basic string.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
