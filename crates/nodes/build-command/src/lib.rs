//! `gaia.build.command`: a declared command, run in a pinned image over
//! declared input trees, keeping only its declared outputs.
//!
//! This replaces shell hooks. What the command reads must be an input:
//! the image (a stored root filesystem tree) and each input tree, bound
//! read-only where the step says; nothing else of the host is there. What
//! it makes must be a declared output: a file or directory under `/out`.
//! Anything else left in `/out` is an error naming it, and a declared
//! output that is missing, or a file where a directory was declared (or the
//! other way round), is an error too.
//!
//! The run is [`gaia_sandbox`]'s: rootless bubblewrap, network off unless
//! declared, a clear environment with only the declared variables. The key
//! holds the image's and every input's digest, the command, the declared
//! outputs and what the sandbox adds (mount points, environment, network,
//! jobs).

mod check;

use daedalus::prelude::*;
use gaia_sandbox::{Limits, Sandbox};
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.command";

/// An input tree and where it is bound inside, read-only.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:command-input")]
pub struct CommandInput {
    pub tree: Tree,
    /// An absolute path inside (`/src`).
    pub at: String,
}

/// A declared output: a path under `/out` and the kind of artifact it is.
/// Kind `tree` declares a directory; any other kind, a file.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:command-output")]
pub struct CommandOutput {
    /// Relative to `/out` (`firmware.bin`, `dist`).
    pub path: String,
    pub kind: String,
}

/// The command: a program inside the image, its arguments, and `NAME=value`
/// environment entries.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:command")]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    /// The directory it starts in; `/out` when empty.
    pub cwd: String,
    pub network: bool,
    /// Build jobs (`GAIA_JOBS`, `MAKEFLAGS`); none when 0.
    pub jobs: u32,
}

/// Runs `command` with `bwrap` in `image`, `inputs` bound in, and gives one
/// artifact per declared output, in their order.
#[node(
    id = "gaia.build.command",
    inputs(store, bwrap, image, inputs, command, outputs),
    outputs(artifacts)
)]
pub fn build_command(
    store: &str,
    bwrap: &str,
    image: Tree,
    inputs: Vec<CommandInput>,
    command: Command,
    outputs: Vec<CommandOutput>,
    cx: &Ctx,
) -> Result<Vec<Artifact>, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |message: String| StepError::body(NAME, message);
    let declared = check::outputs(&outputs).map_err(fail)?;
    let in_store = |tree: &Tree, what: &str| -> Result<(PathBuf, gaia_store::Digest), StepError> {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| fail(format!("{what} is not in the store")))?;
        Ok((store.path(&object), object.digest))
    };
    let (image_dir, image_digest) = in_store(&image, "the image")?;
    let mut sandbox = Sandbox::new(bwrap, &image_dir).network(command.network);
    let mut key = KeyBuilder::new(NAME, 1).input("image", image_digest);
    for (i, input) in inputs.iter().enumerate() {
        check::mount_point(&input.at).map_err(fail)?;
        let (dir, digest) = in_store(&input.tree, &format!("input {}", input.at))?;
        sandbox = sandbox.input(dir, &input.at);
        key = key.input(&format!("input.{i}"), digest);
    }
    for entry in &command.env {
        let (name, value) = entry
            .split_once('=')
            .ok_or_else(|| fail(format!("env {entry:?} is not NAME=value")))?;
        sandbox = sandbox.env(name, value);
    }
    if !command.cwd.is_empty() {
        sandbox = sandbox.cwd(&command.cwd);
    }
    if command.jobs > 0 {
        sandbox = sandbox.limits(Limits {
            cpus: None,
            jobs: Some(command.jobs),
        });
    }
    key = sandbox
        .key(key)
        .config("out", "/out")
        .config("program", &command.program)
        .config("args", command.args.join("\0"))
        .config(
            "outputs",
            outputs
                .iter()
                .map(|o| format!("{} {}", o.path, o.kind))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    let names: Vec<String> = (0..outputs.len()).map(|i| format!("output.{i}")).collect();
    let paths: Vec<PathBuf> = declared.iter().map(|d| Path::new("out").join(d)).collect();
    let pairs: Vec<(&str, &Path)> = names
        .iter()
        .map(String::as_str)
        .zip(paths.iter().map(PathBuf::as_path))
        .collect();
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(&pairs, |work| {
        let out = work.dir().join("out");
        std::fs::create_dir(&out).map_err(|e| fail(format!("out: {e}")))?;
        let tool = sandbox
            .clone()
            .output(&out, "/out")
            .tool(&command.program)
            .map_err(|e| fail(format!("the image: {e}")))?
            .args(&command.args);
        work.run(&tool)?;
        check::produced(&out, &outputs, &declared).map_err(fail)
    })?;
    Ok(outputs
        .iter()
        .zip(&names)
        .map(|(o, name)| Artifact {
            kind: ArtifactKind::parse(&o.kind),
            object: made.outputs[name].into(),
        })
        .collect())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_COMMAND_NODE];
