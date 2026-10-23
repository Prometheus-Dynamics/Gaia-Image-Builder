//! The HTTP transport: one `curl` process per request.
//!
//! Gaia links no HTTP client, so each request runs the host's `curl`. Credentials
//! and headers go in a config file that only this user can read (`-K`), never on
//! the command line, where other users would see them in the process list. The
//! file is removed as soon as curl exits.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::RegistryError;
use super::scratch::Scratch;

/// Seconds allowed to connect before curl gives up.
const CONNECT_TIMEOUT_SECS: u64 = 30;
/// Seconds allowed for a metadata request (manifests, tags, tokens, signatures).
const SMALL_REQUEST_SECS: u64 = 60;
/// Seconds allowed for a blob transfer, which can be large.
pub(super) const BLOB_REQUEST_SECS: u64 = 600;
/// curl writes this after the body: a newline and the status code.
const STATUS_FORMAT: &str = "\n%{http_code}";

/// How a request authenticates. `Debug` hides the secret.
#[derive(Clone)]
pub(super) enum Auth {
    Anonymous,
    Basic { user: String, secret: String },
    Bearer(String),
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anonymous => f.write_str("Anonymous"),
            Self::Basic { user, .. } => write!(f, "Basic {{ user: {user:?}, secret: <hidden> }}"),
            Self::Bearer(_) => f.write_str("Bearer(<hidden>)"),
        }
    }
}

/// The request body: none, a file streamed by curl, or bytes piped to curl.
#[derive(Clone, Debug)]
pub(super) enum Body {
    Empty,
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// Where the response body goes: captured in memory, or written to a file.
#[derive(Clone, Debug)]
pub(super) enum Out {
    Memory,
    File(PathBuf),
}

/// One HTTP request, built with the methods below.
#[derive(Clone, Debug)]
pub(super) struct Request {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    pub(super) auth: Auth,
    body: Body,
    out: Out,
    follow_redirects: bool,
    max_time_secs: u64,
}

impl Request {
    pub(super) fn new(method: &str, url: &str) -> Self {
        Self {
            method: method.to_string(),
            url: url.to_string(),
            headers: Vec::new(),
            auth: Auth::Anonymous,
            body: Body::Empty,
            out: Out::Memory,
            follow_redirects: false,
            max_time_secs: SMALL_REQUEST_SECS,
        }
    }

    pub(super) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub(super) fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    pub(super) fn out(mut self, out: Out) -> Self {
        self.out = out;
        self
    }

    /// Follows redirects. Only blob downloads use it: registries send those to
    /// storage hosts, and curl drops the Authorization header on a host change.
    pub(super) fn follow_redirects(mut self) -> Self {
        self.follow_redirects = true;
        self
    }

    pub(super) fn max_time(mut self, secs: u64) -> Self {
        self.max_time_secs = secs;
        self
    }

    /// The method and URL path, for error messages. No host, query or secret.
    fn describe(&self) -> String {
        let path = self
            .url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|at| &rest[at..]))
            .unwrap_or("");
        format!("{} {path}", self.method)
    }
}

/// A finished response. Header names are lowercase.
#[derive(Debug)]
pub(super) struct Response {
    pub(super) status: u16,
    headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
}

impl Response {
    /// The first value of `name` (lowercase), if the response sent one.
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// `https://` for a registry, `http://` for a loopback one (the test registry).
pub(super) fn base_url(registry: &str) -> String {
    let host = if registry.starts_with('[') {
        registry.find(']').map_or(registry, |end| &registry[..=end])
    } else {
        registry.split(':').next().unwrap_or(registry)
    };
    let scheme = if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{registry}")
}

/// Runs one request through curl and returns what it answered.
pub(super) fn send(request: &Request) -> Result<Response, RegistryError> {
    let dir = std::env::temp_dir();
    let config = Scratch::create(&dir, "curl.cfg", curl_config(request)?.as_bytes())?;
    let header_dump = Scratch::create(&dir, "curl.hdr", b"")?;
    let mut command = Command::new("curl");
    command
        .args(curl_args(request, config.path(), header_dump.path()))
        .stdin(match request.body {
            Body::Bytes(_) => Stdio::piped(),
            _ => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        RegistryError::Transport(format!(
            "cannot run curl for {}: {error}",
            request.describe()
        ))
    })?;
    if let (Body::Bytes(bytes), Some(mut stdin)) = (&request.body, child.stdin.take()) {
        // A write error means curl stopped reading; its exit status says why.
        let _ = stdin.write_all(bytes);
    }
    let output = child.wait_with_output().map_err(|error| {
        RegistryError::Transport(format!(
            "curl did not finish for {}: {error}",
            request.describe()
        ))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().next().unwrap_or("no message").trim();
        return Err(RegistryError::Transport(format!(
            "curl failed for {}: {first}",
            request.describe()
        )));
    }
    let headers = parse_headers(
        &fs::read(header_dump.path())
            .map_err(|error| RegistryError::Io(format!("cannot read curl headers: {error}")))?,
    );
    let (body, status) = split_status(&output.stdout, &request.describe())?;
    Ok(Response {
        status,
        headers,
        body,
    })
}

/// The curl config file: the credential, then the headers. `Expect:` is turned
/// off because some registries answer it badly.
pub(super) fn curl_config(request: &Request) -> Result<String, RegistryError> {
    let mut lines = Vec::new();
    match &request.auth {
        Auth::Anonymous => {}
        Auth::Basic { user, secret } => {
            lines.push(format!("user = {}", quote(&format!("{user}:{secret}"))?));
        }
        Auth::Bearer(token) => {
            lines.push(format!(
                "header = {}",
                quote(&format!("Authorization: Bearer {token}"))?
            ));
        }
    }
    lines.push("header = \"Expect:\"".to_string());
    for (name, value) in &request.headers {
        lines.push(format!("header = {}", quote(&format!("{name}: {value}"))?));
    }
    Ok(lines.join("\n") + "\n")
}

/// Quotes a curl config value, escaping backslashes and quotes. A line break
/// would end the value early, so it is refused. The error names no value.
fn quote(value: &str) -> Result<String, RegistryError> {
    if value.contains(['\n', '\r']) {
        return Err(RegistryError::Protocol(
            "a request header contains a line break".into(),
        ));
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}

/// The curl command line. It names the config file and the header dump, never
/// a credential.
pub(super) fn curl_args(request: &Request, config: &Path, header_dump: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-q", "-sS", "--connect-timeout"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.push(CONNECT_TIMEOUT_SECS.to_string().into());
    args.push("--max-time".into());
    args.push(request.max_time_secs.to_string().into());
    args.extend(["-K".into(), config.into()]);
    args.extend(["-D".into(), header_dump.into()]);
    args.extend(["-w".into(), STATUS_FORMAT.into()]);
    if request.method == "HEAD" {
        args.push("-I".into());
    } else {
        args.extend(["-X".into(), request.method.clone().into()]);
    }
    if request.follow_redirects {
        args.push("-L".into());
    }
    match &request.body {
        Body::Empty => {}
        Body::File(path) => {
            let mut arg = OsString::from("@");
            arg.push(path);
            args.extend(["--data-binary".into(), arg]);
        }
        Body::Bytes(_) => args.extend(["--data-binary".into(), "@-".into()]),
    }
    match &request.out {
        Out::Memory => args.extend(["-o".into(), "-".into()]),
        Out::File(path) => args.extend(["-o".into(), path.into()]),
    }
    args.push(request.url.clone().into());
    args
}

/// Splits curl's stdout into the body and the status code that ends it.
fn split_status(stdout: &[u8], what: &str) -> Result<(Vec<u8>, u16), RegistryError> {
    let cut = stdout
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(|| RegistryError::Protocol(format!("curl gave no status for {what}")))?;
    let status = std::str::from_utf8(&stdout[cut + 1..])
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .ok_or_else(|| RegistryError::Protocol(format!("curl gave no status for {what}")))?;
    Ok((stdout[..cut].to_vec(), status))
}

/// The headers of the last response in curl's dump. Redirects and `100
/// Continue` answers come first and are dropped.
fn parse_headers(raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw);
    let mut headers = Vec::new();
    for line in text.lines() {
        if line.starts_with("HTTP/") {
            headers.clear();
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    headers
}
