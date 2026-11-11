//! `gaia.source.download`: a URL fetched and checked against its digest.

use daedalus::prelude::*;
use gaia_step::{Ctx, ErrorKind, KeyBuilder, Step, StepError, Store, Tool};
use gaia_store::{Digest, file_digest};
use gaia_types::{Artifact, ArtifactKind};
use std::fs;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.source.download";

/// curl's exit when the server will not resume.
const CANNOT_RESUME: i32 = 33;

/// Where partial downloads wait to be resumed: `downloads` next to the
/// store's root.
pub fn partial_dir(store: &Store) -> PathBuf {
    store.root().with_file_name("downloads")
}

/// Fetches `url` with the `curl` program given and checks it against
/// `digest` (`sha256:<hex>`, from the lock). `kind` is the artifact kind
/// the description gives it (`archive`, `firmware`).
#[node(
    id = "gaia.source.download",
    inputs(store, curl, url, digest, kind),
    outputs(file)
)]
pub fn source_download(
    store: &str,
    curl: &str,
    url: &str,
    digest: &str,
    kind: &str,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let want = Digest::parse(digest.trim()).ok_or_else(|| {
        StepError::body(NAME, format!("{url}: `{digest}` is not a sha256 digest"))
    })?;
    let key = KeyBuilder::new(NAME, 1).input("file", want).finish();
    let partials = partial_dir(&store);
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("file", Path::new("file"))],
        |work| {
            fs::create_dir_all(&partials).map_err(|e| StepError::body(NAME, e))?;
            let partial = partials.join(format!("{}.part", want.hex()));
            // One fetch of a digest at a time: they would write one file.
            let lock = fs::File::create(partials.join(format!("{}.lock", want.hex())))
                .map_err(|e| StepError::body(NAME, e))?;
            lock.lock().map_err(|e| StepError::body(NAME, e))?;
            let fetch = |resume: bool| {
                let mut tool =
                    Tool::new(curl).args(["--location", "--fail", "--silent", "--show-error"]);
                if resume {
                    tool = tool.args(["--continue-at", "-"]);
                }
                tool.arg("--output")
                    .arg(&partial)
                    .arg(url)
                    .env("PATH", "/usr/bin:/bin")
            };
            match work.run(&fetch(true)) {
                Ok(_) => {}
                Err(error) if *error.kind() == ErrorKind::Exit(CANNOT_RESUME) => {
                    // The server will not resume: start again from nothing.
                    fs::remove_file(&partial).map_err(|e| StepError::body(NAME, e))?;
                    work.run(&fetch(false)).map_err(|e| named(url, e))?;
                }
                Err(error) => return Err(named(url, error)),
            }
            let got = file_digest(&partial).map_err(|e| StepError::body(NAME, e))?;
            if got != want {
                let _ = fs::remove_file(&partial);
                return Err(StepError::body(
                    NAME,
                    format!("{url}: digest mismatch: the lock says {want}, the download is {got}"),
                ));
            }
            fs::rename(&partial, work.dir().join("file")).map_err(|e| StepError::body(NAME, e))
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse(kind),
        object: made.outputs["file"].into(),
    })
}

/// A failed fetch, with the URL in its reason.
fn named(url: &str, error: StepError) -> StepError {
    let text = error.to_string();
    let first = text.lines().next().unwrap_or_default();
    let what = first.strip_prefix(&format!("{NAME}: ")).unwrap_or(first);
    let mut named = StepError::body(NAME, format!("{url}: {what}"));
    if let Some(log) = error.log() {
        named = named.with_log(log.to_path_buf());
    }
    named
}
