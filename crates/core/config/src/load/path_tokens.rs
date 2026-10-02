//! `@self` and `@source:<id>` path tokens in config string values.
//!
//! Rewritten right after a file is parsed, before it is merged with other
//! layers, so the token always refers to the file that contains it:
//! - `@self` / `@self/<rest>` becomes the absolute directory of that file.
//! - `@source:<id>` / `@source:<id>/<rest>` becomes the checkout directory of
//!   import source `<id>`.

use std::path::Path;

use crate::ConfigError;

const SELF_TOKEN: &str = "@self";
const SOURCE_TOKEN: &str = "@source:";

/// Resolves an `@source:` id to its checkout directory. `Ok(None)` leaves the
/// value untouched (used by the declaration pre-pass, which runs before any
/// source can be resolved).
pub(super) type SourceRootResolver<'a> =
    dyn FnMut(&str) -> Result<Option<std::path::PathBuf>, ConfigError> + 'a;

pub(super) fn rewrite_path_tokens(
    value: &mut toml::Value,
    self_dir: &Path,
    resolve_source: &mut SourceRootResolver<'_>,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => {
            if let Some(rewritten) = rewrite_string(text, self_dir, resolve_source)? {
                *text = rewritten;
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                rewrite_path_tokens(item, self_dir, resolve_source)?;
            }
        }
        toml::Value::Table(table) => {
            for (_, item) in table.iter_mut() {
                rewrite_path_tokens(item, self_dir, resolve_source)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Rewrites a value, treating it as a `:`-separated path list (as used by
/// `external_tree`) so a token may start any entry, not only the first.
fn rewrite_string(
    text: &str,
    self_dir: &Path,
    resolve_source: &mut SourceRootResolver<'_>,
) -> Result<Option<String>, ConfigError> {
    if !text.contains(':') {
        return rewrite_entry(text, self_dir, resolve_source);
    }
    let mut entries = Vec::<String>::new();
    for part in text.split(':') {
        // `@source:<id>/...` contains the separator itself; rejoin it.
        match entries.last_mut() {
            Some(previous) if previous == "@source" => {
                previous.push(':');
                previous.push_str(part);
            }
            _ => entries.push(part.to_string()),
        }
    }
    let mut changed = false;
    let mut rewritten = Vec::with_capacity(entries.len());
    for entry in &entries {
        match rewrite_entry(entry, self_dir, resolve_source)? {
            Some(value) => {
                changed = true;
                rewritten.push(value);
            }
            None => rewritten.push(entry.clone()),
        }
    }
    Ok(changed.then(|| rewritten.join(":")))
}

fn rewrite_entry(
    text: &str,
    self_dir: &Path,
    resolve_source: &mut SourceRootResolver<'_>,
) -> Result<Option<String>, ConfigError> {
    if let Some(rest) = token_rest(text, SELF_TOKEN) {
        return Ok(Some(join(self_dir, rest)));
    }
    let Some(reference) = text.strip_prefix(SOURCE_TOKEN) else {
        return Ok(None);
    };
    let (id, rest) = match reference.split_once('/') {
        Some((id, rest)) => (id, rest),
        None => (reference, ""),
    };
    if id.is_empty() {
        return Ok(None);
    }
    Ok(resolve_source(id)?.map(|root| join(&root, rest)))
}

/// `Some(rest)` for `token` exactly (empty rest) or `token/rest`.
fn token_rest<'a>(text: &'a str, token: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(token)?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix('/')
    }
}

fn join(base: &Path, rest: &str) -> String {
    if rest.is_empty() {
        base.display().to_string()
    } else {
        base.join(rest).display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rewrite(contents: &str) -> toml::Value {
        let mut value: toml::Value = toml::from_str(contents).expect("toml");
        rewrite_path_tokens(&mut value, Path::new("/layers/raze"), &mut |id| {
            Ok((id == "atlas").then(|| PathBuf::from("/cache/atlas-abc")))
        })
        .expect("rewrite");
        value
    }

    #[test]
    fn rewrites_self_and_source_tokens_only_at_the_start() {
        let value = rewrite(
            r#"
a = "@self/overlay"
b = "@self"
c = "@source:atlas/devices/raze"
d = "@source:other/x"
e = "prefix @self/x"
f = "@selfish"
nested = { list = ["@self/units/a.service"] }
"#,
        );
        assert_eq!(value["a"].as_str(), Some("/layers/raze/overlay"));
        assert_eq!(value["b"].as_str(), Some("/layers/raze"));
        assert_eq!(value["c"].as_str(), Some("/cache/atlas-abc/devices/raze"));
        assert_eq!(value["d"].as_str(), Some("@source:other/x"));
        assert_eq!(value["e"].as_str(), Some("prefix @self/x"));
        assert_eq!(value["f"].as_str(), Some("@selfish"));
        assert_eq!(
            value["nested"]["list"][0].as_str(),
            Some("/layers/raze/units/a.service")
        );
    }

    #[test]
    fn rewrites_tokens_in_every_entry_of_a_colon_separated_list() {
        let value = rewrite(
            r#"
first = "@source:atlas/devices/raze/gaia/buildroot-external:raze/assets/buildroot"
later = "raze/assets/buildroot:@source:atlas/ext:@self/ext"
url = "https://example.com:8080/x"
untouched = "@source:other/a:b"
"#,
        );
        assert_eq!(
            value["first"].as_str(),
            Some("/cache/atlas-abc/devices/raze/gaia/buildroot-external:raze/assets/buildroot")
        );
        assert_eq!(
            value["later"].as_str(),
            Some("raze/assets/buildroot:/cache/atlas-abc/ext:/layers/raze/ext")
        );
        assert_eq!(value["url"].as_str(), Some("https://example.com:8080/x"));
        assert_eq!(value["untouched"].as_str(), Some("@source:other/a:b"));
    }
}
