//! Talking to a registry with the `curl` program: GET with the image's
//! accept types, following a bearer or basic challenge once.
//!
//! A secret (a password, a token) never goes on curl's command line, which
//! the run log records: it goes in a config file curl reads, made in the
//! step's directory with mode 0600 and removed when the request ends.

use crate::NAME;
use gaia_step::{StepError, Work};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// A registry session: the curl program, the credentials declared, and the
/// authorization the registry gave.
pub(crate) struct Registry {
    pub curl: PathBuf,
    pub base: String,
    pub repository: String,
    /// `user:password`, when credentials are declared.
    pub credentials: Option<String>,
    /// The `Authorization` header value once a challenge was answered.
    authorization: Option<String>,
}

/// What a request answered.
pub(crate) struct Answer {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
}

impl Registry {
    pub(crate) fn new(
        curl: &str,
        base: String,
        repository: String,
        credentials: Option<String>,
    ) -> Self {
        Self {
            curl: PathBuf::from(curl),
            base,
            repository,
            credentials,
            authorization: None,
        }
    }

    /// GETs `/v2/<repository>/<what>` into `out`, answering one auth
    /// challenge. Anything but 200 is an error naming the URL.
    pub(crate) fn get(
        &mut self,
        work: &mut Work<'_>,
        what: &str,
        accept: &[&str],
        out: &Path,
    ) -> Result<Answer, StepError> {
        let url = format!("{}/v2/{}/{what}", self.base, self.repository);
        let mut answer = self.send(work, &url, accept, self.authorization.clone(), None, out)?;
        if answer.status == 401 && self.authorization.is_none() {
            let challenge = answer
                .headers
                .get("www-authenticate")
                .cloned()
                .unwrap_or_default();
            self.authorization = Some(self.answer_challenge(work, &url, &challenge)?);
            answer = self.send(work, &url, accept, self.authorization.clone(), None, out)?;
        }
        if answer.status != 200 {
            return Err(StepError::body(
                NAME,
                format!("{url}: HTTP {}", answer.status),
            ));
        }
        Ok(answer)
    }

    /// The `Authorization` value that answers `challenge`.
    fn answer_challenge(
        &self,
        work: &mut Work<'_>,
        url: &str,
        challenge: &str,
    ) -> Result<String, StepError> {
        let (scheme, params) = parse_challenge(challenge);
        match scheme.as_str() {
            "bearer" => {
                let realm = params.get("realm").ok_or_else(|| {
                    StepError::body(NAME, format!("{url}: a bearer challenge with no realm"))
                })?;
                let mut query = Vec::new();
                if let Some(service) = params.get("service") {
                    query.push(format!("service={}", encode(service)));
                }
                let scope = params
                    .get("scope")
                    .cloned()
                    .unwrap_or_else(|| format!("repository:{}:pull", self.repository));
                query.push(format!("scope={}", encode(&scope)));
                let token_url = format!("{realm}?{}", query.join("&"));
                let out = work.dir().join("token.json");
                let answer = self.send(
                    work,
                    &token_url,
                    &[],
                    None,
                    self.credentials.as_deref(),
                    &out,
                )?;
                if answer.status != 200 {
                    let who = if self.credentials.is_some() {
                        "with the credentials declared"
                    } else {
                        "anonymously"
                    };
                    return Err(StepError::body(
                        NAME,
                        format!("{url}: no token {who}: HTTP {}", answer.status),
                    ));
                }
                let body: serde_json::Value =
                    serde_json::from_slice(&fs::read(&out).map_err(|e| StepError::body(NAME, e))?)
                        .map_err(|e| StepError::body(NAME, format!("{token_url}: {e}")))?;
                let _ = fs::remove_file(&out);
                let token = body
                    .get("token")
                    .or_else(|| body.get("access_token"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        StepError::body(NAME, format!("{token_url}: no token in the answer"))
                    })?;
                Ok(format!("Bearer {token}"))
            }
            "basic" => match &self.credentials {
                Some(credentials) => Ok(format!("Basic {}", base64(credentials.as_bytes()))),
                None => Err(StepError::body(
                    NAME,
                    format!("{url}: the registry wants credentials and none are declared"),
                )),
            },
            _ => Err(StepError::body(
                NAME,
                format!("{url}: HTTP 401 with no challenge Gaia answers"),
            )),
        }
    }

    /// One request through curl. `authorization` goes as a header and
    /// `user` as basic credentials, both through a config file.
    fn send(
        &self,
        work: &mut Work<'_>,
        url: &str,
        accept: &[&str],
        authorization: Option<String>,
        user: Option<&str>,
        out: &Path,
    ) -> Result<Answer, StepError> {
        let dir = work.dir().to_path_buf();
        let headers_file = dir.join("curl.headers");
        let config = dir.join("curl.config");
        let mut text = String::new();
        for accept in accept {
            text.push_str(&format!("header = \"Accept: {accept}\"\n"));
        }
        if let Some(authorization) = &authorization {
            text.push_str(&format!(
                "header = \"Authorization: {}\"\n",
                quote(authorization)
            ));
        }
        if let Some(user) = user {
            text.push_str(&format!("user = \"{}\"\n", quote(user)));
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&config)
            .map_err(|e| StepError::body(NAME, e))?;
        file.write_all(text.as_bytes())
            .map_err(|e| StepError::body(NAME, e))?;
        drop(file);
        let tool = gaia_step::Tool::new(&self.curl)
            .args(["--silent", "--show-error", "--location", "--config"])
            .arg(&config)
            .arg("--dump-header")
            .arg(&headers_file)
            .arg("--output")
            .arg(out)
            .args(["--write-out", "%{http_code}\\n"])
            .arg(url)
            .env("PATH", "/usr/bin:/bin");
        let result = work.run(&tool);
        let _ = fs::remove_file(&config);
        let output = result.map_err(|error| {
            let first = error
                .to_string()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            StepError::body(NAME, format!("{url}: {first}"))
        })?;
        let status = output
            .stdout_tail
            .last()
            .and_then(|line| line.trim().parse().ok())
            .ok_or_else(|| StepError::body(NAME, format!("{url}: curl gave no status")))?;
        let dump = fs::read_to_string(&headers_file).unwrap_or_default();
        let _ = fs::remove_file(&headers_file);
        Ok(Answer {
            status,
            headers: last_headers(&dump),
        })
    }
}

/// The headers of the last response in a `--dump-header` file (redirects
/// put one block per hop), names lower-cased.
fn last_headers(dump: &str) -> BTreeMap<String, String> {
    let block = dump
        .split("\r\n\r\n")
        .filter(|block| !block.trim().is_empty())
        .last()
        .unwrap_or_default();
    block
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect()
}

/// `Bearer realm="...",service="..."`: the scheme, lower-cased, and the
/// parameters. A quoted value may hold commas.
pub(crate) fn parse_challenge(header: &str) -> (String, BTreeMap<String, String>) {
    let (scheme, rest) = header.trim().split_once(' ').unwrap_or((header.trim(), ""));
    let mut params = BTreeMap::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
            chars.next();
        }
        let key: String = chars.by_ref().take_while(|c| *c != '=').collect();
        if key.is_empty() {
            break;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    c => value.push(c),
                }
            }
        } else {
            value = chars.by_ref().take_while(|c| *c != ',').collect();
        }
        params.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    (scheme.to_ascii_lowercase(), params)
}

/// A value inside a double-quoted curl config string.
fn quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_reads_with_commas_in_quotes() {
        let (scheme, params) = parse_challenge(
            r#"Bearer realm="https://auth.example/token",service="registry.example",scope="repository:a/b:pull,push""#,
        );
        assert_eq!(scheme, "bearer");
        assert_eq!(params["realm"], "https://auth.example/token");
        assert_eq!(params["scope"], "repository:a/b:pull,push");
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
    }
}
