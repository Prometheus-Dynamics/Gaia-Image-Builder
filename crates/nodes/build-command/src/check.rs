//! Checking the declared paths, and what the command left in `/out`.

use crate::CommandOutput;
use std::collections::BTreeSet;
use std::path::{Component, Path};

/// Each output path, relative and plain; no two the same or one inside
/// another.
pub(crate) fn outputs(outputs: &[CommandOutput]) -> Result<Vec<String>, String> {
    if outputs.is_empty() {
        return Err("a command declares at least one output".into());
    }
    let mut paths: Vec<String> = Vec::new();
    for o in outputs {
        let path = o.path.trim_end_matches('/');
        let plain = !path.is_empty()
            && Path::new(path)
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
        if !plain {
            return Err(format!("output {:?} is not a path under /out", o.path));
        }
        if let Some(other) = paths.iter().find(|p| {
            *p == path || path.starts_with(&format!("{p}/")) || p.starts_with(&format!("{path}/"))
        }) {
            return Err(format!("outputs {other} and {path} overlap"));
        }
        paths.push(path.to_string());
    }
    Ok(paths)
}

/// An input's mount point: absolute, and not one the sandbox owns.
pub(crate) fn mount_point(at: &str) -> Result<(), String> {
    let top = at.trim_start_matches('/').split('/').next().unwrap_or("");
    let plain = at.starts_with('/')
        && Path::new(at)
            .components()
            .skip(1)
            .all(|c| matches!(c, Component::Normal(_)));
    if !plain || matches!(top, "" | "out" | "proc" | "dev" | "tmp") {
        return Err(format!("{at} is not a place an input can be bound"));
    }
    Ok(())
}

/// Every declared output is there, as declared, and nothing else is.
pub(crate) fn produced(
    out: &Path,
    outputs: &[CommandOutput],
    declared: &[String],
) -> Result<(), String> {
    for (o, path) in outputs.iter().zip(declared) {
        let meta = std::fs::symlink_metadata(out.join(path))
            .map_err(|_| format!("declared output /out/{path} was not made"))?;
        let want_dir = o.kind == "tree";
        if meta.is_dir() != want_dir || meta.file_type().is_symlink() {
            let what = if want_dir { "a directory" } else { "a file" };
            return Err(format!("declared output /out/{path} is not {what}"));
        }
    }
    // Parents of declared outputs may exist; nothing else may.
    let mut allowed: BTreeSet<String> = BTreeSet::new();
    for path in declared {
        let mut parent = Path::new(path).parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            allowed.insert(p.to_string_lossy().into_owned());
            parent = p.parent();
        }
    }
    let mut extra = Vec::new();
    walk(out, "", declared, &allowed, &mut extra).map_err(|e| format!("/out: {e}"))?;
    if extra.is_empty() {
        return Ok(());
    }
    let shown: Vec<String> = extra.iter().take(8).map(|p| format!("/out/{p}")).collect();
    let more = if extra.len() > 8 {
        format!(" and {} more", extra.len() - 8)
    } else {
        String::new()
    };
    Err(format!("undeclared output {}{more}", shown.join(", ")))
}

fn walk(
    dir: &Path,
    prefix: &str,
    declared: &[String],
    allowed: &BTreeSet<String>,
    extra: &mut Vec<String>,
) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if declared.contains(&path) {
            continue;
        }
        if allowed.contains(&path) {
            walk(&entry.path(), &path, declared, allowed, extra)?;
        } else {
            extra.push(path);
        }
    }
    Ok(())
}
