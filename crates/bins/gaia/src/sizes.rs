//! `gaia sizes [<run.json>]`: what one build made, in bytes.
//!
//! A size manifest read from a run's record and the store, not typed by
//! hand: each output the build gave, then what its steps made that a size
//! is asked of (the disk, the root filesystem, the slot, the initramfs, the
//! boot partition and what its writer said of the fit, the kernel raw and
//! gzipped, the modules, the bootloader's files), and for each merged tree
//! the bytes each component put in it. The same command on another run
//! gives the other column of a comparison.
//!
//! A row is `what  bytes  detail`. `--json` gives the rows as a list.
//! `--xz` also compresses each disk image (xz, preset 6), which takes as
//! long as it takes.

use crate::record::{self, Run};
use crate::{Stop, args, project};
use gaia_store::{Kind, ObjectRef, Store};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

/// The components shown for a merged tree.
const TOP: usize = 10;

#[derive(Debug, Serialize)]
struct Row {
    what: String,
    bytes: u64,
    detail: String,
}

/// The files under `dir`, as paths from it and their sizes. A symlink is
/// its own length, not its target's.
fn files(dir: &Path) -> Vec<(String, u64)> {
    fn walk(root: &Path, dir: &Path, into: &mut Vec<(String, u64)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                walk(root, &path, into);
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                into.push((name, meta.len()));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found
}

fn size(store: &Store, object: &ObjectRef) -> (u64, usize) {
    let path = store.path(object);
    match object.kind {
        Kind::File => (std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), 1),
        Kind::Tree => {
            let found = files(&path);
            (found.iter().map(|(_, bytes)| bytes).sum(), found.len())
        }
    }
}

/// `bytes` gzipped at the highest level: the size a boot firmware loads.
fn gzipped(path: &Path) -> std::io::Result<u64> {
    let mut encoder = flate2::write::GzEncoder::new(Counter(0), flate2::Compression::best());
    std::io::copy(&mut std::fs::File::open(path)?, &mut encoder)?;
    Ok(encoder.finish()?.0)
}

fn xzed(path: &Path) -> std::io::Result<u64> {
    let mut encoder = xz2::write::XzEncoder::new(Counter(0), 6);
    std::io::copy(&mut std::fs::File::open(path)?, &mut encoder)?;
    Ok(encoder.finish()?.0)
}

/// A writer that keeps only how much was written.
struct Counter(u64);

impl Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn is_module(name: &str) -> bool {
    [".ko", ".ko.xz", ".ko.gz", ".ko.zst"]
        .iter()
        .any(|end| name.ends_with(end))
}

/// One path of a merged tree's `owners.json`.
#[derive(serde::Deserialize)]
struct Owned {
    path: String,
    owners: Vec<String>,
}

/// The host tools of the newest build in the store that has erofs tools.
fn erofs_tools(store: &Store) -> Option<std::path::PathBuf> {
    record::all(store).into_iter().rev().find_map(|path| {
        let run = Run::read(&path).ok()?;
        run.steps
            .iter()
            .filter(|step| step.step == "gaia.os.buildroot.host")
            .filter_map(|step| step.outputs.get("seed"))
            .map(|seed| store.path(seed))
            .find(|seed| crate::rootfs_sizes::tools_dir(seed).is_some())
    })
}

fn print(title: &str, path: &Path, rows: &[Row], json: bool) -> Result<(), Stop> {
    if json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            title: path,
            "rows": rows,
        }))
        .map_err(Stop::usage)?;
        println!("{text}");
        return Ok(());
    }
    println!("{title:8} {}", path.display());
    let width = rows.iter().map(|r| r.what.len()).max().unwrap_or(0);
    for r in rows {
        println!("{:width$}  {:>12}  {}", r.what, r.bytes, r.detail);
    }
    Ok(())
}

pub fn sizes(options: &args::Sizes) -> Result<(), Stop> {
    if let Some(rootfs) = &options.rootfs {
        if options.run.is_some() || options.image.is_some() {
            return Err(Stop::usage(
                "sizes takes one of a run, --image and --rootfs",
            ));
        }
        let tools = match &options.tools {
            Some(tools) => tools.clone(),
            None => {
                let store = Store::open(project::store_dir(options.store.as_deref())?)
                    .map_err(Stop::usage)?;
                erofs_tools(&store).ok_or_else(|| {
                    Stop::usage(
                        "no build in this store has erofs tools in its host tools; give --tools DIR",
                    )
                })?
            }
        };
        let rows: Vec<Row> = crate::rootfs_sizes::measure(rootfs, &tools, &std::env::temp_dir())
            .map_err(Stop::usage)?
            .into_iter()
            .map(|(what, bytes, detail)| Row {
                what,
                bytes,
                detail,
            })
            .collect();
        return print("rootfs", rootfs, &rows, options.json);
    }
    if let Some(image) = &options.image {
        if options.run.is_some() {
            return Err(Stop::usage("sizes takes a run or --image, not both"));
        }
        let rows: Vec<Row> = crate::image_sizes::measure(image, &std::env::temp_dir())
            .map_err(|e| Stop::usage(format!("{}: {e}", image.display())))?
            .into_iter()
            .map(|(what, bytes, detail)| Row {
                what,
                bytes,
                detail,
            })
            .collect();
        if options.json {
            let text = serde_json::to_string_pretty(&serde_json::json!({
                "image": image,
                "rows": rows,
            }))
            .map_err(Stop::usage)?;
            println!("{text}");
            return Ok(());
        }
        println!("image    {}", image.display());
        let width = rows.iter().map(|r| r.what.len()).max().unwrap_or(0);
        for r in &rows {
            println!("{:width$}  {:>12}  {}", r.what, r.bytes, r.detail);
        }
        return Ok(());
    }
    let store = Store::open(project::store_dir(options.store.as_deref())?).map_err(Stop::usage)?;
    let record = match &options.run {
        Some(path) => path.clone(),
        None => record::all(&store)
            .pop()
            .ok_or_else(|| Stop::usage("no build recorded in this store"))?,
    };
    let run = Run::read(&record)?;
    let mut rows: Vec<Row> = Vec::new();
    let mut row = |what: String, bytes: u64, detail: String| {
        rows.push(Row {
            what,
            bytes,
            detail,
        });
    };
    let short = |object: &ObjectRef| object.digest.hex()[..12].to_string();

    for (name, output) in &run.outputs {
        let (bytes, count) = size(&store, &output.object);
        let detail = match output.object.kind {
            Kind::File => String::new(),
            Kind::Tree => format!("{count} files"),
        };
        row(format!("output {name}"), bytes, detail);
    }
    // A step looked up twice is one thing made once.
    let mut seen = std::collections::BTreeSet::new();
    for step in &run.steps {
        if !seen.insert((step.step.clone(), step.key.clone())) {
            continue;
        }
        let out = |name: &str| step.outputs.get(name);
        let file = |name: &str| out(name).map(|object| (size(&store, object).0, object));
        match step.step.as_str() {
            "gaia.disk" => {
                if let Some((bytes, object)) = file("image") {
                    row("disk raw".into(), bytes, short(object));
                    if options.xz {
                        let xz = xzed(&store.path(object)).map_err(Stop::usage)?;
                        row("disk xz".into(), xz, "xz preset 6".into());
                    }
                }
            }
            "gaia.fs.erofs" | "gaia.fs.ext4" | "gaia.fs.vfat" => {
                if let Some((bytes, object)) = file("image") {
                    let kind = step.step.trim_start_matches("gaia.fs.");
                    // An erofs says whether it was read back after it was
                    // made: a size beside that line is of a verified image.
                    let read_back = out("read-back")
                        .and_then(|record| std::fs::read_to_string(store.path(record)).ok())
                        .map(|text| text.lines().next().unwrap_or_default().to_string());
                    let detail = match (kind, read_back) {
                        (_, Some(line)) => format!("{}; {line}", short(object)),
                        ("erofs", None) => {
                            format!("{}; NOT read back after it was made", short(object))
                        }
                        (_, None) => short(object),
                    };
                    row(format!("filesystem {kind}"), bytes, detail);
                }
            }
            "gaia.verity" => {
                if let Some((bytes, object)) = file("image") {
                    row(
                        "slot (filesystem + verity tree)".into(),
                        bytes,
                        short(object),
                    );
                }
            }
            "gaia.initramfs" => {
                if let Some((bytes, object)) = file("archive") {
                    row("initramfs".into(), bytes, short(object));
                }
            }
            "gaia.generate.image" => {
                if let Some((bytes, object)) = file("image") {
                    row(
                        "generated image (boot partition)".into(),
                        bytes,
                        short(object),
                    );
                }
                if let Some(report) = out("report")
                    && let Ok(text) = std::fs::read_to_string(store.path(report))
                {
                    // `role stored_bytes [bay_bytes]`, as its writer printed it.
                    for line in text.lines() {
                        let mut words = line.split_whitespace();
                        let (Some(role), Some(stored)) = (words.next(), words.next()) else {
                            continue;
                        };
                        let Ok(stored) = stored.parse::<u64>() else {
                            continue;
                        };
                        let rest: Vec<&str> = words.collect();
                        let detail = match rest.as_slice() {
                            [bay] => format!("of {bay} in its bay"),
                            _ => rest.join(" "),
                        };
                        row(format!("payload {role}"), stored, detail);
                    }
                }
            }
            "gaia.os.buildroot" => {
                if let Some(kernel) = out("kernel") {
                    for (name, bytes) in files(&store.path(kernel)) {
                        row(format!("kernel {name}"), bytes, "raw".into());
                        let gz = gzipped(&store.path(kernel).join(&name)).map_err(Stop::usage)?;
                        row(format!("kernel {name}"), gz, "gzip -9".into());
                    }
                }
                if let Some(modules) = out("modules") {
                    let found = files(&store.path(modules));
                    let kos: Vec<&(String, u64)> =
                        found.iter().filter(|(name, _)| is_module(name)).collect();
                    if !found.is_empty() {
                        row(
                            "kernel modules".into(),
                            kos.iter().map(|(_, bytes)| bytes).sum(),
                            format!("{} modules", kos.len()),
                        );
                        row(
                            "kernel modules tree".into(),
                            found.iter().map(|(_, bytes)| bytes).sum(),
                            format!("{} files", found.len()),
                        );
                    }
                }
                if let Some(bootloader) = out("bootloader") {
                    for (name, bytes) in files(&store.path(bootloader)) {
                        row(format!("bootloader {name}"), bytes, String::new());
                    }
                }
            }
            "gaia.tree.merge" => {
                let (Some(tree), Some(owners)) = (out("tree"), out("owners")) else {
                    continue;
                };
                let root = store.path(tree);
                let (bytes, count) = size(&store, tree);
                let id = short(tree);
                row(format!("merged tree {id}"), bytes, format!("{count} files"));
                let owned: Vec<Owned> = std::fs::read(store.path(owners))
                    .ok()
                    .and_then(|text| serde_json::from_slice(&text).ok())
                    .unwrap_or_default();
                let mut by: BTreeMap<&str, (u64, usize)> = BTreeMap::new();
                for entry in &owned {
                    let Ok(meta) = std::fs::symlink_metadata(root.join(&entry.path)) else {
                        continue;
                    };
                    if meta.is_dir() {
                        continue;
                    }
                    // A path two components gave alike is the first one's.
                    let owner = entry.owners.first().map_or("(none)", String::as_str);
                    let at = by.entry(owner).or_default();
                    at.0 += meta.len();
                    at.1 += 1;
                }
                let mut ranked: Vec<(&str, (u64, usize))> = by.into_iter().collect();
                ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
                for (owner, (bytes, count)) in ranked.into_iter().take(TOP) {
                    row(
                        format!("  in {id}: {owner}"),
                        bytes,
                        format!("{count} files"),
                    );
                }
            }
            _ => {}
        }
    }

    if options.json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "run": record,
            "product": run.product,
            "outcome": run.outcome,
            "rows": rows,
        }))
        .map_err(Stop::usage)?;
        println!("{text}");
        return Ok(());
    }
    println!("run      {}", record.display());
    println!("product  {} ({})", run.product.display(), run.outcome);
    let width = rows.iter().map(|r| r.what.len()).max().unwrap_or(0);
    for r in &rows {
        println!("{:width$}  {:>12}  {}", r.what, r.bytes, r.detail);
    }
    Ok(())
}
