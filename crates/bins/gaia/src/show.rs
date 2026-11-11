//! `gaia show <image>.<output>`: what an output of the latest build holds,
//! and where it came from.

use crate::record::{self, Run};
use crate::{Stop, args, project};
use gaia_store::{Kind, Store};
use std::path::Path;

/// Files listed for a tree before the rest is counted.
const LISTED: usize = 50;

pub fn show(options: &args::Show) -> Result<(), Stop> {
    let store = Store::open(project::store_dir(options.store.as_deref())?).map_err(Stop::usage)?;
    let latest = record::all(&store)
        .pop()
        .ok_or_else(|| Stop::usage("no build recorded in this store"))?;
    let run = Run::read(&latest)?;
    let Some(output) = run.outputs.get(&options.output) else {
        let names: Vec<&str> = run.outputs.keys().map(String::as_str).collect();
        return Err(Stop::usage(format!(
            "the latest build ({}) has no `{}`; it has: {}",
            run.outcome,
            options.output,
            names.join(", ")
        )));
    };
    let path = store.path(&output.object);
    println!("{}  {}", options.output, output.ty);
    println!("object  {}", output.object);
    println!("path    {}", path.display());
    println!("build   {} ({})", latest.display(), run.product.display());
    match output.object.kind {
        Kind::File => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!("size    {size} bytes");
        }
        Kind::Tree => {
            let mut files = Vec::new();
            walk(&path, &path, &mut files);
            files.sort();
            let total: u64 = files.iter().map(|(_, size)| size).sum();
            println!("holds   {} files, {total} bytes", files.len());
            for (name, size) in files.iter().take(LISTED) {
                println!("  {size:>10}  {name}");
            }
            if files.len() > LISTED {
                println!("  ... {} more", files.len() - LISTED);
            }
        }
    }
    if let Some(step) = run
        .steps
        .iter()
        .find(|s| s.outputs.values().any(|o| *o == output.object))
    {
        let how = if step.made {
            "made"
        } else {
            "found in the store"
        };
        println!("step    {} {how}, key {}", step.step, step.key);
    }
    println!("from:");
    for origin in &output.from {
        println!(
            "  {:24} {:22} {}",
            origin.label,
            origin.id,
            origin.file.display()
        );
    }
    Ok(())
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, u64)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            walk(root, &path, files);
        } else {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            files.push((name, meta.len()));
        }
    }
}
