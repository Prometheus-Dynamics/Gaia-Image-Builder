//! Applying patch entries (see `RawEntryPatch`) to the id-keyed tables.
//!
//! A patch replaces the keys it sets in the entry with its id, field by
//! field: a table value such as `execution` replaces that whole table, and
//! a key the patch leaves out keeps the earlier value. The entry is read
//! back through its serialized form, so every key keeps the meaning the
//! entry's own declaration gives it.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ConfigError;
use crate::raw::{PatchTable, RawBuildConfig, RawEntryPatch};

/// Applies `patch` to the entry it names in `base`, or defers it: when `base`
/// has no entry with that id the patch waits in `base.pending_patches` for a
/// layer that does, and the merge reports it if none does.
pub(super) fn apply_or_defer(
    base: &mut RawBuildConfig,
    patch: RawEntryPatch,
) -> Result<(), ConfigError> {
    let applied = match patch.table {
        PatchTable::Artifact => patch_entry(&mut base.artifacts, |item| &item.id, &patch, &[]),
        PatchTable::Install => patch_entry(&mut base.install, |item| &item.id, &patch, &[]),
        // A patch giving `content` replaces the entry's `src`, and the reverse.
        PatchTable::StageFile => patch_entry(
            &mut base.stage.files,
            |item| &item.id,
            &patch,
            &["src", "content"],
        ),
        PatchTable::StageService => {
            patch_entry(&mut base.stage.services, |item| &item.id, &patch, &[])
        }
    }
    .map_err(|message| ConfigError::ConfigPatch {
        message: format!(
            "{} patch '{}' in {} does not make a valid {}: {message}",
            patch.table.name(),
            patch.id,
            patch.location,
            patch.table.name()
        ),
    })?;
    if applied {
        let keys = patch.fields.keys().cloned().collect::<Vec<_>>();
        base.config_notes.push(format!(
            "{} '{}': {} set by {}",
            patch.table.name(),
            patch.id,
            keys.join(", "),
            patch.location
        ));
    } else {
        base.pending_patches.push(patch);
    }
    Ok(())
}

/// Replaces the keys of the entry in `items` whose id is the patch's id.
/// Returns whether such an entry exists; an error names an invalid result.
/// The `exclusive` keys replace one another: a patch setting one of them
/// drops the others from the entry.
fn patch_entry<T>(
    items: &mut [T],
    id: impl Fn(&T) -> &str,
    patch: &RawEntryPatch,
    exclusive: &[&str],
) -> Result<bool, String>
where
    T: Serialize + DeserializeOwned,
{
    let Some(index) = items.iter().position(|item| id(item) == patch.id) else {
        return Ok(false);
    };
    let mut fields = toml::Value::try_from(&items[index])
        .map_err(|error| error.to_string())?
        .as_table()
        .cloned()
        .unwrap_or_default();
    if exclusive.iter().any(|key| patch.fields.contains_key(*key)) {
        for key in exclusive {
            fields.remove(*key);
        }
    }
    for (key, value) in &patch.fields {
        fields.insert(key.clone(), value.clone());
    }
    items[index] = toml::Value::Table(fields)
        .try_into()
        .map_err(|error: toml::de::Error| error.to_string())?;
    Ok(true)
}
