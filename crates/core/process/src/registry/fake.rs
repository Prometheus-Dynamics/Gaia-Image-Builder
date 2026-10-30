//! A fake OCI registry on a loopback socket, for the registry tests.
//!
//! It speaks the subset of the distribution API the client uses: manifests by
//! tag and digest, monolithic blob uploads, tag listing, digest deletion, and a
//! bearer token challenge. Each connection carries one request with a
//! Content-Length body and is closed after the response.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use serde_json::json;

use crate::sha256_hex_bytes;

/// The bearer token the fake's token service issues.
pub(crate) const BEARER: &str = "fake-bearer-token";
const REALM_PATH: &str = "/token";

/// Everything the fake has seen and stores. Tests read and edit it directly.
#[derive(Default)]
pub struct State {
    /// Basic credentials (user, password) the token service accepts. `None`
    /// means the registry needs no authentication at all.
    pub auth: Option<(String, String)>,
    /// Every request as "METHOD path" (no query), in arrival order.
    pub requests: Vec<String>,
    /// The Authorization header of each API request ("" when absent).
    pub api_authorization: Vec<String>,
    /// manifests[image][reference] = bytes; references are tags and digests.
    pub manifests: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    /// blobs[digest] = bytes.
    pub blobs: BTreeMap<String, Vec<u8>>,
    uploads: u64,
}

/// A running fake registry. Its thread lives until the test process exits.
pub struct FakeRegistry {
    addr: String,
    state: Arc<Mutex<State>>,
}

impl FakeRegistry {
    /// Starts a registry. `auth` is the (user, password) that the token
    /// service accepts; with `Some`, every API request needs the bearer token.
    pub fn start(auth: Option<(&str, &str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake registry");
        let addr = listener
            .local_addr()
            .expect("fake registry address")
            .to_string();
        let state = Arc::new(Mutex::new(State {
            auth: auth.map(|(user, pass)| (user.to_string(), pass.to_string())),
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        let host = addr.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&shared);
                let host = host.clone();
                thread::spawn(move || serve(stream, &shared, &host));
            }
        });
        Self { addr, state }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// The base64 standard encoding, with padding. Used to build the expected
/// Basic header.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[((n >> (18 - 6 * index)) & 63) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

struct Request {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
        }
    }

    fn json(status: u16, value: &serde_json::Value) -> Self {
        Self::new(status, serde_json::to_vec(value).unwrap_or_default())
    }

    fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>, host: &str) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let Some(request) = read_request(&mut reader) else {
        return;
    };
    let reply = {
        let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
        route(&request, &mut guard, host)
    };
    let mut out = stream;
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    let _ = out.write_all(head.as_bytes());
    if request.method != "HEAD" {
        let _ = out.write_all(&reply.body);
    }
}

fn read_request(reader: &mut impl BufRead) -> Option<Request> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = BTreeMap::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        target,
        headers,
        body,
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    }
}

fn route(request: &Request, state: &mut State, host: &str) -> Reply {
    let (path, query) = request
        .target
        .split_once('?')
        .unwrap_or((&request.target, ""));
    state.requests.push(format!("{} {path}", request.method));
    if path == REALM_PATH {
        return token_service(request, state);
    }
    let Some(v2) = path.strip_prefix("/v2") else {
        return Reply::new(404, b"{}".to_vec());
    };
    let v2 = v2.trim_start_matches('/');
    if v2.is_empty() {
        return if authorized(request, state) {
            Reply::new(200, b"{}".to_vec())
        } else {
            challenge(host, None)
        };
    }
    let Some((name, operation)) = split_operation(v2) else {
        return Reply::new(404, b"{}".to_vec());
    };
    if !authorized(request, state) {
        return challenge(host, Some(name));
    }
    state.api_authorization.push(
        request
            .headers
            .get("authorization")
            .cloned()
            .unwrap_or_default(),
    );
    match operation {
        Operation::Manifest(reference) => manifest(request, state, name, reference),
        Operation::UploadStart => upload_start(state, name),
        Operation::UploadPut => upload_put(request, state, query),
        Operation::Blob(digest) => blob(request, state, digest),
        Operation::Tags => tags(state, name),
    }
}

enum Operation<'a> {
    Manifest(&'a str),
    UploadStart,
    UploadPut,
    Blob(&'a str),
    Tags,
}

/// Splits `<name>/<operation>` out of the path after `/v2/`.
fn split_operation(v2: &str) -> Option<(&str, Operation<'_>)> {
    if let Some(at) = v2.find("/manifests/") {
        return Some((&v2[..at], Operation::Manifest(&v2[at + 11..])));
    }
    if let Some(at) = v2.find("/blobs/uploads/") {
        let tail = &v2[at + 15..];
        let operation = if tail.is_empty() {
            Operation::UploadStart
        } else {
            Operation::UploadPut
        };
        return Some((&v2[..at], operation));
    }
    if let Some(at) = v2.find("/blobs/") {
        return Some((&v2[..at], Operation::Blob(&v2[at + 7..])));
    }
    v2.strip_suffix("/tags/list")
        .map(|name| (name, Operation::Tags))
}

/// Whether the request may use the API: always, unless the registry needs auth.
fn authorized(request: &Request, state: &State) -> bool {
    if state.auth.is_none() {
        return true;
    }
    let expected = format!("Bearer {BEARER}");
    request.headers.get("authorization").map(String::as_str) == Some(expected.as_str())
}

fn challenge(host: &str, name: Option<&str>) -> Reply {
    let scope = name.map_or(String::new(), |name| {
        format!(",scope=\"repository:{name}:pull,push\"")
    });
    Reply::new(401, b"{}".to_vec()).header(
        "WWW-Authenticate",
        format!("Bearer realm=\"http://{host}{REALM_PATH}\",service=\"fake-registry\"{scope}"),
    )
}

/// Issues the bearer token when the basic credentials match.
fn token_service(request: &Request, state: &State) -> Reply {
    let accepted = match &state.auth {
        None => true,
        Some((user, pass)) => {
            let expected = format!("Basic {}", base64(format!("{user}:{pass}").as_bytes()));
            request.headers.get("authorization") == Some(&expected)
        }
    };
    if accepted {
        Reply::json(200, &json!({ "token": BEARER }))
    } else {
        Reply::new(401, b"{}".to_vec())
    }
}

fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex_bytes(bytes))
}

fn manifest(request: &Request, state: &mut State, name: &str, reference: &str) -> Reply {
    match request.method.as_str() {
        "PUT" => {
            let digest = digest_of(&request.body);
            let refs = state.manifests.entry(name.to_string()).or_default();
            refs.insert(reference.to_string(), request.body.clone());
            refs.insert(digest.clone(), request.body.clone());
            Reply::new(201, Vec::new()).header("Docker-Content-Digest", digest)
        }
        "DELETE" => {
            let Some(refs) = state.manifests.get_mut(name) else {
                return Reply::new(404, b"{}".to_vec());
            };
            let before = refs.len();
            refs.retain(|_, bytes| digest_of(bytes) != reference);
            if refs.len() == before {
                Reply::new(404, b"{}".to_vec())
            } else {
                Reply::new(202, Vec::new())
            }
        }
        _ => match state
            .manifests
            .get(name)
            .and_then(|refs| refs.get(reference))
        {
            Some(bytes) => Reply::new(200, bytes.clone())
                .header("Content-Type", "application/vnd.oci.image.manifest.v1+json")
                .header("Docker-Content-Digest", digest_of(bytes)),
            None => Reply::new(404, b"{}".to_vec()),
        },
    }
}

fn upload_start(state: &mut State, name: &str) -> Reply {
    state.uploads += 1;
    let location = format!("/v2/{name}/blobs/uploads/{}", state.uploads);
    Reply::new(202, Vec::new()).header("Location", location)
}

fn upload_put(request: &Request, state: &mut State, query: &str) -> Reply {
    let Some(digest) = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("digest="))
    else {
        return Reply::new(400, b"{}".to_vec());
    };
    if digest_of(&request.body) != digest {
        return Reply::new(400, b"{}".to_vec());
    }
    state.blobs.insert(digest.to_string(), request.body.clone());
    Reply::new(201, Vec::new()).header("Docker-Content-Digest", digest)
}

fn blob(request: &Request, state: &State, digest: &str) -> Reply {
    let _ = request;
    match state.blobs.get(digest) {
        Some(bytes) => Reply::new(200, bytes.clone()).header("Docker-Content-Digest", digest),
        None => Reply::new(404, b"{}".to_vec()),
    }
}

fn tags(state: &State, name: &str) -> Reply {
    let Some(refs) = state.manifests.get(name) else {
        return Reply::new(404, b"{}".to_vec());
    };
    let tags: Vec<&String> = refs
        .keys()
        .filter(|reference| !reference.starts_with("sha256:"))
        .collect();
    Reply::json(200, &json!({ "name": name, "tags": tags }))
}
