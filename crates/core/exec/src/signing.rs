//! Signatures of the image outputs a run published (`[image.signing]`).
//!
//! The signing runs after a successful run, over every published output that
//! matches `outputs`, so an image the run reused is signed as well. Each
//! signature is written next to its output: `<file>.minisig` (minisign),
//! `<file>.asc` (gpg, armored and detached) or `<file>.sig` (cosign blob). The
//! signing tools are the host's own programs; Gaia holds no key material and
//! never prints the key.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gaia_plan::{ExecutionPlan, OperationId, OperationKind};
use gaia_spec::{ImageSigningKind, ImageSigningSpec, ResolvedBuildSpec};

use crate::operations::{ExecutionCleanupStatus, ExecutionError, ExecutionErrorKind};
use crate::runtime::ExecutionOutcome;

/// Name suffix that marks a verity root hash file: signed whether or not a
/// glob names it.
const ROOT_HASH_SUFFIX: &str = ".roothash";

/// Signs the outputs of `published` that `signing` selects. Returns the
/// signature files written, in the order of the outputs.
pub(crate) fn sign_published_outputs(
    signing: &ImageSigningSpec,
    published: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    sign_published_outputs_with_env(signing, published, &[])
}

/// [`sign_published_outputs`], with `env` added to the tool's environment.
fn sign_published_outputs_with_env(
    signing: &ImageSigningSpec,
    published: &[PathBuf],
    env: &[(OsString, OsString)],
) -> Result<Vec<PathBuf>, String> {
    let kind = ImageSigningKind::parse(&signing.kind).ok_or_else(|| {
        format!(
            "[image.signing] kind '{}' is not minisign, gpg or cosign-blob",
            signing.kind
        )
    })?;
    let selected = select_outputs(&signing.outputs, published);
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let key = resolve_key(&signing.key)?;
    match kind {
        ImageSigningKind::Minisign | ImageSigningKind::CosignBlob => {
            if !Path::new(&key).is_file() {
                return Err(format!(
                    "[image.signing] key file not found; set key to the path of the {} key, or to env:VAR naming a variable that holds it",
                    kind.program()
                ));
            }
        }
        ImageSigningKind::Gpg => {
            if key.trim().is_empty() {
                return Err("[image.signing] gpg key is empty; set key to a key id or env:VAR naming a variable that holds one".into());
            }
        }
    }
    let tool = resolve_program(kind.program()).ok_or_else(|| {
        format!(
            "[image.signing] kind '{}' needs '{}' on PATH, which was not found",
            signing.kind,
            kind.program()
        )
    })?;

    let mut written = Vec::with_capacity(selected.len());
    for file in selected {
        let signature = signature_path(kind, &file);
        let mut command = signing_command(kind, &tool, &key, &file, &signature);
        command.envs(env.iter().map(|(name, value)| (name, value)));
        run_signing(&mut command, kind, &file)?;
        written.push(signature);
    }
    Ok(written)
}

/// The outputs `patterns` select from `published`: files (not missing paths)
/// whose name matches a pattern, plus any root hash file, in the order given.
pub(crate) fn select_outputs(patterns: &[String], published: &[PathBuf]) -> Vec<PathBuf> {
    let mut selected: Vec<PathBuf> = Vec::new();
    for path in published {
        if selected.contains(path) || !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(ROOT_HASH_SUFFIX)
            || patterns.iter().any(|pattern| glob_match(pattern, name))
        {
            selected.push(path.clone());
        }
    }
    selected
}

/// Whether `text` matches `pattern`, where `*` matches any run of characters
/// (including none) and `?` any single character.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    // The last `*` seen, and the text position it has absorbed up to.
    let mut star: Option<usize> = None;
    let mut absorbed = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            absorbed = t;
            p += 1;
        } else if let Some(star_at) = star {
            p = star_at + 1;
            absorbed += 1;
            t = absorbed;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// The signature file for `file`: the output's name with the kind's suffix.
pub(crate) fn signature_path(kind: ImageSigningKind, file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(kind.signature_suffix());
    PathBuf::from(name)
}

/// The command that writes `signature` for `file` with `key`. Only the
/// arguments name the key; nothing here prints it.
pub(crate) fn signing_command(
    kind: ImageSigningKind,
    tool: &Path,
    key: &str,
    file: &Path,
    signature: &Path,
) -> Command {
    let mut command = Command::new(tool);
    match kind {
        ImageSigningKind::Minisign => {
            command
                .arg("-S")
                .arg("-s")
                .arg(key)
                .arg("-m")
                .arg(file)
                .arg("-x")
                .arg(signature);
        }
        ImageSigningKind::Gpg => {
            command
                .args(["--batch", "--yes", "--armor", "--detach-sign"])
                .arg("--local-user")
                .arg(key)
                .arg("--output")
                .arg(signature)
                .arg(file);
        }
        ImageSigningKind::CosignBlob => {
            command
                .args(["sign-blob", "--yes"])
                .arg("--key")
                .arg(key)
                .arg("--output-signature")
                .arg(signature)
                .arg(file);
        }
    }
    command
}

/// The key the setting names: the setting itself, or the value of the
/// variable `env:VAR` names. The error names the variable, never its value.
fn resolve_key(setting: &str) -> Result<String, String> {
    let Some(name) = setting.strip_prefix("env:") else {
        return Ok(setting.to_string());
    };
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        _ => Err(format!(
            "[image.signing] key: environment variable '{name}' is not set or empty"
        )),
    }
}

/// The first `program` in `PATH` that is a file.
fn resolve_program(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|dir| Path::new(dir).join(program))
        .find(|candidate| candidate.is_file())
}

fn run_signing(command: &mut Command, kind: ImageSigningKind, file: &Path) -> Result<(), String> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("failed to start '{}': {error}", kind.program()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no output");
    Err(format!(
        "signing '{}' with {} failed: {}",
        file.display(),
        kind.program(),
        reason.trim()
    ))
}

/// Signs the outputs `outcome` published when `[image.signing]` is set. The
/// signature files join the published outputs of the image results, so the
/// report lists them; a failure is an error of the run.
pub(crate) fn sign_outcome(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    outcome: &mut ExecutionOutcome,
) {
    let Some(signing) = spec.image.signing.as_ref() else {
        return;
    };
    let mut published: Vec<PathBuf> = Vec::new();
    for result in &outcome.image_results {
        for path in result.archive_path.iter().chain(&result.disk_images) {
            if !published.contains(path) {
                published.push(path.clone());
            }
        }
    }
    match sign_published_outputs(signing, &published) {
        Ok(signatures) => {
            if let Some(last) = outcome.image_results.last_mut() {
                last.disk_images.extend(signatures);
            }
        }
        Err(message) => {
            let operation_id = plan
                .operations
                .iter()
                .rev()
                .find(|operation| {
                    matches!(
                        operation.kind,
                        OperationKind::AssembleImage | OperationKind::BuildImage
                    )
                })
                .map(|operation| operation.id.clone())
                .unwrap_or_else(OperationId::resolve);
            outcome.errors.push(ExecutionError {
                code: "image_signing_failed",
                kind: ExecutionErrorKind::BackendCommand,
                operation_id,
                message,
                output_tail: Vec::new(),
                cleanup_domain: None,
                cleanup_paths: Vec::new(),
                cleanup_status: ExecutionCleanupStatus::NotRequired,
                cleanup_failures: Vec::new(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;
    use std::fs;

    fn spec(kind: &str, key: &str, outputs: &[&str]) -> ImageSigningSpec {
        ImageSigningSpec {
            kind: kind.into(),
            key: key.into(),
            outputs: outputs.iter().map(|pattern| (*pattern).into()).collect(),
        }
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn star_and_question_mark_match_file_names() {
        assert!(glob_match("*.img.xz", "rootfs.img.xz"));
        assert!(!glob_match("*.img.xz", "rootfs.img.xzz"));
        assert!(glob_match("disk-?.img", "disk-1.img"));
        assert!(!glob_match("disk-?.img", "disk-12.img"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("a*b*c", "axxbyy"));
        assert!(glob_match("*.slot.tar", "app.slot.tar"));
    }

    #[test]
    fn selection_takes_matching_files_and_root_hashes_in_order() {
        let scratch = TempDir::new("gaia-signing");
        let write = |name: &str| {
            let path = scratch.join(name);
            fs::write(&path, b"x").expect("write");
            path
        };
        let image = write("rootfs.img.xz");
        let slot = write("app.slot.tar");
        let hash = write("data.roothash");
        let other = write("notes.txt");
        let missing = scratch.join("gone.img.xz");
        let published = vec![
            slot.clone(),
            other,
            missing,
            image.clone(),
            hash.clone(),
            slot.clone(),
        ];

        let selected = select_outputs(&["*.img.xz".into(), "*.slot.tar".into()], &published);

        assert_eq!(selected, vec![slot, image, hash]);
    }

    #[test]
    fn each_kind_writes_its_signature_next_to_the_output() {
        let file = Path::new("/out/rootfs.img.xz");
        assert_eq!(
            signature_path(ImageSigningKind::Minisign, file),
            PathBuf::from("/out/rootfs.img.xz.minisig")
        );
        assert_eq!(
            signature_path(ImageSigningKind::Gpg, file),
            PathBuf::from("/out/rootfs.img.xz.asc")
        );
        assert_eq!(
            signature_path(ImageSigningKind::CosignBlob, file),
            PathBuf::from("/out/rootfs.img.xz.sig")
        );
    }

    #[test]
    fn commands_name_the_tool_key_file_and_signature() {
        let file = Path::new("/out/app.slot.tar");
        let sig = Path::new("/out/app.slot.tar.minisig");
        let minisign = signing_command(
            ImageSigningKind::Minisign,
            Path::new("/usr/bin/minisign"),
            "/keys/gaia.key",
            file,
            sig,
        );
        assert_eq!(minisign.get_program(), "/usr/bin/minisign");
        assert_eq!(
            args(&minisign),
            [
                "-S",
                "-s",
                "/keys/gaia.key",
                "-m",
                "/out/app.slot.tar",
                "-x",
                "/out/app.slot.tar.minisig"
            ]
        );

        let gpg = signing_command(
            ImageSigningKind::Gpg,
            Path::new("/usr/bin/gpg"),
            "release@example.org",
            file,
            Path::new("/out/app.slot.tar.asc"),
        );
        assert_eq!(
            args(&gpg),
            [
                "--batch",
                "--yes",
                "--armor",
                "--detach-sign",
                "--local-user",
                "release@example.org",
                "--output",
                "/out/app.slot.tar.asc",
                "/out/app.slot.tar",
            ]
        );

        let cosign = signing_command(
            ImageSigningKind::CosignBlob,
            Path::new("/usr/bin/cosign"),
            "/keys/cosign.key",
            file,
            Path::new("/out/app.slot.tar.sig"),
        );
        assert_eq!(
            args(&cosign),
            [
                "sign-blob",
                "--yes",
                "--key",
                "/keys/cosign.key",
                "--output-signature",
                "/out/app.slot.tar.sig",
                "/out/app.slot.tar",
            ]
        );
    }

    #[test]
    fn an_unknown_kind_or_missing_key_fails_before_any_tool_runs() {
        let scratch = TempDir::new("gaia-signing");
        let output = scratch.join("rootfs.img.xz");
        fs::write(&output, b"x").expect("write");
        let published = vec![output];

        let error = sign_published_outputs(&spec("pgp", "k", &["*.img.xz"]), &published)
            .expect_err("unknown kind");
        assert!(
            error.contains("'pgp' is not minisign, gpg or cosign-blob"),
            "{error}"
        );

        let variable = "GAIA_TEST_SIGNING_KEY_NOT_SET_0F3A";
        let error = sign_published_outputs(
            &spec("minisign", &format!("env:{variable}"), &["*.img.xz"]),
            &published,
        )
        .expect_err("missing key");
        assert!(error.contains(variable), "{error}");
    }

    #[test]
    fn a_missing_key_file_is_named_without_its_path() {
        let scratch = TempDir::new("gaia-signing");
        let output = scratch.join("rootfs.img.xz");
        fs::write(&output, b"x").expect("write");
        let secret_path = "/no/such/dir/signing-key-0F3A.key";
        let error =
            sign_published_outputs(&spec("cosign-blob", secret_path, &["*.img.xz"]), &[output])
                .expect_err("missing key file");
        assert!(error.contains("key file not found"), "{error}");
        assert!(!error.contains(secret_path), "{error}");
    }

    #[test]
    fn gpg_signs_selected_outputs_with_a_throwaway_keyring() {
        if resolve_program("gpg").is_none() {
            return;
        }
        let scratch = TempDir::new("gpg");
        let home = scratch.join("gnupg");
        fs::create_dir_all(&home).expect("home");
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).expect("mode");
        }
        let uid = "Gaia Test <gaia-test@example.invalid>";
        let generated = Command::new("gpg")
            .env("GNUPGHOME", &home)
            .args([
                "--batch",
                "--pinentry-mode",
                "loopback",
                "--passphrase",
                "",
                "--quick-gen-key",
                uid,
                "ed25519",
                "default",
                "never",
            ])
            .stdin(Stdio::null())
            .output()
            .expect("gpg");
        assert!(
            generated.status.success(),
            "gpg could not create a throwaway key: {}",
            String::from_utf8_lossy(&generated.stderr)
        );

        let output = scratch.join("rootfs.img.xz");
        fs::write(&output, b"image bytes").expect("write");
        let skipped = scratch.join("notes.txt");
        fs::write(&skipped, b"notes").expect("write");
        let env = [(OsString::from("GNUPGHOME"), home.clone().into_os_string())];

        let written = sign_published_outputs_with_env(
            &spec("gpg", "gaia-test@example.invalid", &["*.img.xz"]),
            &[output.clone(), skipped.clone()],
            &env,
        )
        .expect("sign");

        let signature = scratch.join("rootfs.img.xz.asc");
        assert_eq!(written, vec![signature.clone()]);
        assert!(!scratch.join("notes.txt.asc").exists());
        let verified = Command::new("gpg")
            .env("GNUPGHOME", &home)
            .args(["--batch", "--verify"])
            .arg(&signature)
            .arg(&output)
            .stdin(Stdio::null())
            .output()
            .expect("gpg verify");
        assert!(
            verified.status.success(),
            "{}",
            String::from_utf8_lossy(&verified.stderr)
        );
    }
}
