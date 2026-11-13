//! Descriptions to a graph.
//!
//! A product (with its boards and components), its lock and the recipes
//! become one Daedalus graph document, which the planner checks before
//! anything runs.
//!
//! - **Sources:** one node per component source an image uses, labelled
//!   `<component>.<source>`, pinned by the lock: `gaia.source.git` at the
//!   locked commit (with the source's `subdir` as its `subdir` setting, when
//!   it names one), `gaia.source.download` and `gaia.source.oci` at the
//!   locked digest, `gaia.source.path` for a path.
//! - **Builds:** one node per built component, labelled
//!   `<component>.build`, its id from the [`Builders`] table. Its `source`
//!   input is the source the build names; any other input named like one of
//!   the component's sources is wired to it; its settings are constants on
//!   inputs of those names. An input `triple` or `cpu` the settings leave
//!   out is the board's target (static musl for Linux), so one component
//!   file serves every board, and a component is built once per board. An
//!   input `vendor` with no source of that name is filled by the builder's
//!   own vendor step, `<component>.vendor` (the node `<builder's id>.vendor`,
//!   when there is one), from the source the build names.
//! - **Images:** the image's recipe (F1), with the image's settings as its
//!   parameters, laid out inline: each stage a node labelled
//!   `<image>::<stage>`. The recipe input `build` takes the image's built
//!   component, and `source` the one source of the image's one component.
//!   The recipe input `control` takes a host input, `<image>.control`: the
//!   [`Control`](gaia_types::Control) made from the `[package]` table of the
//!   image's one component that has one, and `disk` the board's disk
//!   layout ([`HostInput`]). Each recipe output
//!   goes to the host as `<image>.<output>`.
//! - **OS:** an image with `os` gets a `gaia.os.buildroot` step,
//!   `<image>.os`, built from the product's source with a config the board
//!   gives by rule and the image's own fragments (see `os`). It is fed on
//!   `<image>.os` and the shared `buildroot-host`; the trees the
//!   config names are its fan-in inputs. The recipe input `build` is then
//!   `<image>.rootfs`: the OS's tree, then each component's layer (its
//!   install entries placed, `gaia.tree.place`, or its whole build output),
//!   merged by `gaia.tree.merge`. The recipe input `system` is the same
//!   tree without the kernel's image and device trees under /boot, for an
//!   output that is not booted from (a slot whose kernel is read elsewhere). The OS's boot files (Buildroot's
//!   `images/`) are four recipe inputs, by what each file is: `kernel`,
//!   `device-trees`, `bootloader`, and `os` for the rest.
//! - **Raw areas:** the recipe input `blobs` is the board's areas outside
//!   the partition table that name their `content`: each a `gaia.disk.blob`,
//!   `<image>.blob.<name>`, picking that file from the OS's bootloader
//!   output, collected by `<image>.blobs` (`gaia.disk.blobs`).
//! - **Data:** the recipe input `data` is the image's one built data
//!   component. With none, it is left out when every port it names is
//!   optional, and an error otherwise.
//! - **Everywhere:** an input named `store` gets the store, and one named
//!   like a [`Setup`] tool gets that tool's path. Every node has
//!   `daedalus.node.fusion: false`, so fail-fast and cancel stop between
//!   steps, and `gaia.step` naming its step.
//!
//! An input nothing fills, a setting that is not an input, a source the lock
//! does not pin, or a plan the planner refuses is an error naming the
//! description it came from.

mod error;
mod os;
mod wire;

pub use error::Error;
pub use wire::Origin;

use daedalus::layers::graph::{GraphBody, GraphDoc, Requirement};
use daedalus::prelude::{Config, Planned, Registry, plan};
use daedalus_cpu::CpuBackend;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The node metadata key that keeps a node a step of its own.
pub const FUSION: &str = "daedalus.node.fusion";
/// The node metadata key holding the step's name.
pub const STEP: &str = "gaia.step";

/// What the host gives every step: the store and the pinned tools, by the
/// input name nodes use for them (`git`, `curl`, `bwrap`, `toolchain`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Setup {
    pub store: PathBuf,
    pub tools: BTreeMap<String, String>,
    /// make, CPUs, jobs and the download cache, for an image with an OS.
    pub buildroot: Option<gaia_types::BuildrootHost>,
}

/// A component's `builder` name to the node that builds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Builders(pub BTreeMap<String, String>);

impl Default for Builders {
    fn default() -> Self {
        Self(
            [
                ("rust", "gaia.build.rust"),
                ("command", "gaia.build.command"),
            ]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        )
    }
}

/// The graph document, and the description each node came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    pub doc: GraphDoc,
    /// By node index.
    pub origins: Vec<Origin>,
    /// The values the host feeds before the run, by host input port.
    pub inputs: BTreeMap<String, HostInput>,
}

/// A structured value from the descriptions that the host feeds into the
/// graph, because a graph constant cannot be a struct yet (outside block
/// X7 in `docs/design.md`). Interim: when X7 lands each becomes a constant
/// in the graph, so a saved graph document is complete on its own.
///
/// The node that takes it keys on the whole value, so a changed field is a
/// new step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostInput {
    /// A package's control data, from a component's `[package]` table.
    Control(gaia_types::Control),
    /// A disk's table and the board's own partitions, from the board's
    /// `[disk]` table and `[[partitions]]`.
    Disk(gaia_types::DiskLayout),
    /// An image's OS config: the board's fragments by rule, then the
    /// image's own, with the input trees they name.
    Os(gaia_types::OsConfig),
    /// The host side of every Buildroot run, from [`Setup`].
    Buildroot(gaia_types::BuildrootHost),
    /// A component's install entries from one tree.
    Placements(gaia_types::Placements),
}

impl HostInput {
    /// The Daedalus type key of the value.
    pub fn type_key(&self) -> &'static str {
        match self {
            Self::Control(_) => "gaia:package-control",
            Self::Disk(_) => "gaia:disk-layout",
            Self::Os(_) => "gaia:os-config",
            Self::Buildroot(_) => "gaia:buildroot-host",
            Self::Placements(_) => "gaia:placements",
        }
    }

    /// The value in one line, for `gaia plan`.
    pub fn summary(&self) -> String {
        match self {
            Self::Control(c) => {
                let mut line = format!("{} {} {}", c.package, c.version, c.architecture);
                if !c.depends.is_empty() {
                    line.push_str(&format!(", depends {}", c.depends.join(", ")));
                }
                line
            }
            Self::Disk(d) => {
                let scheme = match d.table.scheme {
                    gaia_types::TableScheme::Gpt => "gpt",
                    gaia_types::TableScheme::Mbr => "mbr",
                };
                let mut line = scheme.to_string();
                for (what, size) in [
                    ("first", d.table.first),
                    ("alignment", d.table.alignment),
                    ("size", d.table.size),
                ] {
                    if let Some(size) = size {
                        line.push_str(&format!(", {what} {}", size.0));
                    }
                }
                let names: Vec<&str> = d.partitions.iter().map(|p| p.name.as_str()).collect();
                if !names.is_empty() {
                    line.push_str(&format!(", board partitions {}", names.join(" ")));
                }
                line
            }
            Self::Os(c) => {
                let names: Vec<&str> = c.fragments.iter().map(|f| f.name.as_str()).collect();
                let mut line = format!("fragments {}", names.join(" "));
                if !c.inputs.is_empty() {
                    line.push_str(&format!(", inputs {}", c.inputs.join(" ")));
                }
                let overrides: Vec<String> = c
                    .overrides
                    .iter()
                    .map(|o| format!("{}={}", o.package, o.input))
                    .collect();
                if !overrides.is_empty() {
                    line.push_str(&format!(", built from {}", overrides.join(" ")));
                }
                line
            }
            Self::Buildroot(h) => {
                let mut line = format!("{} -j{}", h.make, h.jobs);
                if !h.cpus.is_empty() {
                    line.push_str(&format!(" on CPUs {}", h.cpus));
                }
                line.push_str(&format!(", downloads {}", h.download_dir));
                line
            }
            Self::Placements(p) => {
                let to: Vec<&str> = p.entries.iter().map(|p| p.to.as_str()).collect();
                to.join(" ")
            }
        }
    }
}

/// Builds the graph for `loaded`, pinned by `lock`, with `recipes`, from
/// the nodes in `registry`.
pub fn build(
    loaded: &gaia_describe::Loaded,
    lock: &gaia_lock::Lock,
    recipes: &gaia_recipe::Recipes,
    registry: &Registry,
    setup: &Setup,
    builders: &Builders,
) -> Result<Built, Error> {
    let (body, origins, inputs) = wire::wire(loaded, lock, recipes, registry, setup, builders)?;
    let mut ids: Vec<String> = body
        .nodes
        .iter()
        .filter(|n| n.kind == daedalus::layers::graph::NodeKindDoc::Node)
        .map(|n| n.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    let mut doc = GraphDoc::new(body);
    doc.requires = ids
        .into_iter()
        .map(|id| Requirement { id, version: None })
        .collect();
    Ok(Built {
        doc,
        origins,
        inputs,
    })
}

impl Built {
    /// Plans the graph with the CPU backend. A planner error names the
    /// descriptions of the nodes it names.
    pub fn plan(&self, registry: &Registry) -> Result<Planned, Error> {
        plan(
            &self.doc.graph,
            registry,
            &[&CpuBackend],
            &Config::default(),
        )
        .map_err(|e| self.blame(&e.to_string()))
    }

    /// The graph body, for running.
    pub fn body(&self) -> &GraphBody {
        &self.doc.graph
    }

    /// `message`, with the files of the nodes it names (by label, or by id
    /// when one node has it).
    fn blame(&self, message: &str) -> Error {
        let mut files: Vec<PathBuf> = Vec::new();
        for (i, origin) in self.origins.iter().enumerate() {
            let node = &self.doc.graph.nodes[i];
            let unique_id = self
                .doc
                .graph
                .nodes
                .iter()
                .filter(|n| n.id == node.id)
                .count()
                == 1;
            // Daedalus names nodes by id: when several share it, each is
            // named.
            let _ = unique_id;
            let named = message.contains(&format!("`{}`", origin.label))
                || message.contains(&format!("`{}`", node.id));
            if named && !origin.file.as_os_str().is_empty() && !files.contains(&origin.file) {
                files.push(origin.file.clone());
            }
        }
        Error::plan(files, message)
    }
}
