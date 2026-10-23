//! Secrets: values a build file names without storing them.
//!
//! A `[secrets]` entry names where a value comes from: an environment
//! variable, a file, or the output of a command. The value is read when it is
//! used, never when the build file loads, so it is not in the resolved spec,
//! the fingerprints, the reuse state, the provenance or `gaia config show`.
//! The spec keeps the reference (`secret:NAME`, or `${secret:NAME}` inside a
//! build-time process environment), and this module turns it into a value.
//!
//! Every value of at least [`MIN_MASKED_LEN`] characters that was resolved is
//! registered with the masker. Captured process output, failure messages and
//! report files replace it with the replacement text (`***` by default), so a
//! build that echoes its secret does not put it in a log or a report.
//!
//! The registry is process-wide: [`install`] adds the entries of a run, and the
//! masks only grow, so a value stays masked for the rest of the process.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, RwLock};

/// Replacement text for a masked secret when a run sets none.
pub const DEFAULT_REPLACEMENT: &str = "***";
/// Shorter values are not masked: a one- or two-character value would hide
/// ordinary text.
pub const MIN_MASKED_LEN: usize = 4;
/// Prefix of a reference to a `[secrets]` entry.
pub const SECRET_PREFIX: &str = "secret:";
/// Prefix of a reference to an environment variable.
pub const ENV_PREFIX: &str = "env:";
/// Opening text of an interpolated secret reference inside a string.
const INTERPOLATED_PREFIX: &str = "${secret:";

/// Where a secret's value comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretSource {
    /// The value of an environment variable.
    Env(String),
    /// The contents of a file, with its trailing newline removed. The file
    /// must not be readable by group or others.
    File(PathBuf),
    /// The standard output of a program with its arguments, trimmed.
    Command(Vec<String>),
}

impl SecretSource {
    /// A description of the source that names no value: the variable, the
    /// path or the program.
    pub fn describe(&self) -> String {
        match self {
            Self::Env(var) => format!("environment variable '{var}'"),
            Self::File(path) => format!("file {}", path.display()),
            Self::Command(argv) => format!(
                "command '{}'",
                argv.first().map(String::as_str).unwrap_or_default()
            ),
        }
    }
}

/// One `[secrets]` entry, with a file path already made absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretDef {
    pub name: String,
    pub source: SecretSource,
    /// A missing or failed source is an error when the secret is used. When
    /// false, the reference resolves to nothing instead.
    pub required: bool,
}

/// A resolved secret value. `Debug` and `Display` print `<secret LABEL>`,
/// where the label names the entry (or `env:VAR` for an environment setting),
/// never the value.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue {
    label: String,
    value: String,
}

impl SecretValue {
    fn new(label: impl Into<String>, value: String) -> Self {
        let label = label.into();
        if value.len() >= MIN_MASKED_LEN {
            register_mask(&value);
        }
        Self { label, value }
    }

    /// The value. Only the code that hands it to a process or a registry
    /// request should read it.
    pub fn expose(&self) -> &str {
        &self.value
    }

    /// The name of the entry the value came from.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<secret {}>", self.label)
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<secret {}>", self.label)
    }
}

#[derive(Default)]
struct Catalog {
    defs: BTreeMap<String, SecretDef>,
    /// Values already read, so a command runs once per process.
    resolved: BTreeMap<String, SecretValue>,
}

static CATALOG: Mutex<Catalog> = Mutex::new(Catalog {
    defs: BTreeMap::new(),
    resolved: BTreeMap::new(),
});
/// Every registered secret value, longest first when masking.
static MASKS: RwLock<Vec<String>> = RwLock::new(Vec::new());
/// The replacement text, empty until a run sets one.
static REPLACEMENT: RwLock<String> = RwLock::new(String::new());

/// Adds `defs` to the entries this process knows, replacing an entry of the
/// same name, and sets the replacement text when `replacement` is not empty.
pub fn install(defs: Vec<SecretDef>, replacement: &str) {
    let mut catalog = lock(&CATALOG);
    for def in defs {
        catalog.resolved.remove(&def.name);
        catalog.defs.insert(def.name.clone(), def);
    }
    drop(catalog);
    if !replacement.is_empty() {
        *REPLACEMENT
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = replacement.to_string();
    }
}

/// Whether an entry of this name was installed.
pub fn is_defined(name: &str) -> bool {
    lock(&CATALOG).defs.contains_key(name)
}

/// The value of the entry `name`. `Ok(None)` when the entry is optional and
/// its source is missing or failed. The error names the entry and its source,
/// never a value.
pub fn resolve(name: &str) -> Result<Option<SecretValue>, String> {
    let (def, cached) = {
        let catalog = lock(&CATALOG);
        let Some(def) = catalog.defs.get(name) else {
            return Err(format!("secret '{name}' is not defined in [secrets]"));
        };
        (def.clone(), catalog.resolved.get(name).cloned())
    };
    if let Some(value) = cached {
        return Ok(Some(value));
    }
    match fetch(&def) {
        Ok(value) => {
            lock(&CATALOG)
                .resolved
                .insert(name.to_string(), value.clone());
            Ok(Some(value))
        }
        Err(_) if !def.required => Ok(None),
        Err(error) => Err(error),
    }
}

fn fetch(def: &SecretDef) -> Result<SecretValue, String> {
    let name = def.name.as_str();
    let text = match &def.source {
        SecretSource::Env(var) => match std::env::var(var) {
            Ok(value) if !value.is_empty() => value,
            _ => {
                return Err(format!(
                    "secret '{name}': environment variable '{var}' is not set or empty"
                ));
            }
        },
        SecretSource::File(path) => read_secret_file(name, path)?,
        SecretSource::Command(argv) => run_secret_command(name, argv)?,
    };
    let trimmed = match &def.source {
        SecretSource::Env(_) => text,
        SecretSource::File(_) => text.trim_end_matches(['\r', '\n']).to_string(),
        SecretSource::Command(_) => text.trim().to_string(),
    };
    if trimmed.is_empty() {
        return Err(format!(
            "secret '{name}': {} produced no value",
            def.source.describe()
        ));
    }
    Ok(SecretValue::new(name, trimmed))
}

/// Reads a secret file. The file must not be readable by group or others.
fn read_secret_file(name: &str, path: &Path) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|_| {
        format!(
            "secret '{name}': file {} cannot be read (missing or not a file)",
            path.display()
        )
    })?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "secret '{name}': file {} is readable by other users; fix it with: chmod 600 {}",
            path.display(),
            path.display()
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "secret '{name}': file {} cannot be read: {}",
            path.display(),
            error.kind()
        )
    })?;
    String::from_utf8(bytes)
        .map_err(|_| format!("secret '{name}': file {} is not UTF-8 text", path.display()))
}

/// Runs a secret command. Its standard error is discarded, never echoed.
fn run_secret_command(name: &str, argv: &[String]) -> Result<String, String> {
    let Some((program, args)) = argv.split_first() else {
        return Err(format!("secret '{name}': command is empty"));
    };
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| format!("secret '{name}': command '{program}' could not be started"))?;
    if !output.status.success() {
        return Err(format!(
            "secret '{name}': command '{program}' failed (exit {})",
            output
                .status
                .code()
                .map_or_else(|| "by signal".to_string(), |code| code.to_string())
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("secret '{name}': command '{program}' printed text that is not UTF-8"))
}

/// Resolves a key setting: a literal (a path, or a key id for gpg), `env:VAR`
/// naming a variable that holds it, or `secret:NAME`. `Ok(None)` for an empty
/// setting. Errors name the variable or entry, never a value.
pub fn key_setting(setting: &str) -> Result<Option<String>, String> {
    let setting = setting.trim();
    if setting.is_empty() {
        return Ok(None);
    }
    if let Some(name) = setting.strip_prefix(SECRET_PREFIX) {
        return Ok(resolve(name)?.map(|value| value.expose().to_string()));
    }
    if let Some(var) = setting.strip_prefix(ENV_PREFIX) {
        return match std::env::var(var) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_string())),
            _ => Err(format!("environment variable '{var}' is not set or empty")),
        };
    }
    Ok(Some(setting.to_string()))
}

/// Resolves a credential setting, which must name a secret: `secret:NAME`, or
/// `env:VAR` (the variable's value is then registered as a secret too).
/// `Ok(None)` when an optional secret is missing or the setting is empty.
pub fn credential_setting(setting: &str) -> Result<Option<SecretValue>, String> {
    let setting = setting.trim();
    if setting.is_empty() {
        return Ok(None);
    }
    if let Some(name) = setting.strip_prefix(SECRET_PREFIX) {
        return resolve(name);
    }
    if let Some(var) = setting.strip_prefix(ENV_PREFIX) {
        return match std::env::var(var) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(SecretValue::new(
                format!("{ENV_PREFIX}{var}"),
                value.trim().to_string(),
            ))),
            _ => Err(format!("environment variable '{var}' is not set or empty")),
        };
    }
    Err("a credential must be secret:NAME or env:VAR, never the value itself".to_string())
}

/// Replaces each `${secret:NAME}` in `text` with the entry's value, for a
/// build-time process environment. An optional entry that is missing becomes
/// empty. The error names the entry, never a value.
pub fn expand_child_value(text: &str) -> Result<String, String> {
    if !text.contains(INTERPOLATED_PREFIX) {
        return Ok(text.to_string());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(INTERPOLATED_PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start + INTERPOLATED_PREFIX.len()..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        if let Some(value) = resolve(name)? {
            out.push_str(value.expose());
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Makes a `[secrets]` file path absolute: `~` is the home directory, and a
/// relative path is taken from `base` (the workspace root).
pub fn file_path(text: &str, base: &Path) -> PathBuf {
    let expanded = match text.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(text),
        },
        None => PathBuf::from(text),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

fn register_mask(value: &str) {
    let mut masks = MASKS.write().unwrap_or_else(|poison| poison.into_inner());
    if !masks.iter().any(|mask| mask == value) {
        masks.push(value.to_string());
        masks.sort_by_key(|mask| std::cmp::Reverse(mask.len()));
    }
}

/// The text with every registered secret value replaced by the replacement.
pub fn mask(text: &str) -> Cow<'_, str> {
    let masks = MASKS.read().unwrap_or_else(|poison| poison.into_inner());
    if masks.is_empty() || !masks.iter().any(|mask| text.contains(mask.as_str())) {
        return Cow::Borrowed(text);
    }
    let replacement = replacement_text();
    let mut out = text.to_string();
    for mask in masks.iter() {
        if out.contains(mask.as_str()) {
            out = out.replace(mask.as_str(), &replacement);
        }
    }
    Cow::Owned(out)
}

/// [`mask`] for lines of text.
pub fn mask_lines(lines: &[String]) -> Vec<String> {
    lines.iter().map(|line| mask(line).into_owned()).collect()
}

fn replacement_text() -> String {
    let text = REPLACEMENT
        .read()
        .unwrap_or_else(|poison| poison.into_inner());
    if text.is_empty() {
        DEFAULT_REPLACEMENT.to_string()
    } else {
        text.clone()
    }
}

fn lock(catalog: &Mutex<Catalog>) -> std::sync::MutexGuard<'_, Catalog> {
    catalog.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;
    use std::os::unix::fs::PermissionsExt;

    fn file_def(name: &str, path: &Path, required: bool) -> SecretDef {
        SecretDef {
            name: name.into(),
            source: SecretSource::File(path.to_path_buf()),
            required,
        }
    }

    fn write_secret(dir: &TempDir, file: &str, text: &str, mode: u32) -> PathBuf {
        let path = dir.join(file);
        fs::write(&path, text).expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("mode");
        path
    }

    #[test]
    fn a_file_secret_is_read_trimmed_and_masked() {
        let dir = TempDir::new("gaia-secrets-file");
        let path = write_secret(&dir, "token", "gaia-test-file-token-7781\n", 0o600);
        install(vec![file_def("gaia_test_file_token", &path, true)], "");

        let value = resolve("gaia_test_file_token")
            .expect("resolve")
            .expect("set");
        assert_eq!(value.expose(), "gaia-test-file-token-7781");
        assert_eq!(format!("{value:?}"), "<secret gaia_test_file_token>");
        assert_eq!(
            mask("token gaia-test-file-token-7781 end").as_ref(),
            "token *** end"
        );
    }

    #[test]
    fn a_file_readable_by_others_is_refused_naming_the_path_and_fix() {
        let dir = TempDir::new("gaia-secrets-mode");
        let path = write_secret(&dir, "loose", "gaia-test-loose-value-1\n", 0o644);
        install(vec![file_def("gaia_test_loose", &path, true)], "");

        let error = resolve("gaia_test_loose").expect_err("refused");
        assert!(error.contains("gaia_test_loose"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("chmod 600"), "{error}");
        assert!(!error.contains("gaia-test-loose-value-1"), "{error}");
    }

    #[test]
    fn an_optional_missing_source_resolves_to_nothing() {
        let dir = TempDir::new("gaia-secrets-optional");
        let missing = dir.join("absent");
        install(vec![file_def("gaia_test_absent", &missing, false)], "");
        assert_eq!(resolve("gaia_test_absent"), Ok(None));

        install(vec![file_def("gaia_test_absent_req", &missing, true)], "");
        let error = resolve("gaia_test_absent_req").expect_err("required");
        assert!(error.contains("gaia_test_absent_req"), "{error}");
    }

    #[test]
    fn an_undefined_secret_is_named() {
        let error = resolve("gaia_test_never_defined").expect_err("undefined");
        assert!(error.contains("gaia_test_never_defined"), "{error}");
    }

    #[test]
    fn a_command_secret_is_trimmed_and_its_failure_hides_stderr() {
        install(
            vec![SecretDef {
                name: "gaia_test_command".into(),
                source: SecretSource::Command(vec![
                    "sh".into(),
                    "-c".into(),
                    "printf 'gaia-test-command-9\\n'".into(),
                ]),
                required: true,
            }],
            "",
        );
        let value = resolve("gaia_test_command").expect("resolve").expect("set");
        assert_eq!(value.expose(), "gaia-test-command-9");

        install(
            vec![SecretDef {
                name: "gaia_test_command_fail".into(),
                source: SecretSource::Command(vec![
                    "sh".into(),
                    "-c".into(),
                    "echo 'leaked-stderr-text' >&2; exit 3".into(),
                ]),
                required: true,
            }],
            "",
        );
        let error = resolve("gaia_test_command_fail").expect_err("failed");
        assert!(error.contains("exit 3"), "{error}");
        assert!(!error.contains("leaked-stderr-text"), "{error}");
    }

    #[test]
    fn interpolated_references_are_replaced_at_spawn_and_name_the_missing_entry() {
        let dir = TempDir::new("gaia-secrets-expand");
        let path = write_secret(&dir, "key", "gaia-test-expand-key-42\n", 0o600);
        install(vec![file_def("gaia_test_expand", &path, true)], "");

        assert_eq!(
            expand_child_value("a-${secret:gaia_test_expand}-b").expect("expand"),
            "a-gaia-test-expand-key-42-b"
        );
        assert_eq!(expand_child_value("plain").expect("plain"), "plain");
        let error = expand_child_value("${secret:gaia_test_expand_missing}").expect_err("missing");
        assert!(error.contains("gaia_test_expand_missing"), "{error}");
    }

    #[test]
    fn key_settings_read_secrets_and_keep_literal_paths() {
        let dir = TempDir::new("gaia-secrets-key");
        let path = write_secret(&dir, "gpg", "release@example.org\n", 0o600);
        install(vec![file_def("gaia_test_gpg_id", &path, true)], "");

        assert_eq!(
            key_setting("secret:gaia_test_gpg_id"),
            Ok(Some("release@example.org".to_string()))
        );
        assert_eq!(
            key_setting("/keys/gaia.sec"),
            Ok(Some("/keys/gaia.sec".to_string()))
        );
        assert_eq!(key_setting("  "), Ok(None));
        let error = key_setting("env:GAIA_TEST_KEY_SETTING_UNSET_Q9").expect_err("unset");
        assert!(error.contains("GAIA_TEST_KEY_SETTING_UNSET_Q9"), "{error}");
    }

    #[test]
    fn a_credential_must_be_a_reference_never_a_literal() {
        let error = credential_setting("ghp_literal_value_abcdef").expect_err("literal");
        assert!(error.contains("secret:NAME or env:VAR"), "{error}");
        assert!(!error.contains("ghp_literal_value_abcdef"), "{error}");
    }

    #[test]
    fn file_paths_expand_home_and_take_relative_paths_from_the_base() {
        assert_eq!(
            file_path("keys/token", Path::new("/work")),
            PathBuf::from("/work/keys/token")
        );
        assert_eq!(
            file_path("/abs/token", Path::new("/work")),
            PathBuf::from("/abs/token")
        );
    }

    #[test]
    fn short_values_are_not_masked() {
        install(
            vec![SecretDef {
                name: "gaia_test_short".into(),
                source: SecretSource::Command(vec!["printf".into(), "ab".into()]),
                required: true,
            }],
            "",
        );
        assert_eq!(
            resolve("gaia_test_short")
                .expect("ok")
                .expect("set")
                .expose(),
            "ab"
        );
        assert_eq!(mask("ab cd").as_ref(), "ab cd");
    }
}
