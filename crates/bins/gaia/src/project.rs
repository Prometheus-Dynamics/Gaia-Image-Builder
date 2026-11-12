//! A product, its lock and recipes, the registered nodes and the tools:
//! everything `plan` and `build` read, made into one checked graph.

use crate::{Stop, args};
use daedalus::layers::planner::explain::explain_plan;
use daedalus::prelude::{Planned, Registry};
use gaia_graph::{Builders, Built, Setup};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Tools looked up on PATH when `--tool` doesn't name them: the input name,
/// and the program.
const PATH_TOOLS: &[(&str, &str)] = &[
    ("git", "git"),
    ("curl", "curl"),
    ("bwrap", "bwrap"),
    ("mke2fs", "mke2fs"),
    ("mkfs_erofs", "mkfs.erofs"),
];

pub struct Project {
    pub product: PathBuf,
    pub setup: Setup,
    pub registry: Registry,
    pub built: Built,
}

impl Project {
    pub fn open(options: &args::Project) -> Result<Self, Stop> {
        let product = &options.product;
        let dir = product.parent().unwrap_or(Path::new("."));
        let loaded = gaia_describe::Product::load(product).map_err(Stop::usage)?;
        let lock_path = options
            .lock
            .clone()
            .unwrap_or_else(|| dir.join("gaia.lock"));
        let lock = gaia_lock::Lock::read(&lock_path)
            .map_err(|e| Stop::usage(format!("{}: {e}", lock_path.display())))?;
        let recipes_dir = options
            .recipes
            .clone()
            .unwrap_or_else(|| dir.join("recipes"));
        let recipes = if recipes_dir.is_dir() || options.recipes.is_some() {
            gaia_recipe::Recipes::load_dir(&recipes_dir).map_err(Stop::usage)?
        } else {
            gaia_recipe::Recipes::default()
        };
        let setup = Setup {
            store: absolute(&store_dir(options.store.as_deref())?),
            tools: tools(&options.tools),
        };
        let registry = registry();
        let built = gaia_graph::build(&loaded, &lock, &recipes, &registry, &setup, &builders())
            .map_err(Stop::usage)?;
        Ok(Self {
            product: product.clone(),
            setup,
            registry,
            built,
        })
    }

    /// The plan, or the planner's error naming the descriptions.
    pub fn plan(&self) -> Result<Planned, Stop> {
        self.built.plan(&self.registry).map_err(Stop::usage)
    }

    /// The node's label, by graph index.
    pub fn label(&self, node: usize) -> &str {
        self.built.doc.graph.nodes[node]
            .label
            .as_deref()
            .unwrap_or("host")
    }

    /// `gaia plan`: the graph, the plan's steps and the outputs; nothing
    /// runs.
    pub fn print_plan(&self, json: bool) -> Result<(), Stop> {
        let planned = self.plan()?;
        let explain = explain_plan(&planned);
        if json {
            let both = serde_json::json!({
                "graph": serde_json::from_str::<serde_json::Value>(
                    &self.built.doc.to_json().map_err(Stop::usage)?
                ).map_err(Stop::usage)?,
                "plan": explain,
                "host_inputs": self.built.inputs.iter().map(|(port, input)| {
                    let value = match input {
                        gaia_graph::HostInput::Control(c) => serde_json::to_value(c),
                    };
                    value.map(|value| serde_json::json!({
                        "port": port,
                        "type": input.type_key(),
                        "value": value,
                    }))
                }).collect::<Result<Vec<_>, _>>().map_err(Stop::usage)?,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&both).map_err(Stop::usage)?
            );
            return Ok(());
        }
        let graph = &self.built.doc.graph;
        println!("product {}", self.product.display());
        println!("store   {}", self.setup.store.display());
        for (name, path) in &self.setup.tools {
            println!("tool    {name} = {path}");
        }
        println!(
            "\ngraph: {} nodes, {} edges",
            graph.nodes.len() - 1,
            graph.edges.len()
        );
        let width = (1..graph.nodes.len())
            .map(|i| self.label(i).len())
            .max()
            .unwrap_or(0);
        for (i, node) in graph.nodes.iter().enumerate().skip(1) {
            let file = self.built.origins[i].file.display().to_string();
            println!("  {:width$}  {:22} {file}", self.label(i), node.id);
        }
        println!("\nedges:");
        for edge in &graph.edges {
            println!(
                "  {}.{} -> {}.{}",
                self.label(edge.from.node as usize),
                edge.from.port,
                self.label(edge.to.node as usize),
                edge.to.port
            );
        }
        println!("\nplan: {} steps", explain.units.len());
        for (n, unit) in explain.units.iter().enumerate() {
            let covers: Vec<&str> = unit
                .covers
                .iter()
                .map(|&node| {
                    if (node as usize) < graph.nodes.len() {
                        self.label(node as usize)
                    } else {
                        // A node the planner inserted (a copy, say): its id.
                        explain
                            .nodes
                            .iter()
                            .find(|n| n.node == node)
                            .map_or("?", |n| n.id.as_str())
                    }
                })
                .collect();
            println!("  {:>3}  {}", n + 1, covers.join(" + "));
        }
        if !self.built.inputs.is_empty() {
            // Fed by `gaia build` before the run; graph constants once
            // Daedalus has struct constants (X7).
            println!("\nhost inputs:");
            for (port, input) in &self.built.inputs {
                println!("  {port}  {}  {}", input.type_key(), input.summary());
            }
        }
        println!("\noutputs:");
        for (port, ty) in &graph.nodes[0].port_types {
            if !self.built.inputs.contains_key(port) {
                println!("  {port}  {ty}");
            }
        }
        Ok(())
    }
}

/// Every node Gaia has. A crate under `crates/nodes` missing here fails
/// the `every_node_crate_is_registered` test.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    let all: &[&[_]] = &[
        &gaia_types::NODES,
        &gaia_source_git::NODES,
        &gaia_source_files::NODES,
        &gaia_source_oci::NODES,
        &gaia_build_rust::NODES,
        &gaia_build_command::NODES,
        &gaia_build_go::NODES,
        &gaia_build_java::NODES,
        &gaia_build_node::NODES,
        &gaia_build_python::NODES,
        &gaia_os_none::NODES,
        &gaia_os_import::NODES,
        &gaia_os_buildroot::NODES,
        &gaia_tree::NODES,
        &gaia_compress::NODES,
        &gaia_fs_ext4::NODES,
        &gaia_fs_erofs::NODES,
        &gaia_sign::NODES,
        &gaia_fs_vfat::NODES,
        &gaia_fs_ubi::NODES,
        &gaia_initramfs::NODES,
        &gaia_strip::NODES,
        &gaia_verity::NODES,
        &gaia_disk::NODES,
        &gaia_package::NODES,
        &gaia_check::NODES,
        &gaia_size::NODES,
        &gaia_report::NODES,
    ];
    for nodes in all {
        registry
            .register_all(nodes)
            .expect("Gaia's nodes have distinct ids");
    }
    registry
}

/// Every builder name a component may give.
fn builders() -> Builders {
    let mut builders = Builders::default();
    for name in ["go", "java", "node", "python"] {
        builders
            .0
            .insert(name.to_string(), format!("gaia.build.{name}"));
    }
    builders
}

/// The store: the flag, `$GAIA_STORE`, or the user's cache directory.
pub fn store_dir(flag: Option<&Path>) -> Result<PathBuf, Stop> {
    if let Some(dir) = flag {
        return Ok(dir.to_path_buf());
    }
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(dir) = env("GAIA_STORE") {
        return Ok(dir.into());
    }
    let cache = env("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or_else(|| Stop::usage("no store: give --store, or set GAIA_STORE or HOME"))?;
    Ok(cache.join("gaia").join("store"))
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The `--tool` paths, then known tools found on PATH, then rustc's
/// sysroot as `toolchain`.
fn tools(given: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut tools = given.clone();
    for (name, program) in PATH_TOOLS {
        if !tools.contains_key(*name)
            && let Some(path) = on_path(program)
        {
            tools.insert(name.to_string(), path.display().to_string());
        }
    }
    if !tools.contains_key("toolchain")
        && let Ok(out) = std::process::Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()
        && out.status.success()
    {
        let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sysroot.is_empty() {
            tools.insert("toolchain".into(), sysroot);
        }
    }
    tools
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    /// Every crate in `crates/nodes` is a dependency here, so its nodes can
    /// be in [`super::registry`].
    #[test]
    fn every_node_crate_is_registered() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest = std::fs::read_to_string(here.join("Cargo.toml")).expect("manifest");
        let source = std::fs::read_to_string(here.join("src/project.rs")).expect("source");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(here.join("../../nodes")).expect("nodes") {
            let dir = entry.expect("entry").path();
            let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
                continue;
            };
            let name = text
                .lines()
                .find_map(|l| l.strip_prefix("name = \""))
                .and_then(|l| l.strip_suffix('"'))
                .expect("a package name");
            let module = name.replace('-', "_");
            if !manifest.contains(&format!("\n{name} = "))
                || !source.contains(&format!("&{module}::NODES"))
            {
                missing.push(name.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "not registered in the gaia command: {missing:?}"
        );
    }
}
