//! `gaia.devicetree.overlay`: a generated device tree overlay, compiled and
//! proven against the tree it is for.
//!
//! An overlay's source is written by a generator from the board's
//! description; the kernel's build makes the base device tree. A firmware
//! that cannot apply the one to the other says nothing and boots without
//! it, so the pair is proven here, where a failure has a name:
//!
//! - the source is compiled with the flags its owner states, with every
//!   warning class its owner lists as an error, by a compiler no older than
//!   its owner's minimum (a contract file read from the owner's source);
//! - the compiled overlay is applied to the base tree the image ships, and
//!   the result thrown away;
//! - what the generator says the image's manifest must state about the
//!   overlay (a small JSON object written in the same run) is found in the
//!   compiled overlay: each string value of it, as a property's value. A
//!   compile that lost the generator's marker fails here and not at every
//!   boot. An empty object states nothing and checks nothing.
//!
//! The source is never edited: its text is what the generator's digest is
//! of. The tools are the image's own build's (`dtc`, `fdtoverlay` under
//! `bin/` of a host tools tree), not this machine's.
//!
//! The contract, TOML, `version = 1`: `minimum_dtc`, `flags` (the exact
//! arguments, in order) and `error_classes` (each passed as `-E <class>`).
//! Another version or an unknown key is refused.

use crate::walk::inside;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::{Artifact, ArtifactKind, Tree};
use serde::Deserialize;
use std::path::Path;

const NAME: &str = "gaia.devicetree.overlay";
/// The version of the compiler contract this reader knows.
const VERSION: i64 = 1;
/// The file the step makes.
const OUTPUT: &str = "overlay.dtbo";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Contract {
    #[allow(dead_code)]
    version: i64,
    minimum_dtc: String,
    flags: Vec<String>,
    error_classes: Vec<String>,
}

/// The compiler's arguments by the contract `text` (named `file` in
/// errors), and the oldest compiler it allows.
///
/// # Errors
///
/// Another version, an unknown key, a flag or class that is not one word, or
/// a minimum that is not a version.
pub fn compiler(text: &str, file: &str) -> Result<(Vec<String>, Vec<u32>), String> {
    let value: toml::Value =
        toml::from_str(text).map_err(|e| format!("{file}: {}", e.message()))?;
    match value.get("version").and_then(toml::Value::as_integer) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(format!(
                "{file}: version {other}; this build reads version {VERSION}"
            ));
        }
        None => return Err(format!("{file}: no `version`")),
    }
    let contract: Contract = value
        .try_into()
        .map_err(|e: toml::de::Error| format!("{file}: {}", e.message()))?;
    let one_word = |word: &str| !word.is_empty() && !word.contains(char::is_whitespace);
    let mut args = Vec::new();
    for flag in &contract.flags {
        if !one_word(flag) {
            return Err(format!("{file}: the flag {flag:?} is not one argument"));
        }
        args.push(flag.clone());
    }
    for class in &contract.error_classes {
        if !one_word(class) || class.starts_with('-') {
            return Err(format!("{file}: {class:?} is not a warning class"));
        }
        args.push("-E".into());
        args.push(class.clone());
    }
    let minimum = version_of(&contract.minimum_dtc).ok_or_else(|| {
        format!(
            "{file}: minimum_dtc {:?} is not a version",
            contract.minimum_dtc
        )
    })?;
    Ok((args, minimum))
}

/// The numbers of the first dotted version in `text` (`Version: DTC 1.7.2`
/// gives 1, 7, 2; a suffix after the numbers is ignored).
pub fn version_of(text: &str) -> Option<Vec<u32>> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|word| word.contains('.') && word.starts_with(|c: char| c.is_ascii_digit()))
        .map(|word| {
            word.split('.')
                .filter_map(|part| part.parse().ok())
                .collect::<Vec<u32>>()
        })
        .filter(|numbers| numbers.len() >= 2)
}

/// Every string value in the JSON `text`, at any depth.
fn strings(value: &serde_json::Value, into: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => into.push(text.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|item| strings(item, into)),
        serde_json::Value::Object(map) => map.values().for_each(|item| strings(item, into)),
        _ => {}
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Compiles the overlay source `source_path` of the tree `source` and
/// proves it against the base tree `base_name` of `base` (the kernel
/// build's device trees). `tools` is a host tools tree with `bin/dtc` and
/// `bin/fdtoverlay`; `contract_path` in `contract` states the compiler's
/// arguments; `stated_path` in `source` is the JSON the generator wrote
/// about the overlay (empty for none).
#[node(
    id = "gaia.devicetree.overlay",
    inputs(
        store,
        tools,
        contract,
        contract_path,
        source,
        source_path,
        stated_path,
        base,
        base_name
    ),
    outputs(overlay)
)]
#[allow(clippy::too_many_arguments)]
pub fn devicetree_overlay(
    store: &str,
    tools: Tree,
    contract: Tree,
    contract_path: &str,
    source: Tree,
    source_path: &str,
    stated_path: &str,
    base: &Artifact,
    base_name: &str,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let in_store = |tree: &Tree, what: &str| {
        tree.object
            .object_ref()
            .ok_or_else(|| fail(format!("{what} is not in the store")))
    };
    let tools_object = in_store(&tools, "the host tools")?;
    let contract_object = in_store(&contract, "the compiler contract's tree")?;
    let source_object = in_store(&source, "the overlay's source tree")?;
    let base_object = base
        .object
        .object_ref()
        .ok_or_else(|| fail("the base device trees are not in the store".into()))?;
    let tool = |name: &str| -> Result<std::path::PathBuf, StepError> {
        // A host tools tree, or the seed of one (its `host/` directory).
        let root = store.path(&tools_object);
        let found = [root.join("bin"), root.join("host/bin")]
            .into_iter()
            .map(|dir| dir.join(name))
            .find(|path| path.is_file());
        if let Some(path) = found {
            Ok(path)
        } else {
            Err(fail(format!(
                "the host tools have no bin/{name}; the image's build makes it with its device tree compiler package"
            )))
        }
    };
    let (dtc, fdtoverlay) = (tool("dtc")?, tool("fdtoverlay")?);
    let contract_rel = inside(contract_path).map_err(fail)?;
    let contract_text = std::fs::read_to_string(store.path(&contract_object).join(&contract_rel))
        .map_err(|e| fail(format!("the compiler contract {contract_rel}: {e}")))?;
    let (args, minimum) = compiler(&contract_text, &contract_rel).map_err(fail)?;
    let source_rel = inside(source_path).map_err(fail)?;
    let source_file = store.path(&source_object).join(&source_rel);
    let source_bytes = std::fs::read(&source_file)
        .map_err(|e| fail(format!("the overlay source {source_rel}: {e}")))?;
    let stated: Vec<String> = if stated_path.is_empty() {
        Vec::new()
    } else {
        let rel = inside(stated_path).map_err(fail)?;
        let text = std::fs::read_to_string(store.path(&source_object).join(&rel))
            .map_err(|e| fail(format!("what the generator states, {rel}: {e}")))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| fail(format!("what the generator states, {rel}: {e}")))?;
        let mut found = Vec::new();
        strings(&value, &mut found);
        found
    };
    let base_rel = inside(base_name).map_err(fail)?;
    let base_file = store.path(&base_object).join(&base_rel);
    if !base_file.is_file() {
        return Err(fail(format!(
            "the kernel's build made no device tree {base_rel}"
        )));
    }
    let base_digest = gaia_step::Digest::of_bytes(
        &std::fs::read(&base_file).map_err(|e| fail(format!("{base_rel}: {e}")))?,
    );

    // The files read, not the trees they are in: another file of the
    // generator's output or another device tree is no new step.
    let key = KeyBuilder::new(NAME, 1)
        .input("tools", tools_object.digest)
        .config("args", args.join(" "))
        .config(
            "minimum",
            minimum
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("."),
        )
        .input("source", gaia_step::Digest::of_bytes(&source_bytes))
        .config("stated", stated.join("\n"))
        .input("base", base_digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("overlay", Path::new(OUTPUT))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let dir = work.dir().to_path_buf();
            let run = |program: &Path| Tool::new(program).env("LC_ALL", "C").cwd(&dir);
            // No older than the contract's minimum.
            let said = work.run(&run(&dtc).arg("--version"))?.stdout_tail.join(" ");
            let have = version_of(&said).ok_or_else(|| {
                StepError::body(NAME, format!("dtc --version gave {said:?}, no version"))
            })?;
            if have < minimum {
                return Err(StepError::body(
                    NAME,
                    format!(
                        "the device tree compiler is {said}, older than the {} the overlay's owner checks with",
                        minimum
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(".")
                    ),
                ));
            }
            let out = dir.join(OUTPUT);
            work.run(
                &run(&dtc)
                    .args(args.clone())
                    .arg("-o")
                    .arg(&out)
                    .arg(&source_file),
            )?;
            // Applied to the tree the image ships; the result is not kept.
            let applied = dir.join("applied.dtb");
            work.run(
                &run(&fdtoverlay)
                    .arg("-i")
                    .arg(&base_file)
                    .arg("-o")
                    .arg(&applied)
                    .arg(&out),
            )?;
            std::fs::remove_file(&applied).map_err(io)?;
            let compiled = std::fs::read(&out).map_err(io)?;
            let lost: Vec<&String> = stated
                .iter()
                .filter(|value| {
                    let mut property = value.as_bytes().to_vec();
                    property.push(0);
                    !contains(&compiled, &property)
                })
                .collect();
            if !lost.is_empty() {
                return Err(StepError::body(
                    NAME,
                    format!(
                        "the compiled overlay does not hold what the generator states about it:\n  {}",
                        lost.iter()
                            .map(|value| value.as_str())
                            .collect::<Vec<_>>()
                            .join("\n  ")
                    ),
                ));
            }
            Ok(())
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::DeviceTree,
        object: made.outputs["overlay"].into(),
    })
}
