//! cosign key signing, offline: a Sigstore bundle signed with the key alone,
//! with no transparency log, timestamp authority or certificate authority.
//!
//! cosign 3 refuses the old `--output-signature` file and, left to its
//! defaults, fetches a signing config from Sigstore's TUF repository and
//! uploads each signature to the public Rekor log. A build must not publish
//! anything, so signing passes a signing config with no services, and
//! verification skips the log (`--insecure-ignore-tlog=true`), since the
//! signature was never in one. The image outputs (crates/core/exec/src/signing.rs)
//! and the registry cache both sign this way, so one key serves both.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A Sigstore signing config that names no services: no CA, OIDC provider,
/// Rekor log or timestamp authority.
pub const OFFLINE_SIGNING_CONFIG: &str = r#"{"mediaType":"application/vnd.dev.sigstore.signingconfig.v0.2+json","caUrls":[],"oidcUrls":[],"rekorTlogUrls":[],"tsaUrls":[],"rekorTlogConfig":{"selector":"ANY"},"tsaConfig":{"selector":"ANY"}}"#;

/// Where the signing config for `bundle` goes: `<bundle>.signing-config.json`.
pub fn offline_signing_config_path(bundle: &Path) -> PathBuf {
    let mut name = bundle.as_os_str().to_os_string();
    name.push(".signing-config.json");
    PathBuf::from(name)
}

/// Writes [`OFFLINE_SIGNING_CONFIG`] at [`offline_signing_config_path`] and
/// returns that path. The caller removes it once cosign has run.
pub fn write_offline_signing_config(bundle: &Path) -> std::io::Result<PathBuf> {
    let path = offline_signing_config_path(bundle);
    std::fs::write(&path, OFFLINE_SIGNING_CONFIG)?;
    Ok(path)
}

/// `cosign sign-blob` writing the Sigstore bundle for `data` to `bundle`
/// with `key`, under the offline `signing_config`.
pub fn sign_blob_command(
    tool: &Path,
    key: &str,
    data: &Path,
    bundle: &Path,
    signing_config: &Path,
) -> Command {
    let mut command = Command::new(tool);
    command
        .args(["sign-blob", "--yes", "--key"])
        .arg(key)
        .arg("--bundle")
        .arg(bundle)
        .arg("--signing-config")
        .arg(signing_config)
        .arg(data);
    command
}

/// `cosign verify-blob` checking `bundle` over `data` with the public `key`,
/// without a transparency log entry.
pub fn verify_blob_command(tool: &Path, key: &str, data: &Path, bundle: &Path) -> Command {
    let mut command = Command::new(tool);
    command
        .args(["verify-blob", "--key"])
        .arg(key)
        .arg("--bundle")
        .arg(bundle)
        .arg("--insecure-ignore-tlog=true")
        .arg(data);
    command
}
