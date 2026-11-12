//! Laying the descriptions out as nodes and edges.

use crate::{Builders, Error, FUSION, HostInput, STEP, Setup};
use daedalus::layers::graph::{
    EdgeDoc, EdgePolicyDoc, GraphBody, Lit, NodeDoc, NodeKindDoc, PortRefDoc,
};
use daedalus::prelude::Registry;
use daedalus_registry::TypeRef;
use gaia_describe::{Component, ImagePlatform, ImageSpec, Loaded, Source};
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
    };
    // Each built component once, by the product's name for it.
    let mut built: BTreeMap<String, u32> = BTreeMap::new();
    let mut host_ports: BTreeMap<String, String> = BTreeMap::new();
    let mut inputs: BTreeMap<String, HostInput> = BTreeMap::new();
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
                let node = match built.get(name) {
                    Some(&node) => node,
                    None => {
                        let node = component_build(&mut g, component, lock, builders, board)?;
                        built.insert(name.clone(), node);
                        node
                    }
                };
                into.push((name.clone(), node));
            }
        }
        let image_in = ImageIn {
            index: i,
            image,
            builds: &builds,
            data: &data,
            members: &members,
            lock,
            board,
            product_file: &product_file,
        };
        image_recipe(&mut g, &image_in, recipes, &mut host_ports, &mut inputs)?;
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

/// The component's sources its build uses, and its build node.
fn component_build(
    g: &mut Graph<'_>,
    component: &Component,
    lock: &Lock,
    builders: &Builders,
    board: &gaia_describe::Board,
) -> Result<u32, Error> {
    let file = &component.file;
    let build = component.build.as_ref().expect("a build");
    let id = builders.0.get(&build.builder).ok_or_else(|| {
        let known: Vec<&str> = builders.0.keys().map(String::as_str).collect();
        Error::at(
            file,
            "build.builder",
            format!(
                "no builder {}; the builders are {}",
                build.builder,
                known.join(", ")
            ),
        )
    })?;
    let node = g.add(
        id,
        format!("{}.build", component.name),
        file,
        "build.builder",
    )?;
    for (key, value) in &build.settings {
        if !g.has_input(node, key) {
            let ports: Vec<&str> = g.inputs(node).iter().map(|p| p.name).collect();
            return Err(Error::at(
                file,
                format!("build.settings.{key}"),
                format!(
                    "{id} has no input `{key}`; its inputs are {}",
                    ports.join(", ")
                ),
            ));
        }
        let lit = literal(value)
            .and_then(|lit| g.check(node, key, &lit).map(|()| lit))
            .map_err(|e| Error::at(file, format!("build.settings.{key}"), e))?;
        g.set(node, key, lit);
    }
    // The source it builds, then any input named like another source.
    let mut wanted = vec![(build.source.clone(), "source".to_string())];
    for name in component.sources.keys() {
        if *name != build.source && g.has_input(node, name) && !build.settings.contains_key(name) {
            wanted.push((name.clone(), name.clone()));
        }
    }
    for (source_name, port) in wanted {
        let source = component
            .sources
            .get(&source_name)
            .ok_or_else(|| Error::at(file, "build.source", format!("no source {source_name}")))?;
        let from = source_node(g, component, &source_name, source, lock, board)?;
        let out = g.first_output(from).unwrap_or("tree");
        g.edge((from, out), (node, &port));
    }
    Ok(node)
}

/// A node that gives one source, pinned by the lock.
fn source_node(
    g: &mut Graph<'_>,
    component: &Component,
    name: &str,
    source: &Source,
    lock: &Lock,
    board: &gaia_describe::Board,
) -> Result<u32, Error> {
    let file = &component.file;
    let key = format!("sources.{name}");
    let lock_name = format!("{}.{name}", component.name);
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
        Source::Git { .. } => {
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
            g.set(node, "reference", s(reference));
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
        if input == "build" {
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
        host_ports.insert(port.clone(), ty);
        g.edge((node, &at.port), (0, &port));
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
    source_node(g, component, name, source, at.lock, at.board)
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
