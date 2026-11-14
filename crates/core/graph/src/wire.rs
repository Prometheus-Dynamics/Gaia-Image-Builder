//! Laying the descriptions out as nodes and edges.

use crate::os::{self, InputFrom};
use crate::{Builders, Error, FUSION, HostInput, STEP, Setup};
use daedalus::layers::graph::{
    EdgeDoc, EdgePolicyDoc, GraphBody, Lit, NodeDoc, NodeKindDoc, PortRefDoc,
};
use daedalus::prelude::Registry;
use daedalus_registry::TypeRef;
use gaia_describe::{
    Component, FileRef, ImagePlatform, ImageSpec, InstallFrom, Loaded, OsSpec, Source,
};
use gaia_lock::{Entry, Lock};
use gaia_recipe::Recipes;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Where a node came from: its step name and its description file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub label: String,
    pub file: PathBuf,
}

struct Graph<'r> {
    registry: &'r Registry,
    nodes: Vec<NodeDoc>,
    edges: Vec<EdgeDoc>,
    origins: Vec<Origin>,
    /// Inputs an edge or a constant fills, by node.
    filled: Vec<BTreeSet<String>>,
    /// The named inputs a laid-out install entry takes.
    used_inputs: BTreeSet<String>,
    /// Each image's generators: what a recipe takes from them.
    generators: BTreeMap<String, Vec<Generator>>,
    /// Each image's host tools step.
    host_tools: BTreeMap<String, u32>,
}

/// A component's `[generate]` step in one image.
#[derive(Clone)]
struct Generator {
    component: String,
    file: PathBuf,
    /// The step, whose `tree` is what the tool wrote.
    node: u32,
    /// The tool's source, and the port its tree is on.
    source: (u32, &'static str),
    provides: BTreeMap<String, String>,
}

impl Graph<'_> {
    fn add(&mut self, id: &str, label: String, file: &Path, key: &str) -> Result<u32, Error> {
        if self.registry.entry(id).is_none() {
            return Err(Error::at(file, key, format!("no node {id} is registered")));
        }
        let mut node = NodeDoc::new(id);
        node.label = Some(label.clone());
        node.metadata.insert(FUSION.into(), Lit::Bool(false));
        node.metadata.insert(STEP.into(), Lit::Str(label.clone()));
        self.nodes.push(node);
        self.origins.push(Origin {
            label,
            file: file.to_path_buf(),
        });
        self.filled.push(BTreeSet::new());
        Ok((self.nodes.len() - 1) as u32)
    }

    fn inputs(&self, node: u32) -> Vec<&'static daedalus_registry::PortDecl> {
        let id = &self.nodes[node as usize].id;
        self.registry
            .entry(id)
            .map(|e| e.decl.inputs.iter().collect())
            .unwrap_or_default()
    }

    fn has_input(&self, node: u32, port: &str) -> bool {
        self.inputs(node).iter().any(|p| p.name == port)
    }

    fn first_output(&self, node: u32) -> Option<&'static str> {
        let id = &self.nodes[node as usize].id;
        self.registry
            .entry(id)?
            .decl
            .outputs
            .first()
            .map(|p| p.name)
    }

    /// Whether `value` can be the constant on `port`: Daedalus sets only
    /// scalars from a constant, and the scalar must be the port's type.
    fn check(&self, node: u32, port: &str, value: &Lit) -> Result<(), String> {
        let id = &self.nodes[node as usize].id;
        let Some(entry) = self.registry.entry(id) else {
            return Ok(());
        };
        let Some(decl) = entry.decl.inputs.iter().find(|p| p.name == port) else {
            let ports: Vec<&str> = entry.decl.inputs.iter().map(|p| p.name).collect();
            return Err(format!(
                "{id} has no input `{port}`; its inputs are {}",
                ports.join(", ")
            ));
        };
        let key = match entry.port_type(decl) {
            TypeRef::Key(key) => key.as_str().to_string(),
            other => format!("{other:?}"),
        };
        const INTS: [&str; 10] = [
            "i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "usize",
        ];
        let int = INTS.contains(&key.as_str());
        let fits = matches!(
            (key.as_str(), value),
            ("string", Lit::Str(_))
                | ("bool", Lit::Bool(_))
                | ("f64" | "f32", Lit::Float(_) | Lit::Int(_))
        ) || (int && matches!(value, Lit::Int(_)));
        if fits {
            return Ok(());
        }
        let scalar = int || matches!(key.as_str(), "string" | "bool" | "f64" | "f32");
        let what = match key.as_str() {
            "string" => "a string".to_string(),
            "bool" => "a bool".to_string(),
            "f64" | "f32" => "a number".to_string(),
            k if scalar => format!("an int ({k})"),
            k => k.to_string(),
        };
        if !scalar {
            return Err(format!(
                "`{port}` takes {what}, which a description cannot set: Daedalus sets only scalars from a constant"
            ));
        }
        Err(format!("`{port}` takes {what}, not {}", shown(value)))
    }

    fn set(&mut self, node: u32, port: &str, value: Lit) {
        self.nodes[node as usize]
            .consts
            .insert(port.to_string(), value);
        self.filled[node as usize].insert(port.to_string());
    }

    fn edge(&mut self, from: (u32, &str), to: (u32, &str)) {
        self.edges.push(EdgeDoc {
            from: PortRefDoc {
                node: from.0,
                port: from.1.to_string(),
            },
            to: PortRefDoc {
                node: to.0,
                port: to.1.to_string(),
            },
            policy: EdgePolicyDoc::default(),
        });
        self.filled[to.0 as usize].insert(to.1.to_string());
    }

    /// `store` and the tools on every input nothing fills; anything else
    /// left unfilled is an error naming the node's description.
    fn fill(&mut self, setup: &Setup) -> Result<(), Error> {
        for node in 1..self.nodes.len() as u32 {
            for port in self.inputs(node) {
                if self.filled[node as usize].contains(port.name) {
                    continue;
                }
                if port.name == "store" {
                    self.set(node, "store", Lit::Str(setup.store.display().to_string()));
                } else if let Some(tool) = setup.tools.get(port.name) {
                    self.set(node, port.name, Lit::Str(tool.clone()));
                } else if !port.optional && port.default.is_none() {
                    let origin = &self.origins[node as usize];
                    return Err(Error::at(
                        &origin.file,
                        "",
                        format!(
                            "step {} ({}) needs its input `{}`, and nothing gives it: a setting, a source or a tool of that name",
                            origin.label, self.nodes[node as usize].id, port.name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The wired graph body, the origin of each node, and the host inputs `gaia build` feeds.
pub(crate) type Wired = (GraphBody, Vec<crate::Origin>, BTreeMap<String, HostInput>);

pub(crate) fn wire(
    loaded: &Loaded,
    lock: &Lock,
    recipes: &Recipes,
    registry: &Registry,
    setup: &Setup,
    builders: &Builders,
) -> Result<Wired, Error> {
    let product_file = loaded.product.file.clone();
    let mut host = NodeDoc::new("host");
    host.kind = NodeKindDoc::HostBridge;
    let mut g = Graph {
        registry,
        nodes: vec![host],
        edges: Vec::new(),
        origins: vec![Origin {
            label: "host".into(),
            file: PathBuf::new(),
        }],
        filled: vec![BTreeSet::new()],
        used_inputs: BTreeSet::new(),
        generators: BTreeMap::new(),
        host_tools: BTreeMap::new(),
    };
    // Each built component once, by the product's name for it.
    let mut built: BTreeMap<String, u32> = BTreeMap::new();
    let mut host_ports: BTreeMap<String, String> = BTreeMap::new();
    let mut inputs: BTreeMap<String, HostInput> = BTreeMap::new();
    // OS input trees, once each: a locked source by its lock name, a
    // directory by its path.
    let mut os_inputs: BTreeMap<String, u32> = BTreeMap::new();
    // The boards each component is built for: a component on several gets a
    // step per board, named for it.
    let mut boards_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for image in &loaded.product.images {
        let board = &loaded.boards[image.board.path()];
        for name in image.components.iter().chain(&image.data) {
            boards_of
                .entry(name.as_str())
                .or_default()
                .insert(board.name.as_str());
        }
    }
    for (i, image) in loaded.product.images.iter().enumerate() {
        let board = &loaded.boards[image.board.path()];
        let mut builds = Vec::new();
        let mut data = Vec::new();
        let mut members = Vec::new();
        for (list, names, into) in [
            ("components", &image.components, &mut builds),
            ("data", &image.data, &mut data),
        ] {
            for name in names {
                let component = loaded.components.get(name).ok_or_else(|| {
                    Error::at(
                        &product_file,
                        format!("images[{i}].{list}"),
                        format!("no component {name}"),
                    )
                })?;
                if list == "components" {
                    members.push(component);
                }
                if component.build.is_none() {
                    continue;
                }
                // Once per board: a build takes its target from the board.
                let once = format!("{name}\n{}", board.name);
                let node = match built.get(&once) {
                    Some(&node) => node,
                    None => {
                        let several = boards_of.get(name.as_str()).is_some_and(|b| b.len() > 1);
                        let build = component.build.as_ref().expect("a build");
                        let node = component_build(
                            &mut g, component, build, "build", lock, builders, board, several,
                        )?;
                        built.insert(once, node);
                        node
                    }
                };
                into.push((name.clone(), node));
            }
        }
        let os = match &image.os {
            None => None,
            Some(spec) => Some(image_os(
                &mut g,
                &OsIn {
                    index: i,
                    image,
                    spec,
                    board,
                    members: &members,
                    builds: &builds,
                    loaded,
                    lock,
                    setup,
                    wants_system: recipes
                        .get(&image.recipe, &product_file)
                        .is_ok_and(|recipe| recipe.inputs.contains_key("system")),
                    builders,
                },
                &mut os_inputs,
                &mut host_ports,
                &mut inputs,
            )?),
        };
        let image_in = ImageIn {
            index: i,
            image,
            builds: &builds,
            data: &data,
            members: &members,
            lock,
            board,
            product_file: &product_file,
            os,
        };
        image_recipe(&mut g, &image_in, recipes, &mut host_ports, &mut inputs)?;
    }
    for name in setup.inputs.keys() {
        if !g.used_inputs.contains(name) {
            let taken: Vec<&str> = g.used_inputs.iter().map(String::as_str).collect();
            let taken = if taken.is_empty() {
                "none".to_string()
            } else {
                taken.join(", ")
            };
            return Err(Error::at(
                &product_file,
                "",
                format!(
                    "--input {name}={} names no input the product's components take; the inputs it takes are {taken}",
                    setup.inputs[name].display()
                ),
            ));
        }
    }
    g.fill(setup)?;
    g.nodes[0].port_types = host_ports;
    Ok((
        GraphBody {
            nodes: g.nodes,
            edges: g.edges,
            groups: Vec::new(),
        },
        g.origins,
        inputs,
    ))
}

/// The sources a build of the component uses, and the build's node. `build`
/// is the component's own (`word` is `build`) or its tool's (`tool`): the
/// word is in the steps' names and in the keys errors point at.
#[allow(clippy::too_many_arguments)]
fn component_build(
    g: &mut Graph<'_>,
    component: &Component,
    build: &gaia_describe::Build,
    word: &str,
    lock: &Lock,
    builders: &Builders,
    board: &gaia_describe::Board,
    several_boards: bool,
) -> Result<u32, Error> {
    let file = &component.file;
    // A step of a component built for several boards says which it is for.
    let step = |what: &str| {
        let what = match (word, what) {
            ("build", what) => what.to_string(),
            (word, "build") => word.to_string(),
            (word, what) => format!("{word}.{what}"),
        };
        if several_boards {
            format!("{}.{what}@{}", component.name, board.name)
        } else {
            format!("{}.{what}", component.name)
        }
    };
    let id = builders.0.get(&build.builder).ok_or_else(|| {
        let known: Vec<&str> = builders.0.keys().map(String::as_str).collect();
        Error::at(
            file,
            format!("{word}.builder"),
            format!(
                "no builder {}; the builders are {}",
                build.builder,
                known.join(", ")
            ),
        )
    })?;
    let node = g.add(id, step("build"), file, &format!("{word}.builder"))?;
    for (key, value) in &build.settings {
        if !g.has_input(node, key) {
            let ports: Vec<&str> = g.inputs(node).iter().map(|p| p.name).collect();
            return Err(Error::at(
                file,
                format!("{word}.settings.{key}"),
                format!(
                    "{id} has no input `{key}`; its inputs are {}",
                    ports.join(", ")
                ),
            ));
        }
        let lit = literal(value)
            .and_then(|lit| g.check(node, key, &lit).map(|()| lit))
            .map_err(|e| Error::at(file, format!("{word}.settings.{key}"), e))?;
        g.set(node, key, lit);
    }
    // The source it builds, then any input named like another source.
    let mut wanted = vec![(build.source.clone(), "source".to_string())];
    for name in component.sources.keys() {
        if *name != build.source && g.has_input(node, name) && !build.settings.contains_key(name) {
            wanted.push((name.clone(), name.clone()));
        }
    }
    let mut built_from = None;
    for (source_name, port) in wanted {
        let source = component.sources.get(&source_name).ok_or_else(|| {
            Error::at(
                file,
                format!("{word}.source"),
                format!("no source {source_name}"),
            )
        })?;
        let from = component_source(g, component, &source_name, source, lock, board)?;
        let out = g.first_output(from).unwrap_or("tree");
        g.edge((from, out), (node, &port));
        if port == "source" {
            built_from = Some((from, out));
        }
    }
    // The target, from the board, for a builder that takes one and a
    // component that does not set its own.
    if g.has_input(node, "triple") && !g.filled[node as usize].contains("triple") {
        let triple = target_triple(&board.target).ok_or_else(|| {
            Error::at(
                &board.file,
                "target.arch",
                format!(
                    "no target triple is known for {}; set `triple` in {}'s build settings",
                    board.target.arch.as_str(),
                    component.name
                ),
            )
        })?;
        g.set(node, "triple", Lit::Str(triple.into()));
    }
    if g.has_input(node, "cpu")
        && !g.filled[node as usize].contains("cpu")
        && let Some(cpu) = &board.target.cpu
    {
        // Of a big.LITTLE pair (`cortex-a76.cortex-a55`), the little core:
        // code tuned for it runs on both.
        let cpu = cpu.rsplit('.').next().unwrap_or(cpu);
        g.set(node, "cpu", Lit::Str(cpu.to_string()));
    }
    // The dependencies, fetched by the builder's own vendor step from what
    // the source's lock file names, when the component gives no `vendor`.
    let vendor = format!("{id}.vendor");
    if g.has_input(node, "vendor")
        && !g.filled[node as usize].contains("vendor")
        && g.registry.entry(&vendor).is_some()
        && let Some((from, out)) = built_from
    {
        let fetch = g.add(&vendor, step("vendor"), file, &format!("{word}.builder"))?;
        g.edge((from, out), (fetch, "source"));
        let fetched = g.first_output(fetch).unwrap_or("vendor");
        g.edge((fetch, fetched), (node, "vendor"));
    }
    Ok(node)
}

/// The Rust target triple for a board's Linux target: static musl, so a
/// binary needs nothing of the image's C library. None for a target with no
/// such triple; the component then names its own.
fn target_triple(target: &gaia_types::Target) -> Option<&'static str> {
    use gaia_types::{Arch, Os};
    if target.os != Os::Linux {
        return None;
    }
    Some(match target.arch {
        Arch::AArch64 => "aarch64-unknown-linux-musl",
        Arch::X86_64 => "x86_64-unknown-linux-musl",
        Arch::Arm => "armv7-unknown-linux-musleabihf",
        Arch::RiscV64 => "riscv64gc-unknown-linux-musl",
        _ => return None,
    })
}

/// A component's source as a node.
fn component_source(
    g: &mut Graph<'_>,
    component: &Component,
    name: &str,
    source: &Source,
    lock: &Lock,
    board: &gaia_describe::Board,
) -> Result<u32, Error> {
    let lock_name = format!("{}.{name}", component.name);
    source_node(g, &component.file, name, &lock_name, source, lock, board)
}

/// A node that gives one source, written in `file` under `sources.<name>`,
/// pinned by the lock under `lock_name`, and labelled by it.
fn source_node(
    g: &mut Graph<'_>,
    file: &Path,
    name: &str,
    lock_name: &str,
    source: &Source,
    lock: &Lock,
    board: &gaia_describe::Board,
) -> Result<u32, Error> {
    let key = format!("sources.{name}");
    let lock_name = lock_name.to_string();
    let label = lock_name.clone();
    let pinned = || {
        lock.get(&lock_name).ok_or_else(|| {
            Error::at(
                file,
                key.clone(),
                format!("{lock_name} is not in the lock; lock it first"),
            )
        })
    };
    let s = |v: &str| Lit::Str(v.to_string());
    let node = match source {
        Source::Git { subdir, .. } => {
            let Entry::Git { url, commit, .. } = pinned()? else {
                return Err(Error::at(
                    file,
                    key,
                    format!("the lock has {lock_name} as another kind"),
                ));
            };
            let node = g.add("gaia.source.git", label, file, &key)?;
            g.set(node, "url", s(url));
            g.set(node, "commit", s(commit));
            if let Some(subdir) = subdir {
                g.set(node, "subdir", s(subdir.as_str()));
            }
            node
        }
        Source::Download { .. } => {
            let Entry::Download { url, digest } = pinned()? else {
                return Err(Error::at(
                    file,
                    key,
                    format!("the lock has {lock_name} as another kind"),
                ));
            };
            let node = g.add("gaia.source.download", label, file, &key)?;
            g.set(node, "url", s(url));
            g.set(node, "digest", s(digest));
            g.set(node, "kind", s("archive"));
            node
        }
        Source::Image { platform, .. } => {
            let Entry::Image { reference, digest } = pinned()? else {
                return Err(Error::at(
                    file,
                    key,
                    format!("the lock has {lock_name} as another kind"),
                ));
            };
            // The lock's reference may carry the digest the description
            // wrote (`repo:tag@sha256:...`); the node takes the reference
            // and the digest apart.
            let (bare, named) = match reference.split_once('@') {
                Some((bare, named)) => (bare, Some(named)),
                None => (reference.as_str(), None),
            };
            if named.is_some_and(|named| named != digest) {
                return Err(Error::at(
                    file,
                    key,
                    format!(
                        "{lock_name}: the lock's digest {digest} is not the one {reference} names"
                    ),
                ));
            }
            let node = g.add("gaia.source.oci", label, file, &key)?;
            let arch = match platform {
                ImagePlatform::Target => board.target.arch.as_str(),
                ImagePlatform::Host => std::env::consts::ARCH,
            };
            let arch = match arch {
                "aarch64" => "arm64",
                "x86_64" => "amd64",
                "riscv64" => "riscv64",
                other => other,
            };
            g.set(node, "reference", s(bare));
            g.set(node, "digest", s(digest));
            g.set(node, "platform", s(&format!("linux/{arch}")));
            g.set(node, "credentials", s(""));
            node
        }
        Source::Path { path } => {
            let node = g.add("gaia.source.path", label, file, &key)?;
            g.set(node, "path", s(&path.path().display().to_string()));
            node
        }
    };
    Ok(node)
}

/// One image and what its recipe's inputs come from.
struct ImageIn<'a> {
    index: usize,
    image: &'a ImageSpec,
    /// Its built components and their build nodes.
    builds: &'a [(String, u32)],
    /// Its built data components and their build nodes.
    data: &'a [(String, u32)],
    /// Every component it names.
    members: &'a [&'a Component],
    lock: &'a Lock,
    board: &'a gaia_describe::Board,
    product_file: &'a Path,
    /// Its OS's steps and its merged root filesystem, when it has an OS.
    os: Option<OsNodes>,
}

/// The image's recipe, inline, its outputs to the host.
fn image_recipe(
    g: &mut Graph<'_>,
    at: &ImageIn<'_>,
    recipes: &Recipes,
    host_ports: &mut BTreeMap<String, String>,
    inputs: &mut BTreeMap<String, HostInput>,
) -> Result<(), Error> {
    let (i, image, builds, product_file) = (at.index, at.image, at.builds, at.product_file);
    let recipe = recipes
        .get(&image.recipe, product_file)
        .map_err(|e| Error::at(product_file, format!("images[{i}].recipe"), e.message))?;
    let group = recipe.group(&image.settings, product_file).map_err(|e| {
        Error::at(
            &e.file,
            e.key.clone(),
            format!("recipe {}: {}", recipe.name, e.message),
        )
    })?;
    let mut stages = BTreeMap::new();
    for (stage, id) in &group.stages {
        let node = g.add(
            id,
            format!("{}::{stage}", image.name),
            &recipe.file,
            &format!("stages.{stage}"),
        )?;
        stages.insert(stage.clone(), node);
    }
    for c in &group.consts {
        let node = stages[&c.at.stage];
        g.check(node, &c.at.port, &c.value)
            .map_err(|e| Error::at(&recipe.file, format!("{}.{}", c.at.stage, c.at.port), e))?;
        g.set(node, &c.at.port, c.value.clone());
    }
    for (from, to) in &group.connects {
        g.edge(
            (stages[&from.stage], &from.port),
            (stages[&to.stage], &to.port),
        );
    }
    for (input, ports) in &group.inputs {
        if input == "build"
            && let Some(os) = at.os
        {
            for p in ports {
                g.edge((os.merged, "tree"), (stages[&p.stage], &p.port));
            }
        } else if input == "system"
            && let Some(os) = at.os
        {
            for p in ports {
                g.edge((os.system, "tree"), (stages[&p.stage], &p.port));
            }
        } else if matches!(
            input.as_str(),
            "kernel" | "device-trees" | "bootloader" | "os"
        ) {
            let Some((os, port)) = at.os.and_then(|os| os.output(input)) else {
                return Err(Error::at(
                    product_file,
                    format!("images[{i}]"),
                    format!(
                        "recipe {} takes the OS's boot files as `{input}`; this image has no `os`",
                        recipe.name
                    ),
                ));
            };
            for p in ports {
                g.edge((os, port), (stages[&p.stage], &p.port));
            }
        } else if input == "build" || input == "system" {
            // With no OS there is nothing under /boot to leave out: the same tree.
            let [(_, node)] = builds else {
                return Err(Error::at(
                    product_file,
                    format!("images[{i}].components"),
                    format!(
                        "recipe {} takes one built component as `build`; this image has {}",
                        recipe.name,
                        builds.len()
                    ),
                ));
            };
            let out = g.first_output(*node).unwrap_or("tree");
            for p in ports {
                g.edge((*node, out), (stages[&p.stage], &p.port));
            }
        } else if input == "generated"
            || input == "generator-source"
            || input.starts_with("provides.")
        {
            let made = g.generators.get(&image.name).cloned().unwrap_or_default();
            let [generator] = made.as_slice() else {
                return Err(Error::at(
                    product_file,
                    format!("images[{i}].components"),
                    format!(
                        "recipe {} takes `{input}` from the image's one component with a [generate]; this image has {}",
                        recipe.name,
                        made.len()
                    ),
                ));
            };
            if let Some(role) = input.strip_prefix("provides.") {
                let Some(path) = generator.provides.get(role) else {
                    let known: Vec<&str> = generator.provides.keys().map(String::as_str).collect();
                    return Err(Error::at(
                        &generator.file,
                        "generate.provides",
                        format!(
                            "recipe {} takes the generated file of the role `{role}`; {} provides {}",
                            recipe.name,
                            generator.component,
                            if known.is_empty() {
                                "none".to_string()
                            } else {
                                known.join(", ")
                            }
                        ),
                    ));
                };
                stated(g, &stages, ports, &recipe.file, input, path)?;
            } else {
                let (node, port) = if input == "generated" {
                    (generator.node, "tree")
                } else {
                    generator.source
                };
                for p in ports {
                    g.edge((node, port), (stages[&p.stage], &p.port));
                }
            }
        } else if input == "host-tools" {
            let Some(tools) = g.host_tools.get(&image.name).copied() else {
                return Err(Error::at(
                    product_file,
                    format!("images[{i}]"),
                    format!(
                        "recipe {} takes the OS build's host tools; this image has no `os`",
                        recipe.name
                    ),
                ));
            };
            for p in ports {
                g.edge((tools, "seed"), (stages[&p.stage], &p.port));
            }
        } else if input == "cmdline-base" {
            let words = at
                .board
                .kernel
                .as_ref()
                .and_then(|kernel| kernel.cmdline.clone())
                .unwrap_or_default();
            stated(g, &stages, ports, &recipe.file, input, &words)?;
        } else if input == "device-tree-name" {
            let Some(tree) = at
                .board
                .kernel
                .as_ref()
                .and_then(|kernel| kernel.device_tree.as_deref())
            else {
                return Err(Error::at(
                    &at.board.file,
                    "kernel.device_tree",
                    format!(
                        "recipe {} takes the name of the board's device tree; the board {} states none",
                        recipe.name, at.board.name
                    ),
                ));
            };
            let name = format!("{}.dtb", tree.rsplit('/').next().unwrap_or(tree));
            stated(g, &stages, ports, &recipe.file, input, &name)?;
        } else if input == "blobs" {
            let list = image_blobs(g, at, &recipe.name)?;
            for p in ports {
                g.edge((list, "list"), (stages[&p.stage], &p.port));
            }
        } else if input == "data"
            && at.data.is_empty()
            && ports.iter().all(|p| {
                g.inputs(stages[&p.stage])
                    .iter()
                    .any(|decl| decl.name == p.port && decl.optional)
            })
        {
            // No data component, and every port that takes it is optional.
        } else if input == "data" {
            let [(_, node)] = at.data else {
                return Err(Error::at(
                    product_file,
                    format!("images[{i}].data"),
                    format!(
                        "recipe {} takes one built component as `data`; this image has {}",
                        recipe.name,
                        at.data.len()
                    ),
                ));
            };
            let out = g.first_output(*node).unwrap_or("tree");
            for p in ports {
                g.edge((*node, out), (stages[&p.stage], &p.port));
            }
        } else if input == "source" {
            let from = image_source(g, at, &recipe.name)?;
            let out = g.first_output(from).unwrap_or("tree");
            for p in ports {
                g.edge((from, out), (stages[&p.stage], &p.port));
            }
        } else if input == "disk" {
            let layout = image_disk(at, &recipe.name)?;
            let port = format!("{}.disk-layout", image.name);
            let value = HostInput::Disk(layout);
            host_ports.insert(port.clone(), value.type_key().to_string());
            inputs.insert(port.clone(), value);
            for p in ports {
                g.edge((0, &port), (stages[&p.stage], &p.port));
            }
        } else if input == "control" {
            let control = image_control(at, &recipe.name)?;
            let port = format!("{}.control", image.name);
            let value = HostInput::Control(control);
            host_ports.insert(port.clone(), value.type_key().to_string());
            inputs.insert(port.clone(), value);
            for p in ports {
                g.edge((0, &port), (stages[&p.stage], &p.port));
            }
        }
        // Any other input is filled like a stage's own: store or a tool.
    }
    for (output, at) in &group.outputs {
        let node = stages[&at.stage];
        let id = g.nodes[node as usize].id.clone();
        let decl = g.registry.entry(&id).and_then(|e| {
            e.decl
                .outputs
                .iter()
                .find(|p| p.name == at.port)
                .map(|p| e.port_type(p))
        });
        let ty = match decl {
            Some(TypeRef::Key(key)) => key.as_str().to_string(),
            _ => {
                return Err(Error::at(
                    &recipe.file,
                    format!("outputs.{output}"),
                    format!("{id} has no output `{}` with a named type", at.port),
                ));
            }
        };
        let port = format!("{}.{output}", image.name);
        if host_ports.contains_key(&port) {
            return Err(Error::at(
                &recipe.file,
                format!("outputs.{output}"),
                format!("the host port {port} is taken by another input or output"),
            ));
        }
        host_ports.insert(port.clone(), ty);
        g.edge((node, &at.port), (0, &port));
    }
    Ok(())
}

/// Sets the text `value` of the recipe input `input` on each of `ports`.
fn stated(
    g: &mut Graph<'_>,
    stages: &BTreeMap<String, u32>,
    ports: &[daedalus::layers::graph::StagePortDoc],
    recipe_file: &Path,
    input: &str,
    value: &str,
) -> Result<(), Error> {
    for p in ports {
        let node = stages[&p.stage];
        let value = Lit::Str(value.to_string());
        g.check(node, &p.port, &value)
            .map_err(|e| Error::at(recipe_file, format!("inputs.{input}"), e))?;
        g.set(node, &p.port, value);
    }
    Ok(())
}

/// The one source of the image's one component, as a source node.
fn image_source(g: &mut Graph<'_>, at: &ImageIn<'_>, recipe: &str) -> Result<u32, Error> {
    let key = format!("images[{}].components", at.index);
    let [component] = at.members else {
        return Err(Error::at(
            at.product_file,
            key,
            format!(
                "recipe {recipe} takes one component's source as `source`; this image has {} components",
                at.members.len()
            ),
        ));
    };
    let mut sources = component.sources.iter();
    let (Some((name, source)), None) = (sources.next(), sources.next()) else {
        return Err(Error::at(
            &component.file,
            "sources",
            format!(
                "recipe {recipe} takes the component's one source as `source`; it has {}",
                component.sources.len()
            ),
        ));
    };
    component_source(g, component, name, source, at.lock, at.board)
}

/// The board's areas outside the partition table that name their content,
/// each a `gaia.disk.blob` picking that file from the OS's bootloader
/// output, collected by one `gaia.disk.blobs`. Gives the collector.
fn image_blobs(g: &mut Graph<'_>, at: &ImageIn<'_>, recipe: &str) -> Result<u32, Error> {
    let board = at.board;
    let name = &at.image.name;
    let list = g.add(
        "gaia.disk.blobs",
        format!("{name}.blobs"),
        &board.file,
        "partitions",
    )?;
    let mut k = 0;
    for (i, need) in board.partitions.iter().enumerate() {
        let (false, Some(content)) = (need.in_table, &need.content) else {
            continue;
        };
        let Some((os, _)) = at.os.and_then(|os| os.output("bootloader")) else {
            return Err(Error::at(
                at.product_file,
                format!("images[{}]", at.index),
                format!(
                    "recipe {recipe} writes the board's `{}` area from the OS's bootloader output; this image has no `os`",
                    need.name
                ),
            ));
        };
        let offset = need.offset.ok_or_else(|| {
            Error::at(
                &board.file,
                format!("partitions[{i}].offset"),
                "missing: an area outside the table is written at its offset",
            )
        })?;
        let blob = g.add(
            "gaia.disk.blob",
            format!("{name}.blob.{}", need.name),
            &board.file,
            &format!("partitions[{i}]"),
        )?;
        g.set(blob, "path", Lit::Str(content.clone()));
        g.set(blob, "name", Lit::Str(need.name.clone()));
        g.set(blob, "offset", Lit::Int(offset.0.0 as i64));
        g.edge((os, "bootloader"), (blob, "from"));
        g.edge((blob, "blob"), (list, &format!("blobs{k}")));
        k += 1;
    }
    Ok(list)
}

/// The control data from the `[package]` table of the image's one component
/// that has one.
fn image_control(at: &ImageIn<'_>, recipe: &str) -> Result<gaia_types::Control, Error> {
    let packaged: Vec<&&Component> = at.members.iter().filter(|c| c.package.is_some()).collect();
    let [component] = packaged.as_slice() else {
        return Err(Error::at(
            at.product_file,
            format!("images[{}].components", at.index),
            format!(
                "recipe {recipe} takes `control` from one component's [package]; this image has {} with one",
                packaged.len()
            ),
        ));
    };
    let package = component.package.as_ref().expect("filtered");
    let control = gaia_types::Control {
        package: package
            .name
            .clone()
            .unwrap_or_else(|| component.name.clone()),
        version: package.version.clone(),
        architecture: package.architecture.clone(),
        maintainer: package.maintainer.clone(),
        description: package
            .description
            .clone()
            .or_else(|| component.description.clone())
            .unwrap_or_default(),
        depends: package.depends.clone(),
    };
    control
        .check()
        .map_err(|e| Error::at(&component.file, "package", e))?;
    Ok(control)
}

/// The disk layout from the image's board: its `[disk]` table and the
/// partitions it needs in the table. A partition outside the table (a raw
/// bootloader area) needs content, so it is not part of the layout.
fn image_disk(at: &ImageIn<'_>, recipe: &str) -> Result<gaia_types::DiskLayout, Error> {
    let board = at.board;
    let disk = board.disk.as_ref().ok_or_else(|| {
        Error::at(
            &board.file,
            "disk",
            format!("recipe {recipe} lays out a disk; the board has no [disk] table"),
        )
    })?;
    let table = gaia_types::DiskTable {
        scheme: match disk.scheme.expect("checked when the board is read") {
            gaia_describe::Scheme::Gpt => gaia_types::TableScheme::Gpt,
            gaia_describe::Scheme::Mbr => gaia_types::TableScheme::Mbr,
        },
        first: disk.first.map(|s| s.0),
        alignment: disk.alignment.map(|s| s.0),
        size: disk.size.map(|s| s.0),
    };
    let mut partitions = Vec::new();
    for (i, need) in board.partitions.iter().enumerate() {
        if !need.in_table {
            continue;
        }
        let table_type = need.table_type.clone().ok_or_else(|| {
            Error::at(
                &board.file,
                format!("partitions[{i}].type"),
                "a partition in the table needs its type: a GPT type GUID or an MBR type byte",
            )
        })?;
        partitions.push(gaia_types::DiskPartition {
            name: need.name.clone(),
            role: need.role.clone(),
            table_type,
            bootable: false,
            offset: need.offset.map(|s| s.0),
            size: need.size.map(|s| s.0),
            content: None,
        });
    }
    Ok(gaia_types::DiskLayout { table, partitions })
}

/// An image's OS as nodes: the root filesystem's build, the kernel's and
/// the bootloader's when the board has them, and the merged root filesystem.
#[derive(Clone, Copy)]
struct OsNodes {
    rootfs: u32,
    kernel: Option<u32>,
    bootloader: Option<u32>,
    merged: u32,
    /// The root filesystem without the kernel's image and device trees
    /// under /boot: what a slot holds, whose kernel is read from elsewhere.
    /// The same node as `merged` when nothing is under /boot.
    system: u32,
}

impl OsNodes {
    /// The step and its output port a recipe input of this name takes. A
    /// board with no kernel or bootloader build has those files, if any,
    /// from the one build.
    fn output(&self, input: &str) -> Option<(u32, &'static str)> {
        Some(match input {
            "kernel" => (self.kernel.unwrap_or(self.rootfs), "kernel"),
            "device-trees" => (self.kernel.unwrap_or(self.rootfs), "device_trees"),
            "bootloader" => (self.bootloader.unwrap_or(self.rootfs), "bootloader"),
            "os" => (self.rootfs, "images"),
            _ => return None,
        })
    }
}

/// One image's OS and what it is built with.
struct OsIn<'a> {
    index: usize,
    image: &'a ImageSpec,
    spec: &'a OsSpec,
    board: &'a gaia_describe::Board,
    members: &'a [&'a Component],
    builds: &'a [(String, u32)],
    loaded: &'a Loaded,
    lock: &'a Lock,
    setup: &'a Setup,
    /// Whether the image's recipe takes `system`.
    wants_system: bool,
    builders: &'a Builders,
}

/// The image's OS step, its input trees and host inputs, and the image's
/// root filesystem: the OS's tree, then each component's layer, merged.
/// Gives the OS node and the merge node.
fn image_os(
    g: &mut Graph<'_>,
    at: &OsIn<'_>,
    os_inputs: &mut BTreeMap<String, u32>,
    host_ports: &mut BTreeMap<String, String>,
    inputs: &mut BTreeMap<String, HostInput>,
) -> Result<OsNodes, Error> {
    let product = &at.loaded.product;
    let file = &product.file;
    let i = at.index;
    let key = format!("images[{i}].os");
    let plan = os::plan(at.board, i, at.spec, file, at.lock)?;
    let mut parts = os::split(&plan, at.board);
    // A component's kernel contract is checked in the kernel's build, with
    // the board's storage drivers and the kind of image the board takes.
    for component in at.members {
        let Some(wanted) = component
            .requires
            .as_ref()
            .and_then(|requires| requires.kernel_contract.as_ref())
        else {
            continue;
        };
        let at_key = "requires.kernel_contract";
        let source = component.sources.get(&wanted.source).ok_or_else(|| {
            Error::at(
                &component.file,
                format!("{at_key}.source"),
                format!("no source named `{}`", wanted.source),
            )
        })?;
        let Some(kernel) = parts.kernel.as_mut() else {
            return Err(Error::at(
                &component.file,
                at_key,
                format!(
                    "{} states a kernel contract, and the board {} of the image {} has no kernel to check",
                    component.name, at.board.name, at.image.name
                ),
            ));
        };
        let drivers = at
            .board
            .kernel
            .as_ref()
            .and_then(|kernel| kernel.storage_drivers.clone())
            .unwrap_or_default();
        if let Some(bad) = drivers.iter().find(|name| {
            !name.starts_with("CONFIG_")
                || !name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        }) {
            return Err(Error::at(
                &at.board.file,
                "kernel.storage_drivers",
                format!("`{bad}` is not an option as the kernel's .config writes it (CONFIG_NAME)"),
            ));
        }
        // Raw NAND takes an mtd image; everything else a disk.
        let kind = match at.board.flash.as_ref().and_then(|flash| flash.medium) {
            Some(gaia_describe::Medium::SpiNand) => "mtd",
            _ => "disk",
        };
        kernel.require(
            gaia_types::KernelContract {
                input: format!("contract.{}", component.name),
                path: wanted.path.clone(),
                asked_by: component.name.clone(),
                board: at.board.name.clone(),
                kind: kind.into(),
                firmware: at
                    .board
                    .boot
                    .as_ref()
                    .and_then(|boot| boot.firmware.clone())
                    .unwrap_or_default(),
                storage_drivers: drivers,
            },
            InputFrom::Picked {
                file: component.file.clone(),
                name: wanted.source.clone(),
                source: source.clone(),
                lock_name: format!("{}.{}", component.name, wanted.source),
                path: wanted.path.clone(),
            },
        );
    }
    let source = product.sources.get(&at.spec.source).ok_or_else(|| {
        Error::at(
            file,
            format!("{key}.source"),
            format!("no source named `{}`", at.spec.source),
        )
    })?;
    let lock_name = format!("{}.{}", product.name, at.spec.source);
    let from = match os_inputs.get(&format!("source:{lock_name}")) {
        Some(&node) => node,
        None => {
            let node = source_node(
                g,
                file,
                &at.spec.source,
                &lock_name,
                source,
                at.lock,
                at.board,
            )?;
            os_inputs.insert(format!("source:{lock_name}"), node);
            node
        }
    };
    let buildroot = at.setup.buildroot.clone().ok_or_else(|| {
        Error::at(
            file,
            key.clone(),
            "the image has an OS, and the host gives no Buildroot setup (make, jobs, download directory)",
        )
    })?;
    // One build per part: `<image>.os` is the root filesystem's,
    // `<image>.kernel` and `<image>.bootloader` the board's.
    let mut step = |g: &mut Graph<'_>,
                    id: &str,
                    what: &str,
                    port: String,
                    plan: os::OsPlan,
                    host_ports: &mut BTreeMap<String, String>,
                    inputs: &mut BTreeMap<String, HostInput>|
     -> Result<u32, Error> {
        let os = g.add(id, format!("{}.{what}", at.image.name), file, &key)?;
        let out = g.first_output(from).unwrap_or("tree");
        g.edge((from, out), (os, "source"));
        for (k, (name, input)) in plan.inputs.iter().enumerate() {
            let (once, node) = match input {
                InputFrom::Source {
                    file: described,
                    name: source_name,
                    source,
                } => {
                    let once = format!("source:{name}");
                    match os_inputs.get(&once) {
                        Some(&node) => (once, node),
                        None => {
                            let node = source_node(
                                g,
                                described,
                                source_name,
                                name,
                                source,
                                at.lock,
                                at.board,
                            )?;
                            (once, node)
                        }
                    }
                }
                InputFrom::Picked {
                    file: described,
                    name: source_name,
                    source,
                    lock_name,
                    path,
                } => {
                    let once = format!("picked:{lock_name}:{path}");
                    match os_inputs.get(&once) {
                        Some(&node) => (once, node),
                        None => {
                            let from = source_node(
                                g,
                                described,
                                source_name,
                                lock_name,
                                source,
                                at.lock,
                                at.board,
                            )?;
                            let node = g.add(
                                "gaia.tree.pick",
                                format!("{}.os.{name}", at.image.name),
                                described,
                                "requires.kernel_contract",
                            )?;
                            g.set(node, "path", Lit::Str(path.clone()));
                            let out = g.first_output(from).unwrap_or("tree");
                            g.edge((from, out), (node, "tree"));
                            (once, node)
                        }
                    }
                }
                InputFrom::Dir(dir) => {
                    let once = format!("dir:{}", dir.display());
                    match os_inputs.get(&once) {
                        Some(&node) => (once, node),
                        None => {
                            let node = g.add(
                                "gaia.source.path",
                                format!("{}.os.{name}", at.image.name),
                                file,
                                &key,
                            )?;
                            g.set(node, "path", Lit::Str(dir.display().to_string()));
                            (once, node)
                        }
                    }
                }
            };
            os_inputs.insert(once, node);
            let out = g.first_output(node).unwrap_or("tree");
            g.edge((node, out), (os, &format!("inputs{k}")));
        }
        host_input(
            g,
            host_ports,
            inputs,
            &port,
            HostInput::Os(plan.config),
            (os, "os"),
        )?;
        host_input(
            g,
            host_ports,
            inputs,
            "buildroot-host",
            HostInput::Buildroot(buildroot.clone()),
            (os, "host"),
        )?;
        Ok(os)
    };
    let name = &at.image.name;
    // Whether the kernel's image and device trees go in the root filesystem
    // under /boot: the image's fragment says so, in Buildroot's own words.
    let boot_in_rootfs = parts.kernel.as_ref().is_some_and(|kernel| {
        kernel.config.fragments.iter().any(|f| {
            f.text
                .lines()
                .any(|line| line.trim() == "BR2_LINUX_KERNEL_INSTALL_TARGET=y")
        })
    });
    // The host tools, built once from the whole config; each build below
    // starts with them in place.
    let tools = step(
        g,
        "gaia.os.buildroot.host",
        "host-tools",
        format!("{name}.os.host-tools"),
        plan,
        host_ports,
        inputs,
    )?;
    g.host_tools.insert(name.to_string(), tools);
    let os = step(
        g,
        "gaia.os.buildroot",
        "os",
        format!("{name}.os"),
        parts.rootfs,
        host_ports,
        inputs,
    )?;
    g.edge((tools, "seed"), (os, "seed"));
    let kernel = match parts.kernel {
        Some(plan) => {
            let node = step(
                g,
                "gaia.os.buildroot",
                "kernel",
                format!("{name}.os.kernel"),
                plan,
                host_ports,
                inputs,
            )?;
            g.edge((tools, "seed"), (node, "seed"));
            Some(node)
        }
        None => None,
    };
    let bootloader = match parts.bootloader {
        Some(plan) => {
            let node = step(
                g,
                "gaia.os.buildroot",
                "bootloader",
                format!("{name}.os.bootloader"),
                plan,
                host_ports,
                inputs,
            )?;
            g.edge((tools, "seed"), (node, "seed"));
            Some(node)
        }
        None => None,
    };

    // The root filesystem: the OS's tree, then the components' layers.
    let layer = g.add(
        "gaia.tree.layer",
        format!("{}.os.layer", at.image.name),
        file,
        &key,
    )?;
    g.set(layer, "owner", Lit::Str("os".into()));
    g.edge((os, "rootfs"), (layer, "tree"));
    let mut layers = vec![layer];
    // The kernel's image and device trees under /boot, kept apart: `build`
    // has them, `system` does not.
    let mut boot_layers: Vec<u32> = Vec::new();
    // What the kernel's build made that the root filesystem carries: its
    // modules, and its image and device trees when the image wants them.
    if let Some(kernel) = kernel {
        let under = |g: &mut Graph<'_>, what: &str, port: &str, prefix: &str| {
            let node = g.add(
                "gaia.tree.under",
                format!("{name}.kernel.{what}"),
                file,
                &key,
            )?;
            g.set(node, "owner", Lit::Str("os".into()));
            g.set(node, "prefix", Lit::Str(prefix.into()));
            g.edge((kernel, port), (node, "from"));
            Ok::<u32, Error>(node)
        };
        layers.push(under(g, "modules", "modules", "/lib/modules")?);
        if boot_in_rootfs {
            boot_layers.push(under(g, "image", "kernel", "/boot")?);
            boot_layers.push(under(g, "device-trees", "device_trees", "/boot")?);
        }
    }
    for component in at.members {
        let build = at
            .builds
            .iter()
            .find(|(name, _)| *name == component.name)
            .map(|(_, node)| *node);
        layers.extend(component_layers(
            g, at, component, build, host_ports, inputs,
        )?);
    }
    let merge = g.add(
        "gaia.tree.merge",
        format!("{}.rootfs", at.image.name),
        file,
        &key,
    )?;
    for (k, layer) in layers.iter().chain(&boot_layers).enumerate() {
        g.edge((*layer, "layer"), (merge, &format!("layers{k}")));
    }
    // A second tree without /boot's kernel files, only when the recipe asks
    // for `system` and there is something to leave out.
    let system = if at.wants_system && !boot_layers.is_empty() {
        let node = g.add(
            "gaia.tree.merge",
            format!("{}.system", at.image.name),
            file,
            &key,
        )?;
        for (k, layer) in layers.iter().enumerate() {
            g.edge((*layer, "layer"), (node, &format!("layers{k}")));
        }
        node
    } else {
        merge
    };
    Ok(OsNodes {
        rootfs: os,
        kernel,
        bootloader,
        merged: merge,
        system,
    })
}

/// A host input on `port`, fed to `to`. One port the images share (the
/// Buildroot setup) is fed once.
fn host_input(
    g: &mut Graph<'_>,
    host_ports: &mut BTreeMap<String, String>,
    inputs: &mut BTreeMap<String, HostInput>,
    port: &str,
    value: HostInput,
    to: (u32, &str),
) -> Result<(), Error> {
    host_ports.insert(port.to_string(), value.type_key().to_string());
    inputs.insert(port.to_string(), value);
    g.edge((0, port), to);
    Ok(())
}

/// Where a group of install entries is placed from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PlaceFrom {
    /// The component's build output.
    Built,
    /// The component's own directory.
    Beside,
    /// One of the component's sources.
    Source(String),
    /// A named input the command line gives, one file under its name.
    Input(String),
    /// One of the board's descriptions, one file under its name.
    Description(String),
    /// What the component's `[generate]` tool wrote.
    Generated,
}

/// The step that runs a component's `[generate]` tool for the image: the
/// tool, built for the build machine from the component's source, and the
/// board's description it reads.
fn generate_node(g: &mut Graph<'_>, at: &OsIn<'_>, component: &Component) -> Result<u32, Error> {
    let file = &component.file;
    let image = &at.image.name;
    let generate = component.generate.as_ref().expect("a [generate]");
    let described = at.board.descriptions.get(&generate.description).ok_or_else(|| {
        let known: Vec<&str> = at.board.descriptions.keys().map(String::as_str).collect();
        Error::at(
            file,
            "generate.description",
            format!(
                "{} generates from the board's description `{}`, and the board {} ({}) states none of that name under [descriptions]{}",
                component.name,
                generate.description,
                at.board.name,
                at.board.file.display(),
                if known.is_empty() {
                    String::new()
                } else {
                    format!("; it has {}", known.join(", "))
                }
            ),
        )
    })?;
    // The tool runs on the build machine, whatever the board: built for
    // this machine's architecture against the C library of the image it is
    // built in (the image it is then run in), tuned for no CPU.
    let mut settings = toml::Table::new();
    settings.insert("package".into(), generate.tool.package.clone().into());
    settings.insert(
        "triple".into(),
        format!("{}-unknown-linux-gnu", std::env::consts::ARCH).into(),
    );
    settings.insert("cpu".into(), String::new().into());
    let build = gaia_describe::Build {
        builder: "rust".into(),
        source: generate.tool.source.clone(),
        settings,
    };
    let tool = component_build(
        g,
        component,
        &build,
        "tool",
        at.lock,
        at.builders,
        at.board,
        false,
    )?;
    let source = component
        .sources
        .get(&generate.tool.source)
        .ok_or_else(|| {
            Error::at(
                file,
                "generate.tool.source",
                format!("no source named `{}`", generate.tool.source),
            )
        })?;
    let source = component_source(
        g,
        component,
        &generate.tool.source,
        source,
        at.lock,
        at.board,
    )?;
    let description = g.add(
        "gaia.source.input",
        format!(
            "{image}.{}.description.{}",
            component.name, generate.description
        ),
        &at.board.file,
        &format!("descriptions.{}", generate.description),
    )?;
    g.set(
        description,
        "path",
        Lit::Str(described.path().display().to_string()),
    );
    g.set(description, "name", Lit::Str(generate.description.clone()));
    // The image's slot id, for a recipe that has one.
    let slot = at
        .image
        .settings
        .get("slot")
        .and_then(toml::Value::as_str)
        .unwrap_or_default();
    if slot.is_empty() && generate.args.iter().any(|arg| arg.contains("@slot@")) {
        return Err(Error::at(
            file,
            "generate.args",
            format!(
                "an argument names @slot@, and the image {image} sets no `slot` in its settings"
            ),
        ));
    }
    let node = g.add(
        "gaia.generate",
        format!("{image}.{}.generate", component.name),
        file,
        "generate",
    )?;
    let json = |list: &[String]| Lit::Str(serde_json::to_string(list).unwrap_or_default());
    g.set(
        node,
        "program",
        Lit::Str(format!("bin/{}", generate.tool.package)),
    );
    g.set(
        node,
        "description_name",
        Lit::Str(generate.description.clone()),
    );
    g.set(node, "args", json(&generate.args));
    g.set(node, "outputs", json(&generate.outputs));
    g.set(node, "slot", Lit::Str(slot.to_string()));
    // The image the tool was built in is the root it runs in: the
    // component's `root` source, the one its builds take.
    let root = component.sources.get("root").ok_or_else(|| {
        Error::at(
            file,
            "generate.tool",
            format!(
                "{}'s tool runs in the image it is built in, and the component has no `root` source (an image for the build machine)",
                component.name
            ),
        )
    })?;
    let root = component_source(g, component, "root", root, at.lock, at.board)?;
    let out = g.first_output(root).unwrap_or("rootfs");
    g.edge((root, out), (node, "root"));
    let out = g.first_output(tool).unwrap_or("tree");
    g.edge((tool, out), (node, "tool"));
    let out = g.first_output(source).unwrap_or("tree");
    g.edge((source, out), (node, "source"));
    g.generators
        .entry(image.clone())
        .or_default()
        .push(Generator {
            component: component.name.clone(),
            file: file.clone(),
            node,
            source: (source, out),
            provides: generate.provides.clone(),
        });
    let out = g.first_output(description).unwrap_or("tree");
    g.edge((description, out), (node, "description"));
    Ok(node)
}

/// A component's layers in the image: its install entries placed from
/// where each comes from, or, with none, its whole build output.
fn component_layers(
    g: &mut Graph<'_>,
    at: &OsIn<'_>,
    component: &Component,
    build: Option<u32>,
    host_ports: &mut BTreeMap<String, String>,
    inputs: &mut BTreeMap<String, HostInput>,
) -> Result<Vec<u32>, Error> {
    let file = &component.file;
    let image = &at.image.name;
    // The generator runs for the image whether or not a file of its output
    // is installed: a recipe takes from it too.
    let generated = match &component.generate {
        Some(_) => Some(generate_node(g, at, component)?),
        None => None,
    };
    if component.install.is_empty() {
        let Some(build) = build else {
            return Ok(Vec::new());
        };
        let layer = g.add(
            "gaia.tree.layer",
            format!("{image}.{}.layer", component.name),
            file,
            "build",
        )?;
        g.set(layer, "owner", Lit::Str(component.name.clone()));
        let out = g.first_output(build).unwrap_or("tree");
        g.edge((build, out), (layer, "tree"));
        return Ok(vec![layer]);
    }
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let mut groups: BTreeMap<PlaceFrom, Vec<gaia_types::Placement>> = BTreeMap::new();
    // Paths that are symbolic links: nothing is placed, a link is made.
    let mut links: Vec<(String, String)> = Vec::new();
    for (n, install) in component.install.iter().enumerate() {
        let key = format!("install[{n}]");
        let (group, from) = match &install.from {
            InstallFrom::Link(target) => {
                links.push((install.to.clone(), target.clone()));
                continue;
            }
            InstallFrom::Built(path) => (PlaceFrom::Built, path.clone()),
            InstallFrom::File(FileRef::InSource { source, path }) => {
                (PlaceFrom::Source(source.clone()), path.clone())
            }
            InstallFrom::File(FileRef::Input { name }) => {
                if !at.setup.inputs.contains_key(name) {
                    return Err(Error::at(
                        file,
                        format!("{key}.from.input"),
                        format!(
                            "the named input `{name}` is not given; give it with --input {name}=<path>"
                        ),
                    ));
                }
                g.used_inputs.insert(name.clone());
                (PlaceFrom::Input(name.clone()), name.clone())
            }
            InstallFrom::File(FileRef::Generated { path }) => (PlaceFrom::Generated, path.clone()),
            InstallFrom::File(FileRef::Description { name }) => {
                if !at.board.descriptions.contains_key(name) {
                    let known: Vec<&str> =
                        at.board.descriptions.keys().map(String::as_str).collect();
                    return Err(Error::at(
                        file,
                        format!("{key}.from.description"),
                        format!(
                            "{} installs the board's description `{name}`, and the board {} ({}) states none of that name under [descriptions]{}",
                            component.name,
                            at.board.name,
                            at.board.file.display(),
                            if known.is_empty() {
                                String::new()
                            } else {
                                format!("; it has {}", known.join(", "))
                            }
                        ),
                    ));
                }
                (PlaceFrom::Description(name.clone()), name.clone())
            }
            InstallFrom::File(FileRef::Local { file: local }) => {
                let inside = local.path().strip_prefix(&dir).map_err(|_| {
                    Error::at(
                        file,
                        format!("{key}.from"),
                        format!(
                            "{} is outside the component's directory",
                            local.path().display()
                        ),
                    )
                })?;
                (PlaceFrom::Beside, inside.display().to_string())
            }
        };
        let id = |name: &Option<String>, what: &str| -> Result<u32, Error> {
            match name.as_deref() {
                None | Some("root") => Ok(0),
                Some(text) => text.parse().map_err(|_| {
                    Error::at(
                        file,
                        format!("{key}.{what}"),
                        format!(
                            "`{text}` is not root or a number; other names get ids when accounts are wired"
                        ),
                    )
                }),
            }
        };
        groups
            .entry(group)
            .or_default()
            .push(gaia_types::Placement {
                from,
                to: install.to.clone(),
                mode: install.mode.map(|m| m.0),
                uid: id(&install.owner, "owner")?,
                gid: id(&install.group, "group")?,
            });
    }
    let mut layers = Vec::new();
    for (k, (group, placements)) in groups.into_iter().enumerate() {
        let from = match &group {
            PlaceFrom::Built => build.ok_or_else(|| {
                Error::at(
                    file,
                    "install",
                    "an entry is placed from the build's output, and the component has no build",
                )
            })?,
            PlaceFrom::Beside => {
                let node = g.add(
                    "gaia.source.path",
                    format!("{image}.{}.files", component.name),
                    file,
                    "install",
                )?;
                g.set(node, "path", Lit::Str(dir.display().to_string()));
                node
            }
            PlaceFrom::Source(name) => {
                let source = component.sources.get(name).ok_or_else(|| {
                    Error::at(file, "install", format!("no source named `{name}`"))
                })?;
                component_source(g, component, name, source, at.lock, at.board)?
            }
            PlaceFrom::Input(name) => {
                let path = at.setup.inputs.get(name).ok_or_else(|| {
                    Error::at(file, "install", format!("no input named `{name}`"))
                })?;
                let node = g.add(
                    "gaia.source.input",
                    format!("{image}.{}.input.{name}", component.name),
                    file,
                    "install",
                )?;
                g.set(node, "path", Lit::Str(path.display().to_string()));
                g.set(node, "name", Lit::Str(name.clone()));
                node
            }
            PlaceFrom::Generated => generated.ok_or_else(|| {
                Error::at(
                    file,
                    "install",
                    "names a generated file, but there is no [generate]",
                )
            })?,
            PlaceFrom::Description(name) => {
                let described = at.board.descriptions.get(name).ok_or_else(|| {
                    Error::at(file, "install", format!("no description named `{name}`"))
                })?;
                let node = g.add(
                    "gaia.source.input",
                    format!("{image}.{}.description.{name}", component.name),
                    &at.board.file,
                    &format!("descriptions.{name}"),
                )?;
                g.set(
                    node,
                    "path",
                    Lit::Str(described.path().display().to_string()),
                );
                g.set(node, "name", Lit::Str(name.clone()));
                node
            }
        };
        let place = g.add(
            "gaia.tree.place",
            format!("{image}.{}.place{k}", component.name),
            file,
            "install",
        )?;
        g.set(place, "owner", Lit::Str(component.name.clone()));
        let out = g.first_output(from).unwrap_or("tree");
        g.edge((from, out), (place, "from"));
        let port = format!("{image}.{}.placements{k}", component.name);
        host_input(
            g,
            host_ports,
            inputs,
            &port,
            HostInput::Placements(gaia_types::Placements {
                entries: placements,
            }),
            (place, "placements"),
        )?;
        layers.push(place);
    }
    if !links.is_empty() {
        let node = g.add(
            "gaia.tree.links",
            format!("{image}.{}.links", component.name),
            file,
            "install",
        )?;
        g.set(node, "owner", Lit::Str(component.name.clone()));
        g.set(
            node,
            "links",
            Lit::Str(serde_json::to_string(&links).unwrap_or_default()),
        );
        layers.push(node);
    }
    Ok(layers)
}

/// A constant, said with its type.
fn shown(value: &Lit) -> String {
    match value {
        Lit::Str(v) => format!("the string {v:?}"),
        Lit::Int(v) => format!("the int {v}"),
        Lit::Float(v) => format!("the number {v}"),
        Lit::Bool(v) => format!("the bool {v}"),
        Lit::List(_) => "a list".into(),
        _ => "another kind of value".into(),
    }
}

/// A setting as a constant.
fn literal(value: &toml::Value) -> Result<Lit, String> {
    Ok(match value {
        toml::Value::Boolean(v) => Lit::Bool(*v),
        toml::Value::Integer(v) => Lit::Int(*v),
        toml::Value::Float(v) => Lit::Float(*v),
        toml::Value::String(v) => Lit::Str(v.clone()),
        toml::Value::Array(items) => {
            Lit::List(items.iter().map(literal).collect::<Result<_, _>>()?)
        }
        _ => return Err("a setting is a bool, int, float, string or list".into()),
    })
}
