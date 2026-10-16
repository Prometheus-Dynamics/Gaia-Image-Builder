use crate::env::ResolvedEnvironment;
use crate::raw::RawBuildConfig;
use crate::raw_assembly::{RawAssemblyArchiveConfig, RawAssemblyJsonValue};

use super::resolver;

pub(super) fn interpolate_assembly_archive(
    mut archive: RawAssemblyArchiveConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawAssemblyArchiveConfig {
    archive.id = resolver::interpolate_string(archive.id, raw, env);
    archive.output = resolver::interpolate_string(archive.output, raw, env);
    for member in &mut archive.members {
        member.name = resolver::interpolate_string(std::mem::take(&mut member.name), raw, env);
        member.src = member
            .src
            .take()
            .map(|value| resolver::interpolate_string(value, raw, env));
        member.tree = member
            .tree
            .take()
            .map(|value| resolver::interpolate_string(value, raw, env));
        member.src_dir = member
            .src_dir
            .take()
            .map(|value| resolver::interpolate_string(value, raw, env));
        member.src_glob = member
            .src_glob
            .take()
            .map(|value| resolver::interpolate_string(value, raw, env));
        member.prefix = resolver::interpolate_string(std::mem::take(&mut member.prefix), raw, env);
        if let Some(entries) = member.entries.take() {
            member.entries = Some(interpolate_entries(entries, raw, env));
        }
        member.json = member
            .json
            .take()
            .map(|value| interpolate_json(value, raw, env));
    }
    for generated in &mut archive.generated {
        generated.name =
            resolver::interpolate_string(std::mem::take(&mut generated.name), raw, env);
        generated.entries = generated
            .entries
            .take()
            .map(|entries| interpolate_entries(entries, raw, env));
        generated.json = generated
            .json
            .take()
            .map(|value| interpolate_json(value, raw, env));
    }
    archive
}

/// Interpolates file content like an entry value: config tokens resolve,
/// `${assembly...}` tokens are kept for assembly time.
pub(super) fn interpolate_content(
    value: &str,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> String {
    interpolate_entry_value(value, raw, env)
}

/// Interpolates every string of a JSON value (keys too), keeping
/// `${assembly...}` tokens for assembly time.
pub(super) fn interpolate_json(
    value: RawAssemblyJsonValue,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawAssemblyJsonValue {
    match value {
        RawAssemblyJsonValue::String(text) => {
            RawAssemblyJsonValue::String(interpolate_entry_value(&text, raw, env))
        }
        RawAssemblyJsonValue::Integer(_) | RawAssemblyJsonValue::Bool(_) => value,
        RawAssemblyJsonValue::Array(items) => RawAssemblyJsonValue::Array(
            items
                .into_iter()
                .map(|item| interpolate_json(item, raw, env))
                .collect(),
        ),
        RawAssemblyJsonValue::Object(members) => RawAssemblyJsonValue::Object(
            members
                .into_iter()
                .map(|(key, item)| {
                    (
                        resolver::interpolate_string(key, raw, env),
                        interpolate_json(item, raw, env),
                    )
                })
                .collect(),
        ),
    }
}

fn interpolate_entries(
    entries: Vec<(String, String)>,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> Vec<(String, String)> {
    entries
        .into_iter()
        .map(|(key, value)| {
            (
                resolver::interpolate_string(key, raw, env),
                interpolate_entry_value(&value, raw, env),
            )
        })
        .collect()
}

/// Interpolates an entry value like any other string, but keeps
/// `${assembly...}` tokens for assembly time; config tokens nested inside
/// them (`${assembly.sha256:$assembly.out/${build.name}.img}`) still resolve.
fn interpolate_entry_value(value: &str, raw: &RawBuildConfig, env: &ResolvedEnvironment) -> String {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${assembly.") {
        output.push_str(&resolver::interpolate_string(
            rest[..start].to_string(),
            raw,
            env,
        ));
        let Some(end) = matching_brace(&rest[start + 2..]) else {
            output.push_str(&rest[start..]);
            return output;
        };
        let inner = &rest[start + 2..start + 2 + end];
        output.push_str("${");
        output.push_str(&resolver::interpolate_string(inner.to_string(), raw, env));
        output.push('}');
        rest = &rest[start + 2 + end + 1..];
    }
    output.push_str(&resolver::interpolate_string(rest.to_string(), raw, env));
    output
}

/// Index of the `}` closing a token body, skipping nested `${...}`.
fn matching_brace(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut depth = 1usize;
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
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::matching_brace;

    #[test]
    fn matching_brace_skips_nested_tokens() {
        assert_eq!(matching_brace("a}"), Some(1));
        assert_eq!(matching_brace("a:${b}c}d"), Some(7));
        assert_eq!(matching_brace("a:${b}"), None);
    }
}
