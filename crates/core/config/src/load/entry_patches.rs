//! Patch entries: `[[artifacts]]`, `[[install]]`, `[[stage.files]]` and
//! `[[stage.services]]` entries without the key that marks a full
//! declaration. They are taken out of the file's values before the typed
//! read, and the merge applies them (see `merge::entries`).

use std::path::Path;

use crate::ConfigError;
use crate::locate::{KeyLocations, line_col};
use crate::raw::{PatchTable, RawEntryPatch};

/// Removes the patch entries from `value` (the file after its path tokens
/// are rewritten) and returns them in file order. `text` is the file's
/// text, for the lines the patches are reported at, and `layer` its label.
pub(super) fn take_entry_patches(
    value: &mut toml::Value,
    text: &str,
    layer: &str,
) -> Result<Vec<RawEntryPatch>, ConfigError> {
    let locations = KeyLocations::scan(text);
    let mut patches = Vec::new();
    for table in [
        PatchTable::Artifact,
        PatchTable::Install,
        PatchTable::StageFile,
        PatchTable::StageService,
    ] {
        let Some(entries) = entries_mut(value, table) else {
            continue;
        };
        let mut kept = Vec::with_capacity(entries.len());
        for (index, entry) in std::mem::take(entries).into_iter().enumerate() {
            let patch_fields = match entry.as_table() {
                Some(fields)
                    if fields.get("id").and_then(toml::Value::as_str).is_some()
                        && !table.declares(fields) =>
                {
                    fields.clone()
                }
                _ => {
                    kept.push(entry);
                    continue;
                }
            };
            let id = patch_fields
                .get("id")
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut fields = patch_fields;
            fields.remove("id");
            let location = match locations.offset(&format!("{}[{index}]", table.key_path())) {
                Some(offset) => format!("{layer}:{}", line_col(text, offset).0),
                None => layer.to_string(),
            };
            if fields.is_empty() {
                return Err(ConfigError::ConfigPatch {
                    message: format!(
                        "{} patch '{id}' in {location} sets no field to change; a patch \
                         needs at least one key besides id",
                        table.name()
                    ),
                });
            }
            patches.push(RawEntryPatch {
                table,
                id,
                fields,
                location,
            });
        }
        *entries = kept;
    }
    Ok(patches)
}

/// The entries array of `table` in the file, when the file has one.
fn entries_mut(value: &mut toml::Value, table: PatchTable) -> Option<&mut Vec<toml::Value>> {
    let root = value.as_table_mut()?;
    match table {
        PatchTable::Artifact => root.get_mut("artifacts")?.as_array_mut(),
        PatchTable::Install => root.get_mut("install")?.as_array_mut(),
        PatchTable::StageFile => root
            .get_mut("stage")?
            .as_table_mut()?
            .get_mut("files")?
            .as_array_mut(),
        PatchTable::StageService => root
            .get_mut("stage")?
            .as_table_mut()?
            .get_mut("services")?
            .as_array_mut(),
    }
}

/// A layer's name in notes and errors: its path relative to the entrypoint's
/// directory when it is inside it, else the path as it is.
pub(super) fn layer_label(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}
