//! Client for the OCI registry that holds Gaia's shared cache.
//!
//! An entry lives at the image `<repository>/<kind>`, tagged with its key. Its
//! manifest has one layer (the entry's tar.zst, media type [`ARTIFACT_TYPE`]) and
//! the empty `{}` config. Its signature is a second manifest tagged `<key>.sig`
//! whose layer holds the signing tool's output over the exact bytes of the entry
//! manifest. A pull verifies that signature before it downloads the blob, and
//! checks the blob's sha256 against the manifest digest after.
//!
//! HTTP runs through the host's `curl`, archives through `tar` and `zstd`, and
//! signatures through the host's signing tool. No HTTP or compression crate is
//! linked, so the client adds no dependency beyond `serde_json`.

mod archive;
mod auth;
/// A fake registry for tests of registry users (`test-support` feature).
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
mod http;
mod manifest;
mod scratch;
mod signing;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde_json::Value;

pub use archive::{pack_entry, unpack_entry};
use auth::Credentials;
use http::{Auth, BLOB_REQUEST_SECS, Body, Out, Request, Response};
use manifest::Descriptor;
pub use manifest::created_epoch_seconds;
use scratch::Scratch;

/// Artifact type of an entry's layer.
pub const ARTIFACT_TYPE: &str = "application/vnd.gaia.cache.v1+tar";
/// Media type of the empty config blob every entry and signature carries.
pub const EMPTY_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
/// Media type of the manifests this client writes and reads.
pub const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Media type of a signature manifest's layer.
const SIGNATURE_LAYER_MEDIA_TYPE: &str = "application/vnd.gaia.signature.v1";
/// Annotation holding the entry kind. Caller extras cannot override it.
const KIND_ANNOTATION: &str = "dev.gaia.kind";
/// Annotation holding the entry key. Caller extras cannot override it.
const KEY_ANNOTATION: &str = "dev.gaia.key";
/// Annotation holding the Gaia version that wrote the entry.
const VERSION_ANNOTATION: &str = "dev.gaia.version";
/// Annotation on a signature manifest naming the digest of the entry manifest it signs.
const SIGNS_ANNOTATION: &str = "dev.gaia.signs";
/// Standard OCI annotation: the RFC 3339 UTC time the manifest was written.
const CREATED_ANNOTATION: &str = "org.opencontainers.image.created";

/// How manifests are signed. Parsed from the same words as `[image.signing] kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureKind {
    Minisign,
    Gpg,
    CosignBlob,
}

impl SignatureKind {
    /// The kind `[image.signing] kind` names, or `None` for any other word.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "minisign" => Some(Self::Minisign),
            "gpg" => Some(Self::Gpg),
            "cosign-blob" => Some(Self::CosignBlob),
            _ => None,
        }
    }

    /// The program that makes and checks signatures of this kind.
    fn program(self) -> &'static str {
        match self {
            Self::Minisign => "minisign",
            Self::Gpg => "gpg",
            Self::CosignBlob => "cosign",
        }
    }
}

/// Where the registry is and how its entries are trusted. Keys are resolved by
/// the caller (`env:VAR` already expanded). `Debug` never shows key values.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistryTarget {
    /// `host[:port]`, for example `ghcr.io` or `127.0.0.1:5000`.
    pub registry: String,
    /// `<owner>/gaia-cache`; the image of an entry is this plus its kind.
    pub repository: String,
    pub kind: SignatureKind,
    /// The key that signs pushed manifests. `None` disables pushing.
    pub sign_key: Option<String>,
    /// The key that verifies pulled manifests. `None` refuses pulls unless `trust_unsigned`.
    pub verify_key: Option<String>,
    /// Explicit unsafe opt-in: pulls with no verify key skip the signature check.
    pub trust_unsigned: bool,
    /// Directory holding the signing tool. `None` searches `PATH`.
    pub tool_dir: Option<PathBuf>,
}

impl fmt::Debug for RegistryTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryTarget")
            .field("registry", &self.registry)
            .field("repository", &self.repository)
            .field("kind", &self.kind)
            .field("sign_key", &self.sign_key.as_ref().map(|_| "<set>"))
            .field("verify_key", &self.verify_key.as_ref().map(|_| "<set>"))
            .field("trust_unsigned", &self.trust_unsigned)
            .field("tool_dir", &self.tool_dir)
            .finish()
    }
}

/// A pulled entry: its manifest bytes (exactly as signed), its annotations, and
/// the verified blob, stored as `<dest_dir>/<sha256 hex>.tar.zst`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PulledEntry {
    pub manifest: Vec<u8>,
    pub annotations: BTreeMap<String, String>,
    pub blob: PathBuf,
}

/// What a push did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// No sign key is configured: nothing was sent.
    Disabled,
    /// The key tag already exists: nothing was uploaded.
    AlreadyPresent,
    /// The entry was uploaded and signed; `digest` is the manifest digest.
    Pushed { digest: String },
}

/// Why a registry operation failed. `Display` never includes tokens or keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// A pull was refused because no verify key is set and unverified pulls are not allowed.
    Unsigned(String),
    /// A signature is missing, does not verify, or a signing tool failed.
    Signature(String),
    /// The registry could not be reached, or refused the credentials.
    Transport(String),
    /// The registry answered something this client cannot use, or content failed its digest.
    Protocol(String),
    /// A local file or helper program failed.
    Io(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsigned(message) => write!(f, "registry entry refused: {message}"),
            Self::Signature(message) => write!(f, "registry signature check failed: {message}"),
            Self::Transport(message) => write!(f, "registry transport failed: {message}"),
            Self::Protocol(message) => write!(f, "registry protocol error: {message}"),
            Self::Io(message) => write!(f, "registry local error: {message}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Blocking client for one registry repository. Clones share the resolved
/// credentials and the bearer tokens already issued.
#[derive(Clone)]
pub struct RemoteCache {
    target: Arc<RegistryTarget>,
    session: Arc<Session>,
}

/// State shared by the clones of one cache.
struct Session {
    /// Resolved on first use from the environment, or set up front by a test.
    credentials: OnceLock<Credentials>,
    /// Bearer token per image, as issued by the registry's token service.
    bearers: Mutex<BTreeMap<String, String>>,
}

impl fmt::Debug for RemoteCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteCache")
            .field("registry", &self.target.registry)
            .field("repository", &self.target.repository)
            .finish()
    }
}

impl RemoteCache {
    /// A cache for `target`. Credentials are read from the environment, the
    /// docker config and `gh` only when a request first needs them.
    pub fn new(target: RegistryTarget) -> Self {
        Self::with_cell(target, OnceLock::new())
    }

    /// A cache whose credentials are given, not resolved. Used by tests so they
    /// never read the developer's environment or docker config.
    #[cfg(test)]
    fn with_credentials(target: RegistryTarget, credentials: Credentials) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(credentials);
        Self::with_cell(target, cell)
    }

    fn with_cell(target: RegistryTarget, credentials: OnceLock<Credentials>) -> Self {
        Self {
            target: Arc::new(target),
            session: Arc::new(Session {
                credentials,
                bearers: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// Whether the key tag of `kind` exists.
    pub fn has(&self, kind: &str, key: &str) -> Result<bool, RegistryError> {
        let image = self.image(kind)?;
        check_tag(key)?;
        Ok(self.fetch_manifest(&image, key, "HEAD")?.is_some())
    }

    /// Pulls the entry `kind`/`key` into `dest_dir`. Returns `Ok(None)` when the
    /// key tag does not exist. The signature is verified before the blob is
    /// downloaded; with no verify key the pull is refused unless `trust_unsigned`.
    pub fn pull(
        &self,
        kind: &str,
        key: &str,
        dest_dir: &Path,
    ) -> Result<Option<PulledEntry>, RegistryError> {
        let image = self.image(kind)?;
        check_tag(key)?;
        if self.target.verify_key.is_none() && !self.target.trust_unsigned {
            return Err(RegistryError::Unsigned(format!(
                "no verify key is configured for {image}:{key}; set one, or trust_unsigned"
            )));
        }
        let Some(response) = self.fetch_manifest(&image, key, "GET")? else {
            return Ok(None);
        };
        let manifest = response.body;
        if self.target.verify_key.is_some() {
            self.verify_manifest(&image, key, &manifest, dest_dir)?;
        }
        let parsed = manifest::parse(&manifest)?;
        if parsed.annotations.get(KIND_ANNOTATION).map(String::as_str) != Some(kind)
            || parsed.annotations.get(KEY_ANNOTATION).map(String::as_str) != Some(key)
        {
            return Err(RegistryError::Protocol(format!(
                "{image}:{key} does not describe that entry"
            )));
        }
        let [(media_type, digest)] = parsed.layers.as_slice() else {
            return Err(RegistryError::Protocol(format!(
                "{image}:{key} must have exactly one layer"
            )));
        };
        if media_type != ARTIFACT_TYPE {
            return Err(RegistryError::Protocol(format!(
                "{image}:{key} has layer type '{media_type}', not {ARTIFACT_TYPE}"
            )));
        }
        check_digest(digest)?;
        let blob = dest_dir.join(format!("{}.tar.zst", digest.trim_start_matches("sha256:")));
        self.download_blob(&image, digest, &blob)?;
        Ok(Some(PulledEntry {
            manifest,
            annotations: parsed.annotations,
            blob,
        }))
    }

    /// Pushes the entry `kind`/`key` from the tar.zst at `blob_path`. `annotations`
    /// are merged into the manifest; `kind` and `key` always win over them.
    /// Nothing is sent when no sign key is set or the key tag already exists.
    pub fn push(
        &self,
        kind: &str,
        key: &str,
        blob_path: &Path,
        annotations: &BTreeMap<String, String>,
    ) -> Result<PushOutcome, RegistryError> {
        let image = self.image(kind)?;
        check_tag(key)?;
        if self.target.sign_key.is_none() {
            return Ok(PushOutcome::Disabled);
        }
        if self.fetch_manifest(&image, key, "HEAD")?.is_some() {
            return Ok(PushOutcome::AlreadyPresent);
        }
        let blob = describe_file(blob_path, ARTIFACT_TYPE)?;
        let manifest = manifest::build(&entry_annotations(kind, key, annotations), &blob);
        let manifest_digest = manifest::digest_of(&manifest);
        let manifest_file = Scratch::create(&std::env::temp_dir(), "manifest.json", &manifest)?;
        let signature = signing::sign_bytes(&self.target, manifest_file.path())?;

        self.upload_blob(&image, &blob.digest, Body::File(blob_path.to_path_buf()))?;
        let config_digest = manifest::digest_of(manifest::EMPTY_CONFIG);
        let config = Body::Bytes(manifest::EMPTY_CONFIG.to_vec());
        self.upload_blob(&image, &config_digest, config)?;
        self.put_manifest(&image, key, &manifest)?;

        let signature_layer = Descriptor {
            media_type: SIGNATURE_LAYER_MEDIA_TYPE.to_string(),
            digest: manifest::digest_of(&signature),
            size: signature.len() as u64,
        };
        self.upload_blob(&image, &signature_layer.digest, Body::Bytes(signature))?;
        let signature_annotations = BTreeMap::from([
            (KIND_ANNOTATION.to_string(), kind.to_string()),
            (KEY_ANNOTATION.to_string(), key.to_string()),
            (SIGNS_ANNOTATION.to_string(), manifest_digest.clone()),
        ]);
        let signature_manifest = manifest::build(&signature_annotations, &signature_layer);
        self.put_manifest(&image, &format!("{key}.sig"), &signature_manifest)?;
        Ok(PushOutcome::Pushed {
            digest: manifest_digest,
        })
    }

    /// The tags of `image` (a full image name). An unknown image has none.
    /// Only the first page the registry sends is read.
    pub fn list_tags(&self, image: &str) -> Result<Vec<String>, RegistryError> {
        check_name(image)?;
        let path = format!("{image}/tags/list");
        let response = self.send(image, Request::new("GET", &self.url(&path)))?;
        if response.status == 404 {
            return Ok(Vec::new());
        }
        check(&response, &path, &[200])?;
        let value = parse_json(&response.body, &path)?;
        Ok(value
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The digest the registry gives the manifest at `tag`, or `None` when absent.
    pub fn manifest_digest(&self, image: &str, tag: &str) -> Result<Option<String>, RegistryError> {
        check_name(image)?;
        check_reference(tag)?;
        let Some(response) = self.fetch_manifest(image, tag, "HEAD")? else {
            return Ok(None);
        };
        response
            .header("docker-content-digest")
            .map(|digest| Some(digest.to_string()))
            .ok_or_else(|| RegistryError::Protocol(format!("{image}:{tag} sent no digest")))
    }

    /// The manifest bytes at `reference` (a tag or a digest) with their digest,
    /// or `None` when absent. The bytes are checked against the digest the
    /// registry sent.
    pub fn get_manifest(
        &self,
        image: &str,
        reference: &str,
    ) -> Result<Option<(Vec<u8>, String)>, RegistryError> {
        check_name(image)?;
        check_reference(reference)?;
        let Some(response) = self.fetch_manifest(image, reference, "GET")? else {
            return Ok(None);
        };
        let digest = manifest::digest_of(&response.body);
        if response
            .header("docker-content-digest")
            .is_some_and(|sent| sent != digest)
        {
            return Err(RegistryError::Protocol(format!(
                "{image}:{reference} does not match its digest"
            )));
        }
        Ok(Some((response.body, digest)))
    }

    /// Deletes the manifest with `digest`, which also removes its tags. A
    /// manifest that is already gone counts as deleted. Only the generic OCI
    /// delete is used; registries that refuse it fail with a protocol error.
    pub fn delete_manifest(&self, image: &str, digest: &str) -> Result<(), RegistryError> {
        check_name(image)?;
        check_digest(digest)?;
        let path = format!("{image}/manifests/{digest}");
        let response = self.send(image, Request::new("DELETE", &self.url(&path)))?;
        check(&response, &path, &[200, 202, 404])
    }

    fn image(&self, kind: &str) -> Result<String, RegistryError> {
        let image = format!("{}/{kind}", self.target.repository);
        check_name(&image)?;
        Ok(image)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v2/{path}", http::base_url(&self.target.registry))
    }

    /// Sends `request` for `image`. The first try is anonymous unless a bearer
    /// token is cached for the image. A 401 is answered with the challenge's
    /// token (bearer) or the basic credentials, and the request is retried once.
    fn send(&self, image: &str, mut request: Request) -> Result<Response, RegistryError> {
        request.auth = match self.cached_bearer(image) {
            Some(token) => Auth::Bearer(token),
            None => Auth::Anonymous,
        };
        let response = http::send(&request)?;
        if response.status != 401 {
            return Ok(response);
        }
        request.auth = match response
            .header("www-authenticate")
            .map(auth::parse_challenge)
        {
            Some(auth::Challenge::Bearer {
                realm,
                service,
                scope,
            }) => {
                let token = self.fetch_token(&realm, &service, &scope)?;
                self.remember_bearer(image, &token);
                Auth::Bearer(token)
            }
            _ => match self.basic_auth() {
                Some(auth) => auth,
                None => return Ok(response),
            },
        };
        http::send(&request)
    }

    fn credentials(&self) -> &Credentials {
        self.session
            .credentials
            .get_or_init(|| auth::resolve(&self.target.registry))
    }

    fn basic_auth(&self) -> Option<Auth> {
        match self.credentials() {
            Credentials::Anonymous => None,
            Credentials::Basic { username, secret } => Some(Auth::Basic {
                user: username.clone(),
                secret: secret.clone(),
            }),
        }
    }

    fn cached_bearer(&self, image: &str) -> Option<String> {
        self.session
            .bearers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(image)
            .cloned()
    }

    fn remember_bearer(&self, image: &str, token: &str) {
        self.session
            .bearers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(image.to_string(), token.to_string());
    }

    /// Asks the token service named by a bearer challenge for a token for the
    /// scope. The credentials go to the service as basic auth, as docker does.
    fn fetch_token(
        &self,
        realm: &str,
        service: &str,
        scope: &str,
    ) -> Result<String, RegistryError> {
        let separator = if realm.contains('?') { '&' } else { '?' };
        let url = format!(
            "{realm}{separator}service={}&scope={}",
            url_encode(service),
            url_encode(scope)
        );
        let mut request = Request::new("GET", &url);
        // The credentials only go to the registry's own host: a challenge
        // that names another realm must not receive them.
        request.auth = if realm_host(realm) == Some(host_of(&self.target.registry)) {
            self.basic_auth().unwrap_or(Auth::Anonymous)
        } else {
            Auth::Anonymous
        };
        let response = http::send(&request)?;
        if response.status != 200 {
            return Err(status_error("the token service", response.status));
        }
        let value = parse_json(&response.body, "token response")?;
        value
            .get("token")
            .or_else(|| value.get("access_token"))
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| RegistryError::Protocol("token response has no token".into()))
    }

    /// GETs or HEADs the manifest at `reference`: `None` on 404.
    fn fetch_manifest(
        &self,
        image: &str,
        reference: &str,
        method: &str,
    ) -> Result<Option<Response>, RegistryError> {
        let path = format!("{image}/manifests/{reference}");
        let request = Request::new(method, &self.url(&path)).header("Accept", MANIFEST_MEDIA_TYPE);
        let response = self.send(image, request)?;
        match response.status {
            200 => Ok(Some(response)),
            404 => Ok(None),
            status => Err(status_error(&path, status)),
        }
    }

    fn put_manifest(
        &self,
        image: &str,
        reference: &str,
        bytes: &[u8],
    ) -> Result<(), RegistryError> {
        let path = format!("{image}/manifests/{reference}");
        let request = Request::new("PUT", &self.url(&path))
            .header("Content-Type", MANIFEST_MEDIA_TYPE)
            .body(Body::Bytes(bytes.to_vec()));
        let response = self.send(image, request)?;
        check(&response, &path, &[200, 201])
    }

    /// Uploads a blob unless the registry already has it. The upload is
    /// monolithic: start a session, then PUT the whole body with its digest.
    fn upload_blob(&self, image: &str, digest: &str, body: Body) -> Result<(), RegistryError> {
        let path = format!("{image}/blobs/{digest}");
        let head = self.send(image, Request::new("HEAD", &self.url(&path)))?;
        if head.status == 200 {
            return Ok(());
        }
        check(&head, &path, &[404])?;
        let start_path = format!("{image}/blobs/uploads/");
        let start = self.send(image, Request::new("POST", &self.url(&start_path)))?;
        check(&start, &start_path, &[202])?;
        let location = start
            .header("location")
            .ok_or_else(|| RegistryError::Protocol("upload start sent no Location".into()))?;
        let location = absolute_location(&self.target.registry, location);
        let separator = if location.contains('?') { '&' } else { '?' };
        let put = Request::new("PUT", &format!("{location}{separator}digest={digest}"))
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .max_time(BLOB_REQUEST_SECS);
        check(&self.send(image, put)?, &path, &[201])
    }

    /// Downloads a blob into `dest` and checks its sha256 against `digest`. A
    /// file that fails is removed.
    fn download_blob(&self, image: &str, digest: &str, dest: &Path) -> Result<(), RegistryError> {
        let path = format!("{image}/blobs/{digest}");
        let request = Request::new("GET", &self.url(&path))
            .out(Out::File(dest.to_path_buf()))
            .follow_redirects()
            .max_time(BLOB_REQUEST_SECS);
        let outcome = self
            .send(image, request)
            .and_then(|response| check(&response, &path, &[200]))
            .and_then(|()| {
                let actual = crate::sha256_file(dest).map_err(|error| io_error(&path, error))?;
                if format!("sha256:{actual}") == digest {
                    Ok(())
                } else {
                    Err(RegistryError::Protocol(format!(
                        "blob {digest} failed its sha256 check"
                    )))
                }
            });
        if outcome.is_err() {
            let _ = fs::remove_file(dest);
        }
        outcome
    }

    /// Checks the signature tagged `<key>.sig` over the exact `manifest` bytes.
    /// A missing signature is a signature failure.
    fn verify_manifest(
        &self,
        image: &str,
        key: &str,
        manifest: &[u8],
        dir: &Path,
    ) -> Result<(), RegistryError> {
        let Some(signature_manifest) = self.fetch_manifest(image, &format!("{key}.sig"), "GET")?
        else {
            return Err(RegistryError::Signature(format!(
                "{image}:{key} has no signature"
            )));
        };
        let parsed = manifest::parse(&signature_manifest.body)?;
        let [(_, signature_digest)] = parsed.layers.as_slice() else {
            return Err(RegistryError::Signature(format!(
                "the signature of {image}:{key} must have one layer"
            )));
        };
        check_digest(signature_digest)?;
        let path = format!("{image}/blobs/{signature_digest}");
        let response = self.send(image, Request::new("GET", &self.url(&path)))?;
        check(&response, &path, &[200])?;
        let manifest_file = Scratch::create(dir, "manifest", manifest)?;
        let signature_file = Scratch::create(dir, "signature", &response.body)?;
        signing::verify_bytes(&self.target, manifest_file.path(), signature_file.path())
    }
}

/// The annotations an entry manifest carries: the creation time and version,
/// then the caller's extras, then `kind` and `key`, which extras cannot change.
fn entry_annotations(
    kind: &str,
    key: &str,
    extra: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut annotations = BTreeMap::new();
    annotations.insert(CREATED_ANNOTATION.to_string(), manifest::rfc3339_utc_now());
    annotations.insert(
        VERSION_ANNOTATION.to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    annotations.extend(
        extra
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    annotations.insert(KIND_ANNOTATION.to_string(), kind.to_string());
    annotations.insert(KEY_ANNOTATION.to_string(), key.to_string());
    annotations
}

/// The digest, size and media type of a file, for use as a layer.
fn describe_file(path: &Path, media_type: &str) -> Result<Descriptor, RegistryError> {
    let context = path.display().to_string();
    let digest = crate::sha256_file(path).map_err(|error| io_error(&context, error))?;
    let size = fs::metadata(path)
        .map_err(|error| io_error(&context, error))?
        .len();
    Ok(Descriptor {
        media_type: media_type.to_string(),
        digest: format!("sha256:{digest}"),
        size,
    })
}

/// Resolves a `Location` header, which registries may send as a path.
fn absolute_location(registry: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        location.to_string()
    } else {
        format!("{}{location}", http::base_url(registry))
    }
}

/// Accepts the response statuses in `accepted`; anything else is an error.
fn check(response: &Response, what: &str, accepted: &[u16]) -> Result<(), RegistryError> {
    if accepted.contains(&response.status) {
        Ok(())
    } else {
        Err(status_error(what, response.status))
    }
}

/// The error for an unexpected status: a refusal of the credentials is a
/// transport failure, anything else a protocol one.
fn status_error(what: &str, status: u16) -> RegistryError {
    if matches!(status, 401 | 403) {
        RegistryError::Transport(format!(
            "refused the credentials for {what} (HTTP {status})"
        ))
    } else {
        RegistryError::Protocol(format!("{what} answered HTTP {status}"))
    }
}

fn parse_json(bytes: &[u8], what: &str) -> Result<Value, RegistryError> {
    serde_json::from_slice(bytes)
        .map_err(|error| RegistryError::Protocol(format!("{what} is not JSON: {error}")))
}

fn io_error(context: &str, error: std::io::Error) -> RegistryError {
    RegistryError::Io(format!("{context}: {error}"))
}

#[path = "names.rs"]
mod names;
use names::*;
