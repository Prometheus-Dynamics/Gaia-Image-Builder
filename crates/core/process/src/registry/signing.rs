//! Signing and verification of entry manifests with the host's signing tools.
//!
//! The commands and flags match `signing_command` in crates/core/exec/src/signing.rs,
//! so one key serves both. Tools get no stdin: a key that asks for a passphrase
//! fails instead of hanging. Errors name the tool and the file, never a key.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::scratch::Scratch;
use super::{RegistryError, RegistryTarget, SignatureKind};
use crate::secrets;

/// Signs the bytes of `data` with the sign key and returns the signature bytes.
/// The signature file the tool writes beside `data` is removed before returning.
pub(super) fn sign_bytes(target: &RegistryTarget, data: &Path) -> Result<Vec<u8>, RegistryError> {
    let kind = target.kind;
    let key = key_text(target.sign_key.as_deref(), "sign key")?;
    let tool = resolve_tool(target, kind.program())?;
    let signature = Scratch::named(with_suffix(data, ".sig"));
    let command = sign_command(kind, &tool, &key, data, signature.path());
    run(command, kind.program(), data)?;
    std::fs::read(signature.path()).map_err(|error| {
        RegistryError::Io(format!("cannot read the signature the tool wrote: {error}"))
    })
}

/// Checks `signature` over `data` with the verify key. Any failure is a
/// `Signature` error, so the caller refuses the entry.
pub(super) fn verify_bytes(
    target: &RegistryTarget,
    data: &Path,
    signature: &Path,
) -> Result<(), RegistryError> {
    let kind = target.kind;
    let key = key_text(target.verify_key.as_deref(), "verify key")?;
    let tool = resolve_tool(target, kind.program())?;
    let command = verify_command(kind, &tool, &key, data, signature);
    let output = run(command, kind.program(), data)?;
    if kind == SignatureKind::Gpg && !gpg_names_key(&output.stdout, &key) {
        return Err(RegistryError::Signature(format!(
            "gpg reported no good signature from the expected key for {}",
            file_name(data)
        )));
    }
    Ok(())
}

/// The key a setting names, read now (the setting is a path, `env:VAR` or
/// `secret:NAME`). Errors name the setting, never the key.
fn key_text(setting: Option<&str>, what: &str) -> Result<String, RegistryError> {
    let setting =
        setting.ok_or_else(|| RegistryError::Signature(format!("no {what} is configured")))?;
    match secrets::key_setting(setting) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err(RegistryError::Signature(format!("the {what} is empty"))),
        Err(error) => Err(RegistryError::Signature(format!("{what}: {error}"))),
    }
}

/// `data` with `suffix` appended to its name.
fn with_suffix(data: &Path, suffix: &str) -> PathBuf {
    let mut name = data.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn sign_command(
    kind: SignatureKind,
    tool: &Path,
    key: &str,
    data: &Path,
    signature: &Path,
) -> Command {
    let mut command = Command::new(tool);
    match kind {
        SignatureKind::Minisign => {
            command
                .arg("-S")
                .arg("-s")
                .arg(key)
                .arg("-m")
                .arg(data)
                .arg("-x")
                .arg(signature);
        }
        SignatureKind::Gpg => {
            command
                .args(["--batch", "--yes", "--armor", "--detach-sign"])
                .arg("--local-user")
                .arg(key)
                .arg("--output")
                .arg(signature)
                .arg(data);
        }
        SignatureKind::CosignBlob => {
            command
                .args(["sign-blob", "--yes"])
                .arg("--key")
                .arg(key)
                .arg("--output-signature")
                .arg(signature)
                .arg(data);
        }
    }
    command
}

fn verify_command(
    kind: SignatureKind,
    tool: &Path,
    key: &str,
    data: &Path,
    signature: &Path,
) -> Command {
    let mut command = Command::new(tool);
    match kind {
        SignatureKind::Minisign => {
            command
                .arg("-V")
                .arg("-p")
                .arg(key)
                .arg("-m")
                .arg(data)
                .arg("-x")
                .arg(signature);
        }
        SignatureKind::Gpg => {
            // The status lines on stdout name the signing key, which is checked after.
            command
                .args(["--batch", "--status-fd", "1", "--verify"])
                .arg(signature)
                .arg(data);
        }
        SignatureKind::CosignBlob => {
            command
                .args(["verify-blob", "--key"])
                .arg(key)
                .arg("--signature")
                .arg(signature)
                .arg(data);
        }
    }
    command
}

/// The first `tool_dir/<program>` (or `PATH` entry) that is a file.
fn resolve_tool(target: &RegistryTarget, program: &str) -> Result<PathBuf, RegistryError> {
    let candidates: Vec<PathBuf> = match &target.tool_dir {
        Some(dir) => vec![dir.join(program)],
        None => std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths)
                    .map(|dir| dir.join(program))
                    .collect()
            })
            .unwrap_or_default(),
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| RegistryError::Io(format!("signing tool '{program}' was not found")))
}

/// Runs the tool with no stdin. A failed exit is a `Signature` error.
fn run(mut command: Command, program: &str, data: &Path) -> Result<Output, RegistryError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .map_err(|error| RegistryError::Io(format!("cannot run {program}: {error}")))?;
    if output.status.success() {
        return Ok(output);
    }
    let code = output
        .status
        .code()
        .map_or_else(|| "a signal".to_string(), |code| code.to_string());
    Err(RegistryError::Signature(format!(
        "{program} failed for {}: exit {code}",
        file_name(data)
    )))
}

/// Whether gpg's status output has a good signature from `key`. The key is
/// matched as a case-insensitive substring of the GOODSIG or VALIDSIG line,
/// which carries the key id or fingerprint.
fn gpg_names_key(status: &[u8], key: &str) -> bool {
    let wanted: String = key
        .trim()
        .trim_start_matches("0x")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if wanted.is_empty() {
        return false;
    }
    String::from_utf8_lossy(status).lines().any(|line| {
        (line.starts_with("[GNUPG:] GOODSIG ") || line.starts_with("[GNUPG:] VALIDSIG "))
            && line.to_ascii_uppercase().contains(&wanted)
    })
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
