//! `gaia cache --prune --remote`: deletes the registry entries of the
//! `backend = "oci"` checkpoint that are older than `--keep-days`.
//!
//! An entry is kept while its created time (the manifest's
//! `org.opencontainers.image.created`) is within the window, and when that
//! time cannot be read: pruning never guesses. An entry goes with its
//! signature tag. `--dry-run` lists what would go and deletes nothing.
//!
//! Only the generic OCI DELETE (by manifest digest) is used. Registries that
//! refuse it, such as GHCR, need the GitHub Packages API and a token with
//! `delete:packages`; a refused delete is reported and the run goes on.

use gaia_image_providers::registry_cache_from_spec;
use gaia_process::{RemoteCache, created_epoch_seconds};
use gaia_spec::ResolvedBuildSpec;
use std::time::{SystemTime, UNIX_EPOCH};

/// Entry kinds in the registry cache (each is one image, `<repository>/<kind>`).
const KINDS: &[&str] = &["package", "artifact", "layer"];
const CREATED: &str = "org.opencontainers.image.created";
/// `--keep-days` when it is not given.
pub(super) const DEFAULT_KEEP_DAYS: u32 = 30;

/// Prunes the registry cache of the oci checkpoint (`checkpoint`, when given,
/// must be that one). Returns the lines to print.
pub(super) fn prune_remote(
    spec: &ResolvedBuildSpec,
    checkpoint: Option<&str>,
    keep_days: u32,
    dry_run: bool,
) -> Result<Vec<String>, String> {
    let registry = registry_cache_from_spec(spec)?.ok_or_else(|| {
        "no checkpoint uses backend 'oci': --remote needs one in [[checkpoints]]".to_string()
    })?;
    if let Some(id) = checkpoint
        && id != registry.checkpoint_id
    {
        return Err(format!(
            "checkpoint '{id}' is not the oci checkpoint, which is '{}'",
            registry.checkpoint_id
        ));
    }
    let client = registry.client();
    let repository = registry.target.repository.clone();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs()) as i64;
    let cutoff = now - i64::from(keep_days) * 86_400;
    let mut lines = Vec::new();
    let (mut candidates, mut kept) = (0usize, 0usize);
    for kind in KINDS {
        let image = format!("{repository}/{kind}");
        let tags = client
            .list_tags(&image)
            .map_err(|error| format!("listing {image}: {error}"))?;
        for tag in tags.iter().filter(|tag| !tag.ends_with(".sig")) {
            let Some((manifest, digest)) = client
                .get_manifest(&image, tag)
                .map_err(|error| format!("reading {image}:{tag}: {error}"))?
            else {
                continue;
            };
            let created = created_time(&manifest);
            if !created.is_some_and(|time| time < cutoff) {
                kept += 1;
                continue;
            }
            candidates += 1;
            if dry_run {
                lines.push(format!("would delete {kind}:{tag}"));
                continue;
            }
            match delete_entry(&client, &image, tag, &digest) {
                Ok(()) => lines.push(format!("deleted {kind}:{tag}")),
                Err(reason) => lines.push(format!("could not delete {kind}:{tag}: {reason}")),
            }
        }
    }
    lines.push(format!(
        "{} {candidates} entr{} older than {keep_days} day(s); {kept} kept",
        if dry_run { "would delete" } else { "deleted" },
        if candidates == 1 { "y" } else { "ies" },
    ));
    Ok(lines)
}

/// The created time of a manifest, in epoch seconds.
fn created_time(manifest: &[u8]) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    let text = value.get("annotations")?.get(CREATED)?.as_str()?;
    created_epoch_seconds(text)
}

/// Deletes an entry's manifest and its signature's, by digest.
fn delete_entry(client: &RemoteCache, image: &str, tag: &str, digest: &str) -> Result<(), String> {
    client
        .delete_manifest(image, digest)
        .map_err(|error| error.to_string())?;
    let signature = client
        .get_manifest(image, &format!("{tag}.sig"))
        .map_err(|error| error.to_string())?;
    if let Some((_, signature_digest)) = signature {
        client
            .delete_manifest(image, &signature_digest)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_created_time_of_a_manifest_is_read_from_its_annotations() {
        let manifest = br#"{"annotations":{"org.opencontainers.image.created":"1970-01-02T00:00:00Z"}}"#;
        assert_eq!(created_time(manifest), Some(86_400));
        assert_eq!(created_time(b"{}"), None);
        assert_eq!(created_time(b"not json"), None);
    }
}
