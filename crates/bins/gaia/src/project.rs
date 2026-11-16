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
    ("debugfs", "debugfs"),
    ("mkfs_erofs", "mkfs.erofs"),
    ("patch", "patch"),
];

pub struct Project {
    pub product: PathBuf,
    pub setup: Setup,
    pub registry: Registry,
    pub built: Built,
    /// A dev build's EROFS steps made with the dev compression, by label
    /// (`dev::dev_erofs`); empty for a clean build.
    pub dev_erofs: Vec<String>,
}

impl Project {
    pub fn open(options: &args::Project) -> Result<Self, Stop> {
        let product = &options.product;
        let dir = product.parent().unwrap_or(Path::new("."));
        let mut loaded = gaia_describe::Product::load(product).map_err(Stop::usage)?;
        let lock_path = options
            .lock
            .clone()
            .unwrap_or_else(|| dir.join("gaia.lock"));
        let lock = gaia_lock::Lock::read(&lock_path)
            .map_err(|e| Stop::usage(format!("{}: {e}", lock_path.display())))?;
        let recipes_dir = recipes_dir(product, options.recipes.as_deref());
        let recipes = if recipes_dir.is_dir() || options.recipes.is_some() {
            gaia_recipe::Recipes::load_dir(&recipes_dir).map_err(Stop::usage)?
        } else {
            gaia_recipe::Recipes::default()
        };
        apply_settings(&mut loaded.product, product, &recipes, &options.settings)?;
        let store = absolute(&store_dir(options.store.as_deref())?);
        let tools = tools(&options.tools);
        let setup = Setup {
            buildroot: buildroot_host(&store, &tools, options.jobs).map(|mut host| {
                host.config_only = options.config_only;
                if options.dev_reuse {
                    host.dev_reuse = absolute(&store.join(crate::dev::WARM))
                        .display()
                        .to_string();
                }
                host
            }),
            store,
            tools,
            inputs: inputs(&options.inputs)?,
        };
        let registry = registry();
        let built = gaia_graph::build(&loaded, &lock, &recipes, &registry, &setup, &builders())
            .map_err(Stop::usage)?;
        // The configuration alone: only the step that resolves it, told to
        // stop there.
        let until = match (&options.until, options.config_only) {
            (Some(_), true) => {
                return Err(Stop::usage(
                    "--config-only and --until are one or the other",
                ));
            }
            (None, true) => Some("gaia.os.buildroot.host"),
            (step, false) => step.as_deref(),
        };
        let mut built = match until {
            Some(step) => built.until(step).map_err(Stop::usage)?,
            None => built,
        };
        // After `--until`: what is cut away carries no image.
        let mut dev_erofs = Vec::new();
        if options.dev_reuse {
            let dev = crate::dev::dev_erofs(&mut built.doc);
            crate::dev::mark_generators(&mut built.doc);
            built.notes.extend(dev.notes);
            dev_erofs = dev.made;
        }
        Ok(Self {
            product: product.clone(),
            setup,
            registry,
            built,
            dev_erofs,
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
                        gaia_graph::HostInput::Disk(d) => serde_json::to_value(d),
                        gaia_graph::HostInput::Os(c) => serde_json::to_value(c),
                        gaia_graph::HostInput::Buildroot(h) => serde_json::to_value(h),
                        gaia_graph::HostInput::Placements(p) => serde_json::to_value(p),
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
        for note in &self.built.notes {
            println!("{note}");
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
                // An OS config's fragments in full: what the OS step keys on.
                if let gaia_graph::HostInput::Os(config) = input {
                    for fragment in &config.fragments {
                        println!("    [{}]", fragment.name);
                        for line in fragment.text.lines().filter(|l| !l.trim().is_empty()) {
                            println!("      {line}");
                        }
                    }
                }
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
        &gaia_build_make::NODES,
        &gaia_build_java::NODES,
        &gaia_build_node::NODES,
        &gaia_build_python::NODES,
        &gaia_os_none::NODES,
        &gaia_os_import::NODES,
        &gaia_os_buildroot::NODES,
        &gaia_kernel_module::NODES,
        &gaia_os_buildroot_cache::NODES,
        &gaia_tree::NODES,
        &gaia_compress::NODES,
        &gaia_flash_image::NODES,
        &gaia_fs_ext4::NODES,
        &gaia_fs_erofs::NODES,
        &gaia_sign::NODES,
        &gaia_fs_vfat::NODES,
        &gaia_fs_ubi::NODES,
        &gaia_initramfs::NODES,
        &gaia_strip::NODES,
        &gaia_verity::NODES,
        &gaia_disk::NODES,
        &gaia_k230_image::NODES,
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

/// The recipes directory: the flag, else `recipes/` next to the product.
pub fn recipes_dir(product: &Path, flag: Option<&Path>) -> PathBuf {
    match flag {
        Some(dir) => dir.to_path_buf(),
        None => product.parent().unwrap_or(Path::new(".")).join("recipes"),
    }
}

/// `--setting IMAGE.NAME=VALUE`: each fills a parameter of the image's
/// recipe that the product leaves unset, read as the parameter's type.
fn apply_settings(
    product: &mut gaia_describe::Product,
    file: &Path,
    recipes: &gaia_recipe::Recipes,
    given: &[args::Setting],
) -> Result<(), Stop> {
    for setting in given {
        let flag = setting.given();
        let refuse = |why: String| Stop::usage(format!("{flag}: {why}"));
        let names: Vec<&str> = product.images.iter().map(|i| i.name.as_str()).collect();
        let Some(index) = names.iter().position(|n| *n == setting.image) else {
            return Err(refuse(format!(
                "{} has no image `{}`; its images are {}",
                file.display(),
                setting.image,
                names.join(", ")
            )));
        };
        let image = &mut product.images[index];
        let recipe = recipes
            .get(&image.recipe, file)
            .map_err(|e| refuse(e.to_string()))?;
        let Some(parameter) = recipe.parameters.get(&setting.name) else {
            let known: Vec<&str> = recipe.parameters.keys().map(String::as_str).collect();
            return Err(refuse(format!(
                "recipe {} declares no parameter `{}`; its parameters are {}",
                recipe.name,
                setting.name,
                known.join(", ")
            )));
        };
        if image.settings.contains_key(&setting.name) {
            return Err(refuse(format!(
                "{} already sets it (images[{index}].settings.{}); --setting gives only what the product leaves unset",
                file.display(),
                setting.name
            )));
        }
        let text = setting.value.as_str();
        let value = parameter.kind.from_text(text).ok_or_else(|| {
            refuse(format!(
                "recipe {}'s `{}` wants {}, not `{text}`",
                recipe.name, setting.name, parameter.kind
            ))
        })?;
        image.settings.insert(setting.name.clone(), value);
    }
    Ok(())
}

/// `gaia validate` of a product: every image's recipe is one the recipes
/// directory holds, and the image's settings are parameters the recipe
/// declares, each of its type. A parameter left unset is not refused here:
/// it may have a default, or come from `--setting`. With no `--recipes`
/// and no `recipes/` next to the product, nothing is checked.
pub fn check_recipes(product: &Path, flag: Option<&Path>) -> Result<(), Stop> {
    let dir = recipes_dir(product, flag);
    if flag.is_none() && !dir.is_dir() {
        return Ok(());
    }
    let recipes = gaia_recipe::Recipes::load_dir(&dir).map_err(Stop::usage)?;
    let loaded = gaia_describe::Product::read(product).map_err(Stop::usage)?;
    let known: Vec<String> = recipes.by_name.keys().cloned().collect();
    for (index, image) in loaded.images.iter().enumerate() {
        if let Some(recipe) = recipes.by_name.get(&image.recipe) {
            recipe
                .check_settings(&image.settings, product)
                .map_err(|e| {
                    let name = e.key.strip_prefix("parameters.").unwrap_or(&e.key);
                    Stop::usage(format!(
                        "{}: images[{index}].settings.{name}: recipe {}: {}",
                        product.display(),
                        recipe.name,
                        e.message
                    ))
                })?;
            continue;
        }
        let mut message = format!("no recipe named `{}`", image.recipe);
        match closest(&image.recipe, &known) {
            Some(near) => message.push_str(&format!("; did you mean `{near}`?")),
            None if known.is_empty() => message.push_str("; there are no recipes"),
            None => message.push_str(&format!("; the recipes are {}", known.join(", "))),
        }
        return Err(Stop::usage(format!(
            "{}: images[{index}].recipe: {message}",
            product.display()
        )));
    }
    Ok(())
}

/// The known name nearest `name`, as `gaia_describe` picks it: within a
/// third of its length, at least two edits.
fn closest<'a>(name: &str, known: &'a [String]) -> Option<&'a str> {
    known
        .iter()
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, candidate)| *distance <= (candidate.len() / 3).max(2))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate.as_str())
}

/// Edit distance, by characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(here + 1)
                .min(previous + usize::from(ca != *cb));
            previous = here;
        }
    }
    row[b.len()]
}

/// Every builder name a component may give.
fn builders() -> Builders {
    let mut builders = Builders::default();
    for name in ["go", "java", "make", "node", "python"] {
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

/// The download pool: `GAIA_BUILDROOT_DL_POOL` (empty: none), else
/// `gaia/buildroot-download-pool` in `XDG_CACHE_HOME` or `~/.cache`, the
/// default store's neighbour, shared by every store of the user.
fn download_pool(
    set: Option<std::ffi::OsString>,
    xdg_cache: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(set) = set {
        return (!set.is_empty()).then(|| PathBuf::from(set));
    }
    let cache = xdg_cache
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    Some(cache.join("gaia").join("buildroot-download-pool"))
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The `--input` files, by name, as absolute paths. Each must be a regular
/// file now; a component that takes one is checked when the graph is laid
/// out.
fn inputs(given: &BTreeMap<String, PathBuf>) -> Result<BTreeMap<String, PathBuf>, Stop> {
    given
        .iter()
        .map(|(name, path)| {
            if !path.is_file() {
                return Err(Stop::usage(format!(
                    "--input {name}: {} is not a regular file",
                    path.display()
                )));
            }
            Ok((name.clone(), absolute(path)))
        })
        .collect()
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

/// The host side of a Buildroot run: make and taskset from the tools or
/// PATH, the CPU list from `GAIA_CPUS` (none: no pin), the jobs from
/// `--jobs`, else `GAIA_JOBS` (neither: every CPU), and the download cache from
/// `GAIA_BUILDROOT_DL` (none: `buildroot-downloads` beside the store's
/// objects). The compiler cache is `buildroot-ccache` beside them, shared by
/// every build of the store; `GAIA_BUILDROOT_MIRROR` is the primary download
/// site (none: no mirror). None without make.
/// The jobs an OS build's make runs with: `--jobs` when the build was given
/// one (the bound on what the build runs at once is the bound on make too),
/// else `GAIA_JOBS`, else every CPU.
pub fn make_jobs(flag: Option<usize>, env: Option<&str>, cpus: u32) -> u32 {
    flag.and_then(|jobs| u32::try_from(jobs).ok())
        .or_else(|| env.and_then(|text| text.parse().ok()))
        .filter(|&jobs| jobs > 0)
        .unwrap_or(cpus)
}

fn buildroot_host(
    store: &Path,
    tools: &BTreeMap<String, String>,
    jobs: Option<usize>,
) -> Option<gaia_types::BuildrootHost> {
    let found = |name: &str| {
        tools
            .get(name)
            .cloned()
            .or_else(|| on_path(name).map(|p| p.display().to_string()))
    };
    let make = found("make")?;
    let jobs = make_jobs(
        jobs,
        std::env::var("GAIA_JOBS").ok().as_deref(),
        std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
    );
    let download_dir = std::env::var("GAIA_BUILDROOT_DL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| store.join("buildroot-downloads"));
    let ccache_dir = store.join("buildroot-ccache");
    let download_pool = download_pool(
        std::env::var_os("GAIA_BUILDROOT_DL_POOL"),
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
    );
    Some(gaia_types::BuildrootHost {
        make,
        taskset: found("taskset").unwrap_or_default(),
        cpus: std::env::var("GAIA_CPUS").unwrap_or_default(),
        jobs,
        download_dir: absolute(&download_dir).display().to_string(),
        download_pool: download_pool
            .map(|pool| absolute(&pool).display().to_string())
            .unwrap_or_default(),
        ccache_dir: absolute(&ccache_dir).display().to_string(),
        mirror: std::env::var("GAIA_BUILDROOT_MIRROR")
            .unwrap_or_default()
            .trim()
            .to_string(),
        config_only: false,
        dev_reuse: String::new(),
        bwrap: found("bwrap").unwrap_or_default(),
    })
}

/// `program` in a PATH directory, then in the system directories a user's
/// PATH often leaves out (`mke2fs` and `debugfs` live there).
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/sbin"].map(PathBuf::from))
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_download_pool_is_the_setting_else_beside_the_default_store() {
        use super::download_pool;
        let os = |v: &str| Some(std::ffi::OsString::from(v));
        assert_eq!(
            download_pool(os("/pool"), os("/xdg"), os("/home/u")),
            Some("/pool".into())
        );
        assert_eq!(
            download_pool(os(""), os("/xdg"), os("/home/u")),
            None,
            "empty: none"
        );
        assert_eq!(
            download_pool(None, os("/xdg"), os("/home/u")),
            Some("/xdg/gaia/buildroot-download-pool".into())
        );
        assert_eq!(
            download_pool(None, None, os("/home/u")),
            Some("/home/u/.cache/gaia/buildroot-download-pool".into())
        );
        assert_eq!(download_pool(None, None, None), None);
    }

    /// `--jobs` is the bound on an OS build's make too; the variable is for
    /// a build that was given no flag.
    #[test]
    fn the_jobs_flag_bounds_make() {
        use super::make_jobs;
        assert_eq!(make_jobs(Some(13), None, 32), 13);
        assert_eq!(make_jobs(Some(13), Some("26"), 32), 13, "the flag wins");
        assert_eq!(make_jobs(None, Some("8"), 32), 8);
        assert_eq!(make_jobs(None, None, 32), 32);
        assert_eq!(make_jobs(None, Some("0"), 32), 32, "0 is no count");
        assert_eq!(make_jobs(None, Some("many"), 32), 32);
    }

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
