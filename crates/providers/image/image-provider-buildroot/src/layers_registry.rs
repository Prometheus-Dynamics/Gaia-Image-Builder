//! Image layers in the registry cache: pulling a layer by its inputs key, and pushing built ones.

use super::*;

/// The registry key of a layer: the hex of its fingerprint (the `sha256:`
/// input key without its prefix). A fingerprint without that prefix is
/// hashed, so the key is always a hex tag.
pub(super) fn registry_key(inputs: &str) -> String {
    match inputs.strip_prefix("sha256:") {
        Some(hex) => hex.to_string(),
        None => gaia_process::hex(&Sha256::digest(inputs.as_bytes())),
    }
}

/// Pulls layer `name` for `inputs` from the registry and restores its tar and
/// erofs into `layers_dir` when its record and both files verify. Returns the
/// verified record (what `layers.json` records), or `None` when the registry
/// has no such entry. Nothing is written into `layers_dir` before it verifies.
pub(super) fn pull_layer(
    client: &RemoteCache,
    name: &str,
    inputs: &str,
    layers_dir: &Path,
) -> Result<Option<serde_json::Value>, String> {
    let key = registry_key(inputs);
    let staging = layers_dir.join(format!(".pull-{name}"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .map_err(|error| format!("registry pull of layer '{name}' ({key}): {error}"))?;
    let result = pull_layer_into(client, name, &key, inputs, layers_dir, &staging);
    let _ = fs::remove_dir_all(&staging);
    result.map_err(|reason| format!("registry pull of layer '{name}' ({key}): {reason}"))
}

pub(super) fn pull_layer_into(
    client: &RemoteCache,
    name: &str,
    key: &str,
    inputs: &str,
    layers_dir: &Path,
    staging: &Path,
) -> Result<Option<serde_json::Value>, String> {
    let entry = match client.pull(LAYER_KIND, key, staging) {
        Ok(Some(entry)) => entry,
        Ok(None) => return Ok(None),
        Err(RegistryError::Signature(reason)) => {
            return Err(format!(
                "SIGNATURE DOES NOT VERIFY, entry refused, the layer is built instead: {reason}"
            ));
        }
        Err(error) => return Err(error.to_string()),
    };
    let annotation = |field: &str| entry.annotations.get(field).map(String::as_str);
    if annotation(LAYER_ANNOTATION) != Some(name) || annotation(LAYER_STEM_ANNOTATION) != Some(name)
    {
        return Err("the entry is for another layer".into());
    }
    let unpacked = staging.join("unpacked");
    unpack_entry(&entry.blob, &unpacked, name).map_err(|error| error.to_string())?;
    let text = fs::read_to_string(unpacked.join(format!("{name}.json")))
        .map_err(|error| error.to_string())?;
    let record: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "the layer record is unreadable".to_string())?;
    if record["name"] != name || record["inputs"] != inputs {
        return Err("the layer record is for another layer or fingerprint".into());
    }
    let payload = unpacked.join(name);
    let tar_name = format!("{name}.tar");
    if record["tar"] != tar_name.as_str() {
        return Err("the layer record names another tar".into());
    }
    let tar = payload.join(&tar_name);
    if !tar_matches(
        &tar,
        record["tar_size"].as_u64(),
        record["tar_sha256"].as_str(),
    ) {
        return Err("the layer tar does not match its record".into());
    }
    let mut moves = vec![(tar, layers_dir.join(&tar_name))];
    if !record["erofs"].is_null() {
        let erofs_name = format!("{name}.erofs");
        if record["erofs"] != erofs_name.as_str() {
            return Err("the layer record names another erofs".into());
        }
        let erofs = payload.join(&erofs_name);
        if !tar_matches(
            &erofs,
            record["erofs_size"].as_u64(),
            record["erofs_sha256"].as_str(),
        ) {
            return Err("the layer erofs does not match its record".into());
        }
        moves.push((erofs, layers_dir.join(&erofs_name)));
    }
    for (from, to) in moves {
        fs::rename(&from, &to)
            .map_err(|error| format!("failed to restore '{}': {error}", to.display()))?;
    }
    Ok(Some(record))
}

/// Pushes the layers built here, one entry each, and returns the run messages.
pub(super) fn push_layers(
    client: &RemoteCache,
    checkpoint_id: &str,
    rebuilt: &[(String, String, serde_json::Value)],
    layers_dir: &Path,
) -> Vec<String> {
    let mut messages = Vec::new();
    let mut pushed = Vec::new();
    for (name, inputs, record) in rebuilt {
        match push_layer(client, name, inputs, record, layers_dir) {
            Ok(PushOutcome::Pushed { .. }) => pushed.push(name.clone()),
            Ok(PushOutcome::AlreadyPresent | PushOutcome::Disabled) => {}
            Err(message) => messages.push(message),
        }
    }
    if !pushed.is_empty() {
        messages.push(format!(
            "layers: {} pushed to the registry checkpoint '{checkpoint_id}': {}",
            pushed.len(),
            pushed.join(", ")
        ));
    }
    messages
}

/// Packs layer `name` (its tar, its erofs when it has one, and its record) and
/// pushes it under its fingerprint's key. Nothing is pushed without a sign key.
pub(super) fn push_layer(
    client: &RemoteCache,
    name: &str,
    inputs: &str,
    record: &serde_json::Value,
    layers_dir: &Path,
) -> Result<PushOutcome, String> {
    let key = registry_key(inputs);
    let staging = layers_dir.join(format!(".push-{name}"));
    let _ = fs::remove_dir_all(&staging);
    let result = push_layer_from(client, name, &key, record, layers_dir, &staging);
    let _ = fs::remove_dir_all(&staging);
    result.map_err(|reason| {
        format!("registry push of layer '{name}' failed (the build is not affected): {reason}")
    })
}

pub(super) fn push_layer_from(
    client: &RemoteCache,
    name: &str,
    key: &str,
    record: &serde_json::Value,
    layers_dir: &Path,
    staging: &Path,
) -> Result<PushOutcome, String> {
    let payload = staging.join("payload");
    let dir = payload.join(name);
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let mut files = vec![format!("{name}.tar")];
    if !record["erofs"].is_null() {
        files.push(format!("{name}.erofs"));
    }
    for file in &files {
        link_or_copy(&layers_dir.join(file), &dir.join(file))?;
    }
    let record_name = format!("{name}.json");
    let text = serde_json::to_string_pretty(record).map_err(|error| error.to_string())?;
    fs::write(payload.join(&record_name), format!("{text}\n"))
        .map_err(|error| error.to_string())?;
    let blob = staging.join("entry.tar.zst");
    pack_entry(&payload, &[name, record_name.as_str()], &blob)
        .map_err(|error| error.to_string())?;
    let annotations = BTreeMap::from([
        (LAYER_ANNOTATION.to_string(), name.to_string()),
        (LAYER_STEM_ANNOTATION.to_string(), name.to_string()),
    ]);
    client
        .push(LAYER_KIND, key, &blob, &annotations)
        .map_err(|error| error.to_string())
}

/// A hard link to `from` at `to`, or a copy when linking is not possible. The
/// archive holds one name per file, so a linked file packs as a regular file.
pub(super) fn link_or_copy(from: &Path, to: &Path) -> Result<(), String> {
    if fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)
        .map(drop)
        .map_err(|error| format!("failed to stage '{}': {error}", from.display()))
}
