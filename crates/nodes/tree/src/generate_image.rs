//! `gaia.generate.image`: one image file written by a component's tool from
//! a tree the build gives it.
//!
//! Where [`crate::generate`] runs a tool on the board's description alone,
//! this runs the same tool on the description and a *payload*: a tree of
//! what the build made, each file under the name of what it is. The tool
//! owns the format of what it writes (a boot partition with its own
//! records and names); the build owns the size, which is the image
//! layout's, and nothing else of it.
//!
//! Up to three runs of the tool, each a list of arguments:
//!
//! - `fit` (optional), before anything is written: the tool says whether
//!   the payload fits, and fails when it does not. What it prints is kept
//!   as the step's `report` (a line per role and its stored size, say), so
//!   a board's sizes are set from a build and not guessed.
//! - `args`: writes the image, the one file `@out@`.
//! - `verify` (optional): the tool reads back what it wrote; a non-zero
//!   exit fails the step.
//!
//! In an argument: `@source@` is the tool's source tree, `@description@`
//! the board's description file, `@payload@` the payload directory,
//! `@slot@` the image's slot id, `@size@` the image's size in bytes and
//! `@out@` the file to write. Any other `@name@` is refused.
//!
//! A recipe can give the run more, for a file that holds the whole slot (a
//! bundle an installer takes): `@rootfs@`, the slot's root image, and
//! `@verity-table@`, its table; `@product@` and `@version@`, the image's
//! own; and a signing key. The key is a file the environment names
//! (`key_env`), bound for the runs at `@key@` and given to the tool by the
//! run's `key_args`, which are added to `args` only when there is a key. It
//! is never stored, and the step is keyed on what the build says
//! identifies it (`key_id`), not on the key. With no `size`, the file
//! written may be any size.
//!
//! The tool runs as [`crate::generate`]'s does: sandboxed in the image it
//! was built in, with no network and nothing of this machine. The file
//! written must be exactly `size` bytes.

use crate::walk::inside;
use daedalus::prelude::*;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Stream};
use gaia_types::{Artifact, ArtifactKind, Tree};
use serde::Deserialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

const NAME: &str = "gaia.generate.image";
const TOOL: &str = "/tool";
const SOURCE: &str = "/source";
const DESCRIPTION: &str = "/description";
const PAYLOAD: &str = "/payload";
const OUT: &str = "/out";
/// Where the slot's root image, its verity table and a signing key file
/// are, when the recipe gives them: each one file.
const ROOTFS: &str = "/rootfs.img";
const TABLE: &str = "/verity-table";
const KEY: &str = "/signing-key";
/// The file the tool writes, in the output directory.
const IMAGE: &str = "image";
pub use gaia_types::GENERATE_IMAGE_NAMES as TOKENS;

/// The `@name@` words of `arg` that are not one of [`TOKENS`].
pub fn unknown_tokens(arg: &str) -> Vec<String> {
    gaia_types::unknown_names(arg, &TOKENS)
}

/// The tool's runs, as a component states them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runs {
    #[serde(default)]
    pub fit: Vec<String>,
    pub args: Vec<String>,
    #[serde(default)]
    pub verify: Vec<String>,
    /// Added to `args` when the build has a signing key: how the tool is
    /// given it (`@key@` is the key file).
    #[serde(default)]
    pub key_args: Vec<String>,
}

/// The image `runs` (JSON of [`Runs`]) makes from `payload`: `size` bytes,
/// written by `program` of `tool`, with what its `fit` run printed.
#[node(
    id = "gaia.generate.image",
    inputs(
        store,
        bwrap,
        root,
        tool,
        program,
        source,
        description,
        description_name,
        payload,
        runs,
        slot,
        size,
        rootfs,
        table,
        key_env,
        key_id,
        product,
        version
    ),
    outputs(image, report)
)]
#[allow(clippy::too_many_arguments)]
pub fn generate_image(
    store: &str,
    bwrap: &str,
    root: Tree,
    tool: Tree,
    program: &str,
    source: Tree,
    description: Tree,
    description_name: &str,
    payload: Tree,
    runs: &str,
    slot: &str,
    size: i64,
    rootfs: Option<&gaia_types::Image>,
    table: Option<&Artifact>,
    key_env: Option<&String>,
    key_id: Option<&String>,
    product: Option<&String>,
    version: Option<&String>,
    cx: &Ctx,
) -> Result<(Artifact, Artifact), StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let in_store = |tree: &Tree, what: &str| {
        tree.object
            .object_ref()
            .ok_or_else(|| fail(format!("{what} is not in the store")))
    };
    let root_object = in_store(&root, "the image the tool runs in")?;
    let tool_object = in_store(&tool, "the tool")?;
    let source_object = in_store(&source, "the tool's source")?;
    let description_object = in_store(&description, "the board's description")?;
    let payload_object = in_store(&payload, "the payload")?;
    let program_rel = inside(program).map_err(fail)?;
    if !store.path(&tool_object).join(&program_rel).is_file() {
        return Err(fail(format!("the tool's build has no {program}")));
    }
    let described = inside(description_name).map_err(fail)?;
    if !store.path(&description_object).join(&described).is_file() {
        return Err(fail(format!(
            "the board's description {description_name} is not a file"
        )));
    }
    let runs: Runs = serde_json::from_str(runs).map_err(|e| {
        fail(format!(
            "the runs are not `fit`, `args` and `verify` lists: {e}"
        ))
    })?;
    if runs.args.is_empty() {
        return Err(fail("no `args`: nothing would write the image".into()));
    }
    // The signing key: a file the environment names, never a stored thing.
    // With one, the tool is told of it by the run's `key_args`; the step is
    // keyed on what the build says identifies the key, not on the key.
    let key_env = key_env.map(String::as_str).unwrap_or_default();
    let key_id = key_id.map(String::as_str).unwrap_or_default();
    let key_file = if key_env.is_empty() {
        None
    } else {
        if key_id.is_empty() {
            return Err(fail(format!(
                "a signing key is taken from {key_env}, and nothing says which key it is: state its public key or id, which the step is keyed on in place of the key"
            )));
        }
        if runs.key_args.is_empty() {
            return Err(fail(format!(
                "a signing key is taken from {key_env}, and the tool's runs have no `key_args` to give it by"
            )));
        }
        let path = std::env::var_os(key_env)
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_file())
            .ok_or_else(|| {
                fail(format!(
                    "{key_env} is not set to a key file's path: the image is signed with the key it names"
                ))
            })?;
        Some(path)
    };
    let mut written_args = runs.args.clone();
    if key_file.is_some() {
        written_args.extend(runs.key_args.iter().cloned());
    }
    let runs = Runs {
        args: written_args,
        ..runs
    };
    let rootfs_object = match rootfs {
        Some(image) => Some(
            image
                .object
                .object_ref()
                .ok_or_else(|| fail("the root image is not in the store".into()))?,
        ),
        None => None,
    };
    let table_object = match table {
        Some(table) => Some(
            table
                .object
                .object_ref()
                .ok_or_else(|| fail("the verity table is not in the store".into()))?,
        ),
        None => None,
    };
    let product = product.map(String::as_str).unwrap_or_default();
    let version = version.map(String::as_str).unwrap_or_default();
    if size < 0 {
        return Err(fail(format!("size {size}: not a size")));
    }
    for arg in runs.fit.iter().chain(&runs.args).chain(&runs.verify) {
        let unknown = unknown_tokens(arg);
        if !unknown.is_empty() {
            return Err(fail(format!(
                "the argument {arg:?} names {}; the names are {}",
                unknown.join(", "),
                TOKENS.join(", ")
            )));
        }
        if arg.contains("@slot@") && slot.is_empty() {
            return Err(fail(format!(
                "the argument {arg:?} names @slot@, and the image has no slot id"
            )));
        }
        for (name, given, what) in [
            ("@size@", size != 0, "the recipe gives no size"),
            (
                "@rootfs@",
                rootfs_object.is_some(),
                "the recipe gives no root image",
            ),
            (
                "@verity-table@",
                table_object.is_some(),
                "the recipe gives no verity table",
            ),
            ("@key@", key_file.is_some(), "the build has no signing key"),
            (
                "@product@",
                !product.is_empty(),
                "the image states no product",
            ),
            (
                "@version@",
                !version.is_empty(),
                "the image states no version",
            ),
        ] {
            if arg.contains(name) && !given {
                return Err(fail(format!(
                    "the argument {arg:?} names {name}, and {what}"
                )));
            }
        }
    }
    if !runs.args.iter().any(|arg| arg.contains("@out@")) {
        return Err(fail(
            "no argument of `args` names @out@, the file to write".into(),
        ));
    }
    let all = || runs.fit.iter().chain(&runs.args).chain(&runs.verify);
    let uses_source = all().any(|arg| arg.contains("@source@"));

    let sandbox = Sandbox::new(bwrap, store.path(&root_object))
        .input(store.path(&tool_object), TOOL)
        .input(store.path(&source_object), SOURCE)
        .input(store.path(&description_object), DESCRIPTION)
        .input(store.path(&payload_object), PAYLOAD)
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C");
    let mut sandbox = sandbox;
    if let Some(object) = &rootfs_object {
        sandbox = sandbox.input(store.path(object), ROOTFS);
    }
    if let Some(object) = &table_object {
        sandbox = sandbox.input(store.path(object), TABLE);
    }
    let mut key = sandbox
        .key(KeyBuilder::new(NAME, 1))
        .input("root", root_object.digest)
        .input("tool", tool_object.digest)
        .config("program", program)
        .input("description", description_object.digest)
        .config("description name", description_name)
        .input("payload", payload_object.digest)
        .config("fit", runs.fit.join("\n"))
        .config("args", runs.args.join("\n"))
        .config("verify", runs.verify.join("\n"))
        .config("slot", slot)
        .config("size", size.to_string());
    if uses_source {
        key = key.input("source", source_object.digest);
    }
    // Each further input only when it is given: a run without them has the
    // key it had.
    if let Some(object) = &rootfs_object {
        key = key.input("rootfs", object.digest);
    }
    if let Some(object) = &table_object {
        key = key.input("table", object.digest);
    }
    if key_file.is_some() {
        key = key.config("key id", key_id);
    }
    if !product.is_empty() || !version.is_empty() {
        key = key.config("product", product).config("version", version);
    }
    // The key file is bound for the runs only: it is no input of the key,
    // and the sandbox with it is not the one the key was taken from.
    let sandbox = match &key_file {
        Some(path) => sandbox.input(path.clone(), KEY),
        None => sandbox,
    };
    let program_in = format!("{TOOL}/{program_rel}");
    let described_in = format!("{DESCRIPTION}/{described}");
    let image_in = format!("{OUT}/{IMAGE}");
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[
            ("image", Path::new("out/image")),
            ("report", Path::new("report.txt")),
        ],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let out = work.dir().join("out");
            std::fs::create_dir(&out).map_err(io)?;
            let fill = |args: &[String]| -> Vec<String> {
                args.iter()
                    .map(|arg| {
                        arg.replace("@source@", SOURCE)
                            .replace("@description@", &described_in)
                            .replace("@payload@", PAYLOAD)
                            .replace("@slot@", slot)
                            .replace("@size@", &size.to_string())
                            .replace("@out@", &image_in)
                            .replace("@rootfs@", ROOTFS)
                            .replace("@verity-table@", TABLE)
                            .replace("@key@", KEY)
                            .replace("@product@", product)
                            .replace("@version@", version)
                    })
                    .collect()
            };
            let run = |args: &[String]| {
                sandbox
                    .clone()
                    .output(&out, OUT)
                    .tool(&program_in)
                    .map(|tool| tool.args(fill(args)))
            };
            // Whether it fits, and what the tool says of it, before a byte
            // is written.
            let said = Arc::new(Mutex::new(String::new()));
            if !runs.fit.is_empty() {
                let sink = said.clone();
                let tool = run(&runs.fit).map_err(io)?.on_line(move |stream, line| {
                    if stream == Stream::Stdout
                        && let Ok(mut text) = sink.lock()
                    {
                        text.push_str(line);
                        text.push('\n');
                    }
                });
                work.run(&tool)?;
            }
            let report = said.lock().map(|text| text.clone()).unwrap_or_default();
            std::fs::write(work.dir().join("report.txt"), &report).map_err(io)?;
            work.run(&run(&runs.args).map_err(io)?)?;
            let image = out.join(IMAGE);
            let written = std::fs::metadata(&image)
                .map_err(|_| StepError::body(NAME, format!("{program} wrote no file at @out@")))?;
            if !written.is_file() || (size != 0 && written.len() != size.unsigned_abs()) {
                return Err(StepError::body(
                    NAME,
                    format!(
                        "{program} wrote {} bytes at @out@; the image's size is {size}",
                        written.len()
                    ),
                ));
            }
            if !runs.verify.is_empty() {
                work.run(&run(&runs.verify).map_err(io)?)?;
            }
            Ok(())
        },
    )?;
    Ok((
        Artifact {
            kind: ArtifactKind::parse("generated-image"),
            object: made.outputs["image"].into(),
        },
        Artifact {
            kind: ArtifactKind::parse("report"),
            object: made.outputs["report"].into(),
        },
    ))
}
