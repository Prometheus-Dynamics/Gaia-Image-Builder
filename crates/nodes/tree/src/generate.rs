//! `gaia.generate`: files a pinned tool writes from a board's description.
//!
//! Some of what an image carries is not written by anyone: it is derived
//! from one description of the board by the program that will read the
//! result (a device tree overlay, a kernel command line fragment, device
//! permissions, a boot directory). The tool is built for the build machine
//! from a locked source, and run here once per image with the board's
//! description, so every derived file comes from the same run.
//!
//! The arguments are the description's own words, with four names the
//! step fills in:
//!
//! - `@source@`: the tool's source tree (for data the tool reads from it);
//! - `@description@`: the board's description file;
//! - `@out@`: the directory the tool writes into;
//! - `@slot@`: the slot's id, where the image has one.
//!
//! What the tool is to write is declared: each declared output must be
//! there when the tool ends, and nothing else may be at the top of `@out@`.
//! A tool that silently stops writing a file, or starts writing a new one,
//! fails the step by that file's name.
//!
//! The tool runs with an empty environment in the step's own directory. It
//! is not yet inside the network-less sandbox the builders use.

use crate::walk::inside;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::Tree;
use std::collections::BTreeSet;
use std::path::Path;

const NAME: &str = "gaia.generate";
/// The names an argument may hold.
pub const TOKENS: [&str; 4] = ["@source@", "@description@", "@out@", "@slot@"];

/// The `@name@` words of `text` that are none of [`TOKENS`].
pub fn unknown_tokens(text: &str) -> Vec<String> {
    let mut unknown = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('@') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('@') else { break };
        let word = &after[..end];
        let whole = format!("@{word}@");
        let is_name = !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if is_name && !TOKENS.contains(&whole.as_str()) {
            unknown.push(whole);
        }
        // A word that is no name (a path with an `@` in it) is left alone,
        // and its closing `@` may open the next.
        rest = if is_name {
            &after[end + 1..]
        } else {
            &after[end..]
        };
    }
    unknown
}

/// Runs `program` (a path in the tree `tool`) with `args` (a JSON array of
/// strings, since a graph sets it as one constant) and gives what it wrote
/// under `@out@` as a tree: exactly the paths `outputs` declares (a JSON
/// array of paths at the top of `@out@`).
///
/// `source` is the tool's source tree, `description` a tree with the
/// board's description as its one file `description_name`, and `slot` the
/// slot's id (empty when the image has none: an argument that names
/// `@slot@` is then an error).
#[node(
    id = "gaia.generate",
    inputs(
        store,
        tool,
        program,
        source,
        description,
        description_name,
        args,
        outputs,
        slot
    ),
    outputs(tree)
)]
#[allow(clippy::too_many_arguments)]
pub fn generate(
    store: &str,
    tool: Tree,
    program: &str,
    source: Tree,
    description: Tree,
    description_name: &str,
    args: &str,
    outputs: &str,
    slot: &str,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let in_store = |tree: &Tree, what: &str| {
        tree.object
            .object_ref()
            .ok_or_else(|| fail(format!("{what} is not in the store")))
    };
    let tool_object = in_store(&tool, "the tool")?;
    let source_object = in_store(&source, "the tool's source")?;
    let description_object = in_store(&description, "the board's description")?;
    let program_path = store
        .path(&tool_object)
        .join(inside(program).map_err(fail)?);
    if !program_path.is_file() {
        return Err(fail(format!("the tool's build has no {program}")));
    }
    let described = store
        .path(&description_object)
        .join(inside(description_name).map_err(fail)?);
    if !described.is_file() {
        return Err(fail(format!(
            "the board's description {description_name} is not a file"
        )));
    }
    let args: Vec<String> = serde_json::from_str(args)
        .map_err(|e| fail(format!("the arguments are not a list of strings: {e}")))?;
    let declared: Vec<String> = serde_json::from_str(outputs)
        .map_err(|e| fail(format!("the outputs are not a list of paths: {e}")))?;
    let mut tops = BTreeSet::new();
    for output in &declared {
        let path = inside(output).map_err(fail)?;
        if path.contains('/') {
            return Err(fail(format!(
                "the output {output} is not at the top of the output directory"
            )));
        }
        tops.insert(path);
    }
    if tops.is_empty() {
        return Err(fail("no output is declared".into()));
    }
    for arg in &args {
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
    }
    let uses_source = args.iter().any(|arg| arg.contains("@source@"));

    // The arguments as written: no path of this store is in the key.
    let mut key = KeyBuilder::new(NAME, 1)
        .input("tool", tool_object.digest)
        .config("program", program)
        .input("description", description_object.digest)
        .config("description name", description_name)
        .config("args", args.join("\n"))
        .config(
            "outputs",
            tops.iter().cloned().collect::<Vec<_>>().join("\n"),
        )
        .config("slot", slot);
    // The source only when the tool is pointed at it.
    if uses_source {
        key = key.input("source", source_object.digest);
    }
    let source_path = store.path(&source_object);
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("tree", Path::new("out"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let out = work.dir().join("out");
            std::fs::create_dir(&out).map_err(io)?;
            let filled: Vec<String> = args
                .iter()
                .map(|arg| {
                    arg.replace("@source@", &source_path.display().to_string())
                        .replace("@description@", &described.display().to_string())
                        .replace("@out@", &out.display().to_string())
                        .replace("@slot@", slot)
                })
                .collect();
            work.run(
                &Tool::new(&program_path)
                    .args(filled)
                    .cwd(work.dir())
                    .env("LC_ALL", "C"),
            )?;
            let mut written = BTreeSet::new();
            for entry in std::fs::read_dir(&out).map_err(io)? {
                written.insert(
                    entry
                        .map_err(io)?
                        .file_name()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            let missing: Vec<&String> = tops.difference(&written).collect();
            let extra: Vec<&String> = written.difference(&tops).collect();
            if !missing.is_empty() || !extra.is_empty() {
                let list = |names: &[&String]| {
                    names
                        .iter()
                        .map(|name| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let mut why = format!("{program} did not write what is declared:");
                if !missing.is_empty() {
                    why.push_str(&format!("\n  declared and not written: {}", list(&missing)));
                }
                if !extra.is_empty() {
                    why.push_str(&format!("\n  written and not declared: {}", list(&extra)));
                }
                return Err(StepError::body(NAME, why));
            }
            Ok(())
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}
