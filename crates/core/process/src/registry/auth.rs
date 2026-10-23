//! Where registry credentials come from, and how a bearer challenge is read.
//!
//! Resolution order: `GAIA_REGISTRY_TOKEN` (user from `GAIA_REGISTRY_USER`), the
//! docker config (`auths`, `credHelpers`, `credsStore`), `gh auth token` for
//! `ghcr.io`, then anonymous. Secrets are never printed or logged.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

/// Environment variable holding a registry token.
const TOKEN_ENV: &str = "GAIA_REGISTRY_TOKEN";
/// Environment variable holding the user name paired with the token.
const USER_ENV: &str = "GAIA_REGISTRY_USER";
/// User name used when none is given, and for a token from `gh`. Registries
/// that take personal access tokens accept any user name with them.
const DEFAULT_USER: &str = "gaia";
/// The registry `gh auth token` is consulted for.
const GHCR_HOST: &str = "ghcr.io";

/// The credentials a registry request can carry. `Debug` hides the secret.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Credentials {
    Anonymous,
    Basic { username: String, secret: String },
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anonymous => f.write_str("Anonymous"),
            Self::Basic { username, .. } => {
                write!(f, "Basic {{ username: {username:?}, secret: <hidden> }}")
            }
        }
    }
}

/// The challenge a registry sent with a 401.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Challenge {
    /// A token service to ask for a bearer token for `scope`.
    Bearer {
        realm: String,
        service: String,
        scope: String,
    },
    Basic,
    Other,
}

/// Resolves the credentials for `registry`, falling back to anonymous.
pub(super) fn resolve(registry: &str) -> Credentials {
    env_credentials()
        .or_else(|| docker_credentials(registry))
        .or_else(|| gh_credentials(registry))
        .unwrap_or(Credentials::Anonymous)
}

fn env_credentials() -> Option<Credentials> {
    let secret = std::env::var(TOKEN_ENV).ok()?.trim().to_string();
    if secret.is_empty() {
        return None;
    }
    let username = std::env::var(USER_ENV)
        .ok()
        .filter(|user| !user.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_USER.to_string());
    Some(Credentials::Basic { username, secret })
}

fn docker_credentials(registry: &str) -> Option<Credentials> {
    let config = read_docker_config()?;
    if let Some(helper) = config
        .get("credHelpers")
        .and_then(|helpers| helpers.get(registry))
        .and_then(Value::as_str)
    {
        return helper_credentials(helper, registry);
    }
    if let Some(credentials) = config_auth(&config, registry) {
        return Some(credentials);
    }
    let store = config.get("credsStore").and_then(Value::as_str)?;
    helper_credentials(store, registry)
}

/// The config file: `$DOCKER_CONFIG/config.json`, or `~/.docker/config.json`.
fn read_docker_config() -> Option<Value> {
    let dir = match std::env::var_os("DOCKER_CONFIG") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".docker"),
    };
    let bytes = fs::read(dir.join("config.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The `auth` entry (base64 `user:password`) stored for `registry`.
fn config_auth(config: &Value, registry: &str) -> Option<Credentials> {
    let auths = config.get("auths")?.as_object()?;
    let (_, entry) = auths
        .iter()
        .find(|(key, _)| normalize_registry_key(key) == registry)?;
    decode_basic(entry.get("auth")?.as_str()?)
}

/// Docker keys `auths` by URL (`https://ghcr.io/`); the host is what is compared.
fn normalize_registry_key(key: &str) -> &str {
    key.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
}

/// Runs `docker-credential-<helper> get` with the registry URL on stdin, as
/// docker does. Any failure means no credentials from this helper.
fn helper_credentials(helper: &str, registry: &str) -> Option<Credentials> {
    let valid = !helper.is_empty()
        && helper
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return None;
    }
    let mut child = Command::new(format!("docker-credential-{helper}"))
        .arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()?
        .write_all(format!("https://{registry}").as_bytes())
        .ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    let username = value.get("Username")?.as_str()?;
    let secret = value.get("Secret")?.as_str()?;
    if username.is_empty() || secret.is_empty() {
        return None;
    }
    Some(Credentials::Basic {
        username: username.to_string(),
        secret: secret.to_string(),
    })
}

/// `gh auth token`, for `ghcr.io` only.
fn gh_credentials(registry: &str) -> Option<Credentials> {
    if registry.split(':').next() != Some(GHCR_HOST) {
        return None;
    }
    let output = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let secret = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if !output.status.success() || secret.is_empty() {
        return None;
    }
    Some(Credentials::Basic {
        username: DEFAULT_USER.to_string(),
        secret,
    })
}

/// Decodes a standard base64 `user:password` pair. `None` when it is not one.
pub(super) fn decode_basic(encoded: &str) -> Option<Credentials> {
    let bytes = base64_decode(encoded.trim())?;
    let text = String::from_utf8(bytes).ok()?;
    let (username, secret) = text.split_once(':')?;
    if username.is_empty() || secret.is_empty() {
        return None;
    }
    Some(Credentials::Basic {
        username: username.to_string(),
        secret: secret.to_string(),
    })
}

/// Standard base64, padding optional. `None` on a character outside the alphabet.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Reads a `WWW-Authenticate` value. Only `Bearer` and `Basic` are understood.
pub(super) fn parse_challenge(header: &str) -> Challenge {
    let header = header.trim();
    let (scheme, rest) = header.split_once(' ').unwrap_or((header, ""));
    if scheme.eq_ignore_ascii_case("basic") {
        return Challenge::Basic;
    }
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Challenge::Other;
    }
    let mut params = parse_params(rest);
    Challenge::Bearer {
        realm: params.remove("realm").unwrap_or_default(),
        service: params.remove("service").unwrap_or_default(),
        scope: params.remove("scope").unwrap_or_default(),
    }
}

/// Parses `key="value", key=value` pairs. Quoted values may hold commas, as
/// scopes like `pull,push` do, so a plain split on commas would be wrong.
fn parse_params(text: &str) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    let mut rest = text.trim_start();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq]
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        let after = rest[eq + 1..].trim_start();
        let (value, remainder) = if let Some(quoted) = after.strip_prefix('"') {
            match quoted.find('"') {
                Some(end) => (&quoted[..end], &quoted[end + 1..]),
                None => (quoted, ""),
            }
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (after[..end].trim(), &after[end..])
        };
        params.insert(key, value.to_string());
        rest = remainder.trim_start().trim_start_matches(',').trim_start();
    }
    params
}
