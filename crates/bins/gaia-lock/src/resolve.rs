//! Resolving one source: a git ref to its commit, a download to the digest
//! of its bytes, an image tag to its manifest digest, a directory to its
//! tree digest. Each asks the network (or the disk) once and records
//! nothing else.

use gaia_lock::{Entry, Want};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The programs resolving uses.
#[derive(Debug, Clone)]
pub struct Tools {
    pub git: PathBuf,
    pub curl: PathBuf,
}

/// What `want` resolves to now. `base` is the directory a tree's path is
/// recorded relative to.
pub fn resolve(want: &Want, base: &Path, tools: &Tools, scratch: &Path) -> Result<Entry, String> {
    match want {
        Want::Git { url, reference } => Ok(Entry::Git {
            url: url.clone(),
            reference: reference.clone(),
            commit: git_commit(&tools.git, url, reference)?,
        }),
        Want::Download { url } => Ok(Entry::Download {
            url: url.clone(),
            digest: download_digest(&tools.curl, url, scratch)?,
        }),
        Want::Image { reference } => Ok(Entry::Image {
            reference: reference.clone(),
            digest: image_digest(&tools.curl, reference, scratch)?,
        }),
        Want::Tree { path } => Ok(Entry::Tree {
            path: relative(Path::new(path), base),
            digest: gaia_store::tree_digest(Path::new(path))
                .map_err(|e| format!("{path}: {e}"))?
                .to_string(),
        }),
    }
}

/// `path` relative to `base`, with `..` for each directory of `base` it is
/// not under: `components/app` beside `products/demo` is
/// `../../components/app`. So a lock holds no path of the checkout it was
/// written in, wherever the product's files sit. Only paths with no common
/// root (another Windows drive) stay absolute.
pub fn relative(path: &Path, base: &Path) -> String {
    use std::path::Component;
    let path = normal(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    let base = normal(&std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf()));
    let (from, to): (Vec<_>, Vec<_>) = (base.components().collect(), path.components().collect());
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if shared == 0 || !matches!(from[shared - 1], Component::RootDir | Component::Normal(_)) {
        return path.display().to_string();
    }
    let mut out = std::path::PathBuf::new();
    for _ in shared..from.len() {
        out.push("..");
    }
    for part in &to[shared..] {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out.display().to_string()
}

/// `path` without `.` and `name/..` pairs.
fn normal(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn is_hex_commit(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The commit `reference` names at `url`: an annotated tag's commit, then a
/// tag, a branch, or a ref named in full. A commit names itself.
fn git_commit(git: &Path, url: &str, reference: &str) -> Result<String, String> {
    if is_hex_commit(reference) {
        return Ok(reference.to_ascii_lowercase());
    }
    let out = Command::new(git)
        .args(["ls-remote", "--", url])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("{}: {e}", git.display()))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-remote {}: {}",
            gaia_lock::redact(url),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let refs: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(commit, name)| (name, commit))
        .collect();
    let wanted = [
        format!("refs/tags/{reference}^{{}}"),
        format!("refs/tags/{reference}"),
        format!("refs/heads/{reference}"),
        reference.to_string(),
    ];
    wanted
        .iter()
        .find_map(|name| {
            refs.iter()
                .find(|(n, _)| n == name)
                .map(|(_, c)| c.to_string())
        })
        .ok_or_else(|| format!("{}: no ref `{reference}`", gaia_lock::redact(url)))
}

/// The digest of the bytes at `url`.
fn download_digest(curl: &Path, url: &str, scratch: &Path) -> Result<String, String> {
    let file = scratch.join("download");
    let out = Command::new(curl)
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&file)
        .arg(url)
        .output()
        .map_err(|e| format!("{}: {e}", curl.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{}: {}",
            gaia_lock::redact(url),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let digest = gaia_store::file_digest(&file).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&file);
    Ok(digest.to_string())
}

const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";

/// The manifest digest `reference`'s tag names now, asked anonymously. A
/// reference with `@sha256:...` names its own digest.
fn image_digest(curl: &Path, reference: &str, scratch: &Path) -> Result<String, String> {
    let parsed = gaia_source_oci::Reference::parse(reference)?;
    if let Some(digest) = parsed.digest {
        return Ok(digest);
    }
    let last = reference.rsplit('/').next().unwrap_or(reference);
    let tag = last.split_once(':').map_or("latest", |(_, tag)| tag);
    let url = format!("{}/v2/{}/manifests/{tag}", parsed.base, parsed.repository);
    let body = scratch.join("manifest");
    let mut answer = get(curl, &url, &[format!("Accept: {ACCEPT}")], &body)?;
    if answer.status == 401 {
        let challenge = answer.header("www-authenticate").unwrap_or_default();
        let token = token(curl, &challenge, &parsed.repository, scratch)
            .map_err(|e| format!("{reference}: {e}"))?;
        answer = get(
            curl,
            &url,
            &[
                format!("Accept: {ACCEPT}"),
                format!("Authorization: Bearer {token}"),
            ],
            &body,
        )?;
    }
    if answer.status != 200 {
        return Err(format!("{url}: HTTP {}", answer.status));
    }
    // The digest of the manifest's bytes is the digest; a registry's
    // header must agree with it.
    let digest = gaia_store::file_digest(&body)
        .map_err(|e| e.to_string())?
        .to_string();
    let _ = std::fs::remove_file(&body);
    if let Some(header) = answer.header("docker-content-digest")
        && header != digest
    {
        return Err(format!(
            "{url}: the registry says {header}, the bytes hash to {digest}"
        ));
    }
    Ok(digest)
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }
}

/// GETs `url` into `out` with `headers`, without failing on a status.
fn get(curl: &Path, url: &str, headers: &[String], out: &Path) -> Result<Answer, String> {
    let mut command = Command::new(curl);
    command.args(["-sSL", "-D", "-", "-o"]).arg(out);
    for header in headers {
        command.arg("-H").arg(header);
    }
    let run = command
        .arg(url)
        .output()
        .map_err(|e| format!("{}: {e}", curl.display()))?;
    if !run.status.success() {
        return Err(format!(
            "{url}: {}",
            String::from_utf8_lossy(&run.stderr).trim()
        ));
    }
    let dump = String::from_utf8_lossy(&run.stdout);
    // Redirects give several header blocks; the last is the answer.
    let block = dump
        .split("\r\n\r\n")
        .filter(|b| b.starts_with("HTTP/"))
        .last()
        .unwrap_or("");
    let mut lines = block.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("{url}: no HTTP status"))?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Ok(Answer { status, headers })
}

/// An anonymous pull token for a bearer challenge.
fn token(curl: &Path, challenge: &str, repository: &str, scratch: &Path) -> Result<String, String> {
    let (scheme, rest) = challenge.split_once(' ').unwrap_or((challenge, ""));
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(format!(
            "the registry asks for `{scheme}` credentials; gaia lock asks anonymously"
        ));
    }
    let param = |key: &str| {
        rest.split(',').find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            k.eq_ignore_ascii_case(key)
                .then(|| v.trim_matches('"').to_string())
        })
    };
    let realm = param("realm").ok_or("a bearer challenge with no realm")?;
    let mut query = Vec::new();
    if let Some(service) = param("service") {
        query.push(format!("service={service}"));
    }
    query.push(format!("scope=repository:{repository}:pull"));
    let body = scratch.join("token");
    let answer = get(curl, &format!("{realm}?{}", query.join("&")), &[], &body)?;
    let text = std::fs::read(&body).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&body);
    if answer.status != 200 {
        return Err(format!("no anonymous token: HTTP {}", answer.status));
    }
    let json: serde_json::Value = serde_json::from_slice(&text).map_err(|e| e.to_string())?;
    json.get("token")
        .or_else(|| json.get("access_token"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "no token in the answer".into())
}

#[cfg(test)]
mod relative_tests {
    use super::relative;
    use std::path::Path;

    #[test]
    fn under_beside_and_the_same() {
        let base = Path::new("/w/devices/products/p");
        for (path, expected) in [
            ("/w/devices/products/p/files", "files"),
            ("/w/devices/products/p/a/b", "a/b"),
            ("/w/devices/components/c/files", "../../components/c/files"),
            ("/w/devices/products/p", "."),
            ("/w/devices/products/p/x/../y", "y"),
            ("/elsewhere/tree", "../../../../elsewhere/tree"),
        ] {
            assert_eq!(relative(Path::new(path), base), expected, "{path}");
        }
    }
}
