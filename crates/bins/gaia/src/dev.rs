//! `gaia build --dev-reuse`: what makes a dev build one, on this side of
//! the steps. The reuse itself is the Buildroot node's (`gaia-os-buildroot`,
//! module `dev`).
//!
//! A dev build is for bench iteration. **Release means a clean build:** a
//! release path, when one is built, builds clean and refuses a run record
//! that says `"build": "dev-reuse"`. A dev image's bytes carry no mark, so
//! what keeps it from a release is:
//!
//! - **Refused at plan time:** a step that signs with a key (`gaia.sign`, or
//!   any step given a signing key's variable, `key_env`: a signed first-flash
//!   image, a signed bundle). Gaia has no publish step; one added later is to
//!   be refused here too. And a dev EROFS image ([`dev_erofs`]: lz4hc, for
//!   the bench) that a disk, a flash image or a signature would carry.
//! - **Marked:** the run record (`"build": "dev-reuse"`, and `dev_erofs`
//!   naming each dev EROFS step); each generator step's `@build@`
//!   ([`mark_generators`]), for a tool that writes it into what the board
//!   carries (a slot's manifest); each output copied
//!   to `<store>/dev-outputs/<run>/<output>` with `<output>.gaia-build.json`
//!   beside it, `{"build":"dev-reuse"}`, and that copy is the path printed
//!   (a store object may be a clean build's too, so nothing is put beside
//!   it); and the build's last line on stderr, `DEV BUILD — not for release`.
//! - **Checked on demand:** `--verify` builds the same product clean after
//!   the dev build and compares each output, and each Buildroot step's
//!   outputs that ship, file by file. Run it before trusting a dev image for
//!   anything but the bench. Two clean builds of the same inputs are equal
//!   in all of these, so any difference is a stale reuse.
use crate::Stop;
use crate::record::Run;
use daedalus::layers::graph::{GraphDoc, Lit, NodeDoc};
use gaia_store::{Digest, ObjectRef, Store};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The run record's `build` for a dev build.
pub const BUILD: &str = gaia_types::BUILD_DEV_REUSE;
/// Where the warm Buildroot output directories are, beside the store's
/// objects.
pub const WARM: &str = "dev-buildroot";
/// Where a dev build's outputs are copied, beside the store's objects.
pub const OUTPUTS: &str = "dev-outputs";
/// The node that signs.
const SIGN: &str = "gaia.sign";
/// The input that names a signing key's variable.
const KEY_ENV: &str = "key_env";
/// A Buildroot step's outputs that never reach an image: its resolved
/// config, and the kernel build tree and toolchain later steps build with.
/// What those steps make from them (a module, a program) is an output of
/// theirs, compared where it ships; the host programs inside them (the
/// kernel's own build tools, the device-tree compiler) run only on the build
/// machine.
const BUILD_SIDE: [&str; 3] = ["config", "kernel-build", "toolchain"];

/// Whether a Buildroot step's output `name` is part of what ships.
fn ships(name: &str) -> bool {
    !BUILD_SIDE.contains(&name)
}

/// The steps of a graph document (`doc`, as JSON) a dev build refuses: each
/// node that signs, by its label.
pub fn refused(doc: &serde_json::Value) -> Vec<String> {
    let mut refused = Vec::new();
    for node in doc["graph"]["nodes"].as_array().into_iter().flatten() {
        let label = node["label"]
            .as_str()
            .or_else(|| node["id"].as_str())
            .unwrap_or("?");
        let keyed = node["consts"][KEY_ENV]["value"]
            .as_str()
            .is_some_and(|value| !value.is_empty());
        if node["id"].as_str() == Some(SIGN) {
            refused.push(format!("{label} (signs)"));
        } else if keyed {
            refused.push(format!("{label} (signs with a key)"));
        }
    }
    refused
}

/// The refusal of a dev build of a graph that signs.
pub fn check(doc: &serde_json::Value) -> Result<(), Stop> {
    let refused = refused(doc);
    if refused.is_empty() {
        return Ok(());
    }
    Err(Stop::usage(format!(
        "--dev-reuse builds are not signed or published, and this product's graph signs:\n  {}\n\
         Build it clean, or give the dev build a product that does not sign.",
        refused.join("\n  ")
    )))
}

/// The node that makes an EROFS image.
const EROFS: &str = "gaia.fs.erofs";
/// What a dev erofs is compressed with: about 15 times faster to make than
/// the release lzma, at 1.8 times the size (1.3 s against
/// 18-23 s, 30.1 MB against 17.0 MB, on a 68 MB root).
pub const DEV_COMPRESSION: &str = "lz4hc";

/// Whether node `node` makes what leaves the bench: a disk or a flash image,
/// or a signature.
fn leaves_the_bench(node: &NodeDoc) -> bool {
    let keyed = matches!(node.consts.get(KEY_ENV), Some(Lit::Str(value)) if !value.is_empty());
    node.id.starts_with("gaia.disk")
        || node.id.starts_with("gaia.flash")
        || node.id == SIGN
        || keyed
}

/// The first step that leaves the bench (by [`leaves_the_bench`]) that
/// `from` reaches, following the graph's edges: what would carry its output.
fn carried_by(doc: &GraphDoc, from: usize) -> Option<usize> {
    let nodes = &doc.graph.nodes;
    let mut seen = vec![false; nodes.len()];
    let mut todo = vec![from];
    while let Some(node) = todo.pop() {
        if std::mem::replace(&mut seen[node], true) {
            continue;
        }
        if node != from && leaves_the_bench(&nodes[node]) {
            return Some(node);
        }
        for edge in &doc.graph.edges {
            if edge.from.node as usize == node && edge.to.node != 0 {
                todo.push(edge.to.node as usize);
            }
        }
    }
    None
}

fn label(doc: &GraphDoc, node: usize) -> String {
    let node = &doc.graph.nodes[node];
    node.label.clone().unwrap_or_else(|| node.id.clone())
}

/// The EROFS images of a dev build.
#[derive(Debug, Default, PartialEq)]
pub struct DevErofs {
    /// The steps made dev ([`DEV_COMPRESSION`]), by label.
    pub made: Vec<String>,
    /// A line for each EROFS step that keeps its compression, and why.
    pub notes: Vec<String>,
}

/// A dev build's EROFS images (`gaia build --dev-reuse`): each EROFS step
/// whose image reaches no disk, flash image or signature is made with
/// [`DEV_COMPRESSION`], at the compressor's default level, without lzma's
/// dictionary or `fragments` (the slow packing). The compression is in the
/// step's key, so a dev image never shares one with a release image. An
/// EROFS step that does reach one keeps what it was given: a dev image never
/// leaves the bench. To get a dev image from a product that makes a disk,
/// build it `--until` the EROFS step.
pub fn dev_erofs(doc: &mut GraphDoc) -> DevErofs {
    let mut dev = DevErofs::default();
    for node in 1..doc.graph.nodes.len() {
        if doc.graph.nodes[node].id != EROFS {
            continue;
        }
        let name = label(doc, node);
        if let Some(by) = carried_by(doc, node) {
            dev.notes.push(format!(
                "dev erofs: {name} is not one: it reaches {}, and a dev erofs never leaves the bench (build --until {name} for one)",
                label(doc, by)
            ));
            continue;
        }
        let consts = &mut doc.graph.nodes[node].consts;
        consts.insert("compression".into(), Lit::Str(DEV_COMPRESSION.into()));
        consts.insert("level".into(), Lit::Int(0));
        consts.remove("dictsize");
        consts.remove("fragments");
        dev.notes
            .push(format!("dev erofs: {name} is made with {DEV_COMPRESSION}"));
        dev.made.push(name);
    }
    dev
}

/// The generator nodes, whose runs may name `@build@`.
const GENERATORS: [&str; 3] = ["gaia.generate", "gaia.generate.image", "gaia.generate.tree"];

/// A dev build's generator steps are told so: each is given `build` =
/// [`BUILD`], which its runs see as `@build@` (a clean build gives none:
/// `clean`). A step is keyed on it only when a run names it.
pub fn mark_generators(doc: &mut GraphDoc) {
    for node in &mut doc.graph.nodes {
        if GENERATORS.contains(&node.id.as_str()) {
            node.consts.insert("build".into(), Lit::Str(BUILD.into()));
        }
    }
}

/// The refusal of a dev build in which a dev EROFS image (`made`, by label)
/// would leave the bench: carried by a disk, a flash image or a signature.
/// Checked on the graph as it will run, apart from [`dev_erofs`]'s own test.
pub fn check_erofs(doc: &GraphDoc, made: &[String]) -> Result<(), Stop> {
    let mut refused = Vec::new();
    for node in 1..doc.graph.nodes.len() {
        if made.contains(&label(doc, node))
            && let Some(by) = carried_by(doc, node)
        {
            refused.push(format!("{} reaches {}", label(doc, node), label(doc, by)));
        }
    }
    if refused.is_empty() {
        return Ok(());
    }
    Err(Stop::usage(format!(
        "a dev erofs ({DEV_COMPRESSION}, --dev-reuse) never goes into a disk, a flash image or a signature:\n  {}",
        refused.join("\n  ")
    )))
}

/// Copies each output of `run` to `<store>/dev-outputs/<run>/<output>` with
/// its marker beside it. Returns the copies' paths, by output.
pub fn mark_outputs(
    store: &Store,
    run: &Run,
    record: &Path,
) -> Result<BTreeMap<String, PathBuf>, Stop> {
    let run_name = record
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = store.root().join(OUTPUTS).join(run_name);
    let fail = |e: io::Error| Stop::usage(format!("{}: {e}", dir.display()));
    fs::create_dir_all(&dir).map_err(fail)?;
    let marker = format!(
        "{{\"build\":\"{BUILD}\",\"record\":\"{}\"}}\n",
        record.display()
    );
    let mut copies = BTreeMap::new();
    for (port, output) in &run.outputs {
        let copy = dir.join(port);
        copy_any(&store.path(&output.object), &copy).map_err(fail)?;
        fs::write(dir.join(format!("{port}.gaia-build.json")), &marker).map_err(fail)?;
        copies.insert(port.clone(), copy);
    }
    Ok(copies)
}

/// A copy of a file or a tree; on the store's disk, a reflink where the
/// filesystem has them.
fn copy_any(from: &Path, to: &Path) -> io::Result<()> {
    let status = std::process::Command::new("cp")
        .args(["-a", "--reflink=auto", "--"])
        .arg(from)
        .arg(to)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("cp {} failed", from.display())))
    }
}

/// What differs between a dev build and a clean build of the same product:
/// each output, and each Buildroot step's outputs that ship (paired by the
/// config they resolved), file by file. One line each; empty when they are
/// equal.
pub fn compare(store: &Store, dev: &Run, clean: &Run) -> Vec<String> {
    let mut lines = Vec::new();
    for (port, output) in &dev.outputs {
        match clean.outputs.get(port) {
            None => lines.push(format!("{port}: only in the dev build")),
            Some(other) => {
                for diff in differ(store, &output.object, &other.object) {
                    lines.push(format!("{port}: {diff}"));
                }
            }
        }
    }
    let by_config = |run: &Run| -> BTreeMap<ObjectRef, BTreeMap<String, ObjectRef>> {
        run.steps
            .iter()
            .filter(|s| s.step == "gaia.os.buildroot")
            .filter_map(|s| s.outputs.get("config").map(|c| (*c, s.outputs.clone())))
            .collect()
    };
    let clean_steps = by_config(clean);
    for (config, outputs) in by_config(dev) {
        let Some(other) = clean_steps.get(&config) else {
            lines.push(format!(
                "a Buildroot step with config {config} has no clean twin"
            ));
            continue;
        };
        for (name, object) in outputs.iter().filter(|(name, _)| ships(name)) {
            if let Some(twin) = other.get(name) {
                for diff in differ(store, object, twin) {
                    lines.push(format!("gaia.os.buildroot {name}: {diff}"));
                }
            }
        }
    }
    lines
}

/// The files that differ between two objects.
fn differ(store: &Store, a: &ObjectRef, b: &ObjectRef) -> Vec<String> {
    if a == b {
        return Vec::new();
    }
    let (left, right) = (listing(&store.path(a)), listing(&store.path(b)));
    let mut lines = Vec::new();
    for (path, what) in &left {
        match right.get(path) {
            None => lines.push(format!("{path}: only in the dev build")),
            Some(other) if other != what => lines.push(format!("{path}: differs")),
            Some(_) => {}
        }
    }
    for path in right.keys().filter(|p| !left.contains_key(*p)) {
        lines.push(format!("{path}: only in the clean build"));
    }
    if lines.is_empty() {
        lines.push("differs".into());
    }
    lines
}

/// Each file, link and directory under `root` (or `root` itself, a file),
/// with its mode and its content's digest or its link's target.
fn listing(root: &Path) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut todo = vec![PathBuf::new()];
    while let Some(relative) = todo.pop() {
        let path = root.join(&relative);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        let name = if relative.as_os_str().is_empty() {
            ".".to_string()
        } else {
            relative.display().to_string()
        };
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap_or_default();
            found.insert(name, format!("link {}", target.display()));
        } else if meta.is_dir() {
            found.insert(name, format!("dir {mode:o}"));
            for entry in fs::read_dir(&path).into_iter().flatten().flatten() {
                todo.push(relative.join(entry.file_name()));
            }
        } else {
            let digest = fs::read(&path)
                .map(|bytes| Digest::of_bytes(&bytes).hex())
                .unwrap_or_else(|e| format!("unreadable: {e}"));
            found.insert(name, format!("file {mode:o} {digest}"));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_steps_are_refused() {
        let doc = serde_json::json!({"graph": {"nodes": [
            {"id": "host"},
            {"id": "gaia.disk", "label": "p::disk", "consts": {}},
            {"id": "gaia.flash.image", "label": "p::flash", "consts": {"key_env": {"value": ""}}},
            {"id": "gaia.generate.image", "label": "p::bundle", "consts": {"key_env": {"value": "SEED"}}},
            {"id": "gaia.sign", "label": "p::sign", "consts": {}},
        ]}});
        assert_eq!(
            refused(&doc),
            ["p::bundle (signs with a key)", "p::sign (signs)"]
        );
        assert!(check(&doc).is_err());
        let unsigned = serde_json::json!({"graph": {"nodes": [
            {"id": "gaia.flash.image", "label": "p::flash", "consts": {}},
        ]}});
        assert!(check(&unsigned).is_ok());
    }

    /// A graph document of `nodes` (id, label, consts as JSON) and `edges`
    /// (from node, to node); node 0 is the host.
    fn doc(nodes: &[(&str, &str, serde_json::Value)], edges: &[(u32, u32)]) -> GraphDoc {
        let mut all = vec![serde_json::json!({"id": "host", "kind": "host_bridge"})];
        all.extend(nodes.iter().map(
            |(id, label, consts)| serde_json::json!({"id": id, "label": label, "consts": consts}),
        ));
        let edges: Vec<_> = edges
            .iter()
            .map(|(from, to)| {
                serde_json::json!({"from": {"node": from, "port": "out"}, "to": {"node": to, "port": "in"}, "policy": {"kind": "fifo"}})
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "format": "daedalus.graph", "schema_version": 1, "requires": [],
            "graph": {"nodes": all, "edges": edges},
        }))
        .expect("a graph document")
    }

    /// A dev build gives each generator step `build` = dev-reuse, for its
    /// runs' `@build@`; no other step is given it.
    #[test]
    fn generator_steps_are_told_the_build() {
        let mut d = doc(
            &[
                ("gaia.generate", "p::gen", serde_json::json!({})),
                ("gaia.generate.image", "p::bundle", serde_json::json!({})),
                ("gaia.generate.tree", "p::data", serde_json::json!({})),
                ("gaia.fs.erofs", "p::erofs", serde_json::json!({})),
            ],
            &[],
        );
        mark_generators(&mut d);
        let build = |node: usize| match d.graph.nodes[node].consts.get("build") {
            Some(Lit::Str(value)) => value.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(
            [build(1), build(2), build(3), build(4)],
            ["dev-reuse", "dev-reuse", "dev-reuse", "None"]
        );
    }

    fn release_erofs() -> serde_json::Value {
        serde_json::json!({
            "compression": {"type": "str", "value": "lzma"},
            "level": {"type": "int", "value": 6},
            "dictsize": {"type": "int", "value": 2097152},
            "cluster": {"type": "int", "value": 1048576},
            "fragments": {"type": "bool", "value": true},
            "ztailpacking": {"type": "bool", "value": true},
        })
    }

    /// An EROFS image that reaches no disk, flash image or signature is made
    /// dev: lz4hc at the default level, no dictionary, no fragments, the
    /// cluster and tail packing as given. One that reaches a disk (through
    /// another step) keeps lzma and says why.
    #[test]
    fn only_an_erofs_that_stays_on_the_bench_is_made_dev() {
        let mut graph = doc(
            &[
                ("gaia.fs.erofs", "p::slot", release_erofs()),
                ("gaia.verity", "p::verity", serde_json::json!({})),
                ("gaia.fs.erofs", "q::root", release_erofs()),
                ("gaia.verity", "q::verity", serde_json::json!({})),
                ("gaia.disk", "q::disk", serde_json::json!({})),
            ],
            &[(1, 2), (2, 0), (3, 4), (4, 5), (5, 0)],
        );
        let dev = dev_erofs(&mut graph);
        assert_eq!(dev.made, ["p::slot"]);
        assert_eq!(
            dev.notes,
            [
                "dev erofs: p::slot is made with lz4hc",
                "dev erofs: q::root is not one: it reaches q::disk, and a dev erofs never leaves the bench (build --until q::root for one)",
            ]
        );
        let consts =
            |node: usize| serde_json::to_value(&graph.graph.nodes[node].consts).expect("consts");
        assert_eq!(
            consts(1),
            serde_json::json!({
                "compression": {"type": "str", "value": "lz4hc"},
                "level": {"type": "int", "value": 0},
                "cluster": {"type": "int", "value": 1048576},
                "ztailpacking": {"type": "bool", "value": true},
            })
        );
        assert_eq!(consts(3), release_erofs());
        assert!(check_erofs(&graph, &dev.made).is_ok());
    }

    /// The check stands apart from the choice: a dev EROFS image that a
    /// disk, a flash image or a signing step would carry is refused, each
    /// named with what carries it.
    #[test]
    fn a_dev_erofs_that_would_leave_the_bench_is_refused() {
        for (id, consts) in [
            ("gaia.disk", serde_json::json!({})),
            ("gaia.flash.image", serde_json::json!({})),
            ("gaia.sign", serde_json::json!({})),
            (
                "gaia.generate.image",
                serde_json::json!({"key_env": {"type": "str", "value": "SEED"}}),
            ),
        ] {
            let graph = doc(
                &[
                    ("gaia.fs.erofs", "p::slot", release_erofs()),
                    ("gaia.verity", "p::verity", serde_json::json!({})),
                    (id, "p::out", consts),
                ],
                &[(1, 2), (2, 3), (3, 0)],
            );
            let refused = check_erofs(&graph, &["p::slot".to_string()])
                .expect_err(id)
                .message;
            assert!(
                refused.contains("p::slot reaches p::out"),
                "{id}: {refused}"
            );
            let unsigned = doc(
                &[
                    ("gaia.fs.erofs", "p::slot", release_erofs()),
                    (
                        "gaia.generate.image",
                        "p::out",
                        serde_json::json!({"key_env": {"type": "str", "value": ""}}),
                    ),
                ],
                &[(1, 2), (2, 0)],
            );
            assert!(check_erofs(&unsigned, &["p::slot".to_string()]).is_ok());
        }
    }

    #[test]
    fn only_what_ships_is_compared() {
        for name in [
            "rootfs",
            "rootfs-entries",
            "kernel",
            "modules",
            "device-trees",
        ] {
            assert!(ships(name), "{name}");
        }
        for name in ["bootloader", "images"] {
            assert!(ships(name), "{name}");
        }
        for name in ["config", "kernel-build", "toolchain"] {
            assert!(!ships(name), "{name}");
        }
    }

    #[test]
    fn listings_name_what_differs() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let (a, b) = (scratch.path().join("a"), scratch.path().join("b"));
        for dir in [&a, &b] {
            fs::create_dir_all(dir.join("etc")).expect("dir");
            fs::write(dir.join("etc/same"), "x").expect("file");
        }
        fs::write(a.join("etc/changed"), "1").expect("file");
        fs::write(b.join("etc/changed"), "2").expect("file");
        fs::write(a.join("stale"), "").expect("file");
        let (left, right) = (listing(&a), listing(&b));
        assert_eq!(left["etc/same"], right["etc/same"]);
        assert_ne!(left["etc/changed"], right["etc/changed"]);
        assert!(!right.contains_key("stale"));
    }
}
