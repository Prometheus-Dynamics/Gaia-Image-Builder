//! `gaia layer delta`: what a board that runs an earlier release needs to
//! update to a new set of `[image.layers]` layers.
//!
//! Each layer of the new `layers.json` is compared by its tar's sha256 with
//! the layer of the same name in every earlier release given. A layer that
//! matches one is unchanged and needs nothing. A changed or new layer gets its
//! tar compressed whole (`<name>.tar.zst`, for a board that has none of the
//! earlier releases) and, per earlier release that has the layer, a
//! `zstd --patch-from` delta against that release's tar
//! (`<name>.from-<sha12>.tar.zst`), kept only when it is smaller than the
//! whole layer. Deltas are taken on the uncompressed tar because deltas of
//! compressed erofs images are poor; the board applies the delta to its copy
//! of the earlier tar and makes the erofs from the result.
//!
//! Every file written is decompressed again and checked against the new
//! tar's sha256 before it is recorded. `deltas.json` lists them, and a copy
//! of the new `layers.json` sits beside it.

use gaia_process::{format_bytes, sha256_file};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::CommandOutcome;

/// The description of the layers, beside their tars.
const LAYERS_JSON: &str = "layers.json";
/// The description of the deltas, in the output directory.
const DELTAS_JSON: &str = "deltas.json";
/// Version of `deltas.json`.
const DELTAS_SCHEMA: u64 = 1;
/// The zstd level of every file written: deltas are made once per release
/// and fetched by every board, so size matters more than time.
const ZSTD_LEVEL: &str = "-19";

/// One release's layers: its `layers.json` and the directory of its tars.
struct Release {
    dir: PathBuf,
    layers: Vec<Value>,
}

impl Release {
    /// The release in `path`: a layers directory, or its `layers.json`.
    fn read(path: &str) -> Result<Self, String> {
        let path = Path::new(path);
        let (dir, json) = if path.is_dir() {
            (path.to_path_buf(), path.join(LAYERS_JSON))
        } else {
            let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            (dir, path.to_path_buf())
        };
        let text = fs::read_to_string(&json)
            .map_err(|error| format!("failed to read '{}': {error}", json.display()))?;
        let document: Value = serde_json::from_str(&text)
            .map_err(|error| format!("'{}' is not valid JSON: {error}", json.display()))?;
        let layers = document["layers"]
            .as_array()
            .filter(|layers| layers.iter().all(|layer| layer["name"].is_string()))
            .ok_or_else(|| format!("'{}' has no layers list", json.display()))?
            .clone();
        Ok(Self { dir, layers })
    }

    fn layer(&self, name: &str) -> Option<&Value> {
        self.layers.iter().find(|layer| layer["name"] == name)
    }

    /// The layer's tar, when the release still has it and it is the tar
    /// `layers.json` describes.
    fn tar(&self, layer: &Value) -> Result<PathBuf, String> {
        let file = layer["tar"]
            .as_str()
            .ok_or_else(|| format!("layer '{}' records no tar", layer["name"]))?;
        let path = self.dir.join(file);
        let want = layer["tar_sha256"].as_str().unwrap_or_default();
        let have = sha256_file(&path)
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        if have != want {
            return Err(format!(
                "'{}' has sha256 {have}, but layers.json records {want}",
                path.display()
            ));
        }
        Ok(path)
    }
}

/// `gaia layer delta <layers> --from <old-layers> [--from ...] --out <dir>`.
pub(crate) fn layer_delta_command(
    new: &str,
    from: &[String],
    out: Option<&str>,
    json: bool,
) -> CommandOutcome {
    let Some(out) = out else {
        return CommandOutcome::Failed {
            message: "gaia layer delta needs --out <dir>".into(),
        };
    };
    match write_deltas(new, from, Path::new(out)) {
        Ok(document) if json => CommandOutcome::Json { value: document },
        Ok(document) => CommandOutcome::Text {
            text: summary(&document, out),
        },
        Err(message) => CommandOutcome::Failed { message },
    }
}

/// Writes the deltas of the layers in `new` against each release in `from`
/// to `out`, and returns `deltas.json`.
fn write_deltas(new: &str, from: &[String], out: &Path) -> Result<Value, String> {
    let new = Release::read(new)?;
    let old = from
        .iter()
        .map(|path| Release::read(path))
        .collect::<Result<Vec<_>, _>>()?;
    fs::create_dir_all(out)
        .map_err(|error| format!("failed to create '{}': {error}", out.display()))?;
    let mut records = Vec::new();
    for layer in &new.layers {
        records.push(layer_deltas(layer, &new, &old, out)?);
    }
    let document = json!({ "schema": DELTAS_SCHEMA, "layers": records });
    fs::copy(new.dir.join(LAYERS_JSON), out.join(LAYERS_JSON))
        .map_err(|error| format!("failed to copy layers.json to '{}': {error}", out.display()))?;
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("failed to encode deltas.json: {error}"))?;
    fs::write(out.join(DELTAS_JSON), format!("{text}\n"))
        .map_err(|error| format!("failed to write deltas.json: {error}"))?;
    Ok(document)
}

/// One layer's `deltas.json` record, writing its files to `out`.
fn layer_deltas(
    layer: &Value,
    new: &Release,
    old: &[Release],
    out: &Path,
) -> Result<Value, String> {
    let name = layer["name"].as_str().unwrap_or_default();
    let tar_sha256 = layer["tar_sha256"].as_str().unwrap_or_default();
    let mut record = json!({
        "name": name,
        "id": layer["id"],
        "tar_sha256": tar_sha256,
        "tar_size": layer["tar_size"],
    });
    let bases: Vec<(&Release, &Value)> = old
        .iter()
        .filter_map(|release| release.layer(name).map(|base| (release, base)))
        .collect();
    if bases
        .iter()
        .any(|(_, base)| base["tar_sha256"] == tar_sha256)
    {
        record["status"] = json!("unchanged");
        return Ok(record);
    }
    record["status"] = json!(if bases.is_empty() { "new" } else { "changed" });
    let tar = new.tar(layer)?;
    let window = window_log(&tar, &[]);
    let full = out.join(format!("{name}.tar.zst"));
    zstd(&tar, None, window, &full)?;
    let full_record = verified(&full, None, window, tar_sha256)?;
    let full_size = full_record["size"].as_u64().unwrap_or(u64::MAX);
    record["full"] = full_record;
    let mut deltas = Vec::new();
    for (release, base) in bases {
        let base_sha256 = base["tar_sha256"].as_str().unwrap_or_default();
        // A release without its tar (or with a different one) cannot be a
        // base: the board's copy is what layers.json describes.
        let Ok(base_tar) = release.tar(base) else {
            continue;
        };
        let file = out.join(format!("{name}.from-{}.tar.zst", short(base_sha256)));
        let window = window_log(&tar, &[&base_tar]);
        zstd(&tar, Some(&base_tar), window, &file)?;
        let mut delta = verified(&file, Some(&base_tar), window, tar_sha256)?;
        if delta["size"].as_u64().unwrap_or(u64::MAX) >= full_size {
            let _ = fs::remove_file(&file);
            continue;
        }
        delta["from_tar_sha256"] = json!(base_sha256);
        delta["from_id"] = base["id"].clone();
        deltas.push(delta);
    }
    deltas.sort_by_key(|delta| delta["size"].as_u64());
    record["deltas"] = json!(deltas);
    Ok(record)
}

/// The zstd window (as a power of two) that covers `tar` and every base, so
/// a delta can refer back to any byte of its base. Decompressing needs the
/// same `--long`.
fn window_log(tar: &Path, bases: &[&Path]) -> u32 {
    let largest = std::iter::once(tar)
        .chain(bases.iter().copied())
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .max()
        .unwrap_or(0);
    let bits = u64::BITS - largest.max(1).leading_zeros();
    bits.clamp(10, 31)
}

/// Compresses `tar` to `out`, as a delta against `base` when one is given.
fn zstd(tar: &Path, base: Option<&Path>, window: u32, out: &Path) -> Result<(), String> {
    let mut command = Command::new("zstd");
    command
        .args(["-q", "-f", ZSTD_LEVEL, "-T0"])
        .arg(format!("--long={window}"));
    if let Some(base) = base {
        command.arg(format!("--patch-from={}", base.display()));
    }
    command.arg(tar).arg("-o").arg(out);
    run(command, &format!("compress '{}'", tar.display()))
}

/// The record of the compressed file `file`, after checking that it
/// decompresses (against `base`, for a delta) to the tar with `tar_sha256`.
fn verified(
    file: &Path,
    base: Option<&Path>,
    window: u32,
    tar_sha256: &str,
) -> Result<Value, String> {
    let check = file.with_extension("check");
    let mut command = Command::new("zstd");
    command
        .args(["-q", "-d", "-f"])
        .arg(format!("--long={window}"));
    if let Some(base) = base {
        command.arg(format!("--patch-from={}", base.display()));
    }
    command.arg(file).arg("-o").arg(&check);
    let result = run(command, &format!("decompress '{}'", file.display()))
        .and_then(|()| sha256_file(&check).map_err(|error| error.to_string()));
    let _ = fs::remove_file(&check);
    let have = result?;
    if have != tar_sha256 {
        let _ = fs::remove_file(file);
        return Err(format!(
            "'{}' decompresses to sha256 {have}, not the layer's {tar_sha256}",
            file.display()
        ));
    }
    let size = fs::metadata(file)
        .map_err(|error| format!("failed to read '{}': {error}", file.display()))?
        .len();
    let sha256 = sha256_file(file).map_err(|error| error.to_string())?;
    Ok(json!({
        "file": file.file_name().map(|name| name.to_string_lossy().into_owned()),
        "sha256": sha256,
        "size": size,
        "window_log": window,
    }))
}

fn run(mut command: Command, what: &str) -> Result<(), String> {
    let output = command.output().map_err(|error| {
        format!("gaia layer delta needs zstd (1.4.7 or later) on PATH: {error}")
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "zstd failed to {what}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// The first 12 hex digits of a sha256, enough to tell releases apart in a
/// file name.
fn short(sha256: &str) -> &str {
    &sha256[..sha256.len().min(12)]
}

/// One line per layer: what a board needs, at best and at worst.
fn summary(document: &Value, out: &str) -> String {
    let mut lines = Vec::new();
    for layer in document["layers"].as_array().into_iter().flatten() {
        let name = layer["name"].as_str().unwrap_or_default();
        let tar = format_bytes(layer["tar_size"].as_u64().unwrap_or(0));
        let line = match layer["status"].as_str() {
            Some("unchanged") => format!("layer {name}: unchanged"),
            status => {
                let full = format_bytes(layer["full"]["size"].as_u64().unwrap_or(0));
                let deltas = layer["deltas"].as_array().map_or(&[][..], Vec::as_slice);
                match deltas.first() {
                    Some(best) => format!(
                        "layer {name}: {} delta(s), smallest {} from {} (whole layer {full}, tar {tar})",
                        deltas.len(),
                        format_bytes(best["size"].as_u64().unwrap_or(0)),
                        short(best["from_tar_sha256"].as_str().unwrap_or_default()),
                    ),
                    None => format!(
                        "layer {name}: {}, whole layer only: {full} (tar {tar})",
                        status.unwrap_or("changed")
                    ),
                }
            }
        };
        lines.push(line);
    }
    lines.push(format!(
        "described by '{}'",
        Path::new(out).join(DELTAS_JSON).display()
    ));
    lines.join("\n")
}

#[cfg(test)]
#[path = "layer_delta_tests.rs"]
mod tests;
