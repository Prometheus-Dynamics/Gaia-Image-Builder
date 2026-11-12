//! The recipe file: reading it, checking it, and making its group.

use crate::{Error, ParamType, VERSION};
use daedalus::layers::graph::{GroupDoc, StageConstDoc, StagePortDoc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A recipe file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// The recipe file. Not written in it.
    #[serde(skip)]
    pub file: PathBuf,
    #[serde(skip_serializing)]
    version: i64,
    /// What products name it by, and the group's id.
    pub name: String,
    pub description: Option<String>,
    /// Stage name to node id.
    pub stages: BTreeMap<String, String>,
    #[serde(default)]
    pub connects: Vec<Connect>,
    /// The group's input ports, each feeding stage ports (`stage.port`).
    #[serde(default)]
    pub inputs: BTreeMap<String, Vec<String>>,
    /// The group's output ports, each a stage port.
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
    #[serde(default)]
    pub parameters: BTreeMap<String, Parameter>,
    /// Fixed constants on stage ports, `"stage.port" = value`.
    #[serde(default)]
    pub consts: BTreeMap<String, toml::Value>,
}

/// An edge between two stages, or, with a list of sources, one edge from
/// each into a fan-in port: `to = "merge.layers"` takes them on `layers0`,
/// `layers1` and so on, in list order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    pub from: Sources,
    pub to: String,
}

/// One stage port, or a list of them for a fan-in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Sources {
    One(String),
    Many(Vec<String>),
}

impl Sources {
    /// The `stage.port` texts, in order.
    pub fn ports(&self) -> &[String] {
        match self {
            Self::One(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }
}

/// A value the recipe takes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    #[serde(rename = "type")]
    pub kind: ParamType,
    /// When not set, the parameter must be given.
    pub default: Option<toml::Value>,
    /// The stage ports (`stage.port`) it sets.
    pub sets: Vec<String>,
    pub description: Option<String>,
}

impl Recipe {
    /// Reads and checks the recipe file at `path`.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::new(path, "", format!("cannot read: {e}")))?;
        let line_of = |offset: usize| text[..offset.min(text.len())].matches('\n').count() + 1;
        let table: toml::Table = toml::from_str(&text).map_err(|e| {
            Error::new(path, "", e.message().to_string())
                .at_line(e.span().map(|s| line_of(s.start)))
        })?;
        match table.get("version") {
            Some(toml::Value::Integer(VERSION)) => {}
            None => {
                return Err(Error::new(
                    path,
                    "version",
                    format!("missing; write `version = {VERSION}`"),
                ));
            }
            Some(other) => {
                return Err(Error::new(
                    path,
                    "version",
                    format!("{other} is not a version of this format; it is at {VERSION}"),
                ));
            }
        }
        let name = table
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::to_string);
        let deserializer = toml::de::Deserializer::parse(&text)
            .map_err(|e| Error::new(path, "", e.message().to_string()))?;
        let mut recipe: Recipe = serde_path_to_error::deserialize(deserializer).map_err(|e| {
            let key = e.path().to_string();
            let key = if key == "." { String::new() } else { key };
            let line = e.inner().span().map(|s| line_of(s.start));
            let error = Error::new(path, key, e.inner().message().to_string()).at_line(line);
            match &name {
                Some(name) => error.recipe(name),
                None => error,
            }
        })?;
        recipe.file = path.to_path_buf();
        recipe.check()?;
        Ok(recipe)
    }

    fn error(&self, key: impl Into<String>, message: impl Into<String>) -> Error {
        Error::new(&self.file, key, message).recipe(&self.name)
    }

    /// Every stage port names a stage; every default has its type.
    fn check(&self) -> Result<(), Error> {
        if self.stages.is_empty() {
            return Err(self.error("stages", "a recipe needs at least one stage"));
        }
        let port = |key: String, text: &str| self.port(&key, text).map(drop);
        for (i, c) in self.connects.iter().enumerate() {
            if c.from.ports().is_empty() {
                return Err(self.error(
                    format!("connects[{i}].from"),
                    "an empty list; name the stage ports that feed the fan-in",
                ));
            }
            for from in c.from.ports() {
                port(format!("connects[{i}].from"), from)?;
            }
            port(format!("connects[{i}].to"), &c.to)?;
        }
        for (name, targets) in &self.inputs {
            for target in targets {
                port(format!("inputs.{name}"), target)?;
            }
        }
        for (name, target) in &self.outputs {
            port(format!("outputs.{name}"), target)?;
        }
        for (name, p) in &self.parameters {
            let key = format!("parameters.{name}");
            if p.sets.is_empty() {
                return Err(self.error(key, "sets no stage port"));
            }
            for target in &p.sets {
                port(format!("{key}.sets"), target)?;
            }
            if let Some(default) = &p.default {
                p.kind
                    .lit(default)
                    .map_err(|e| self.error(format!("{key}.default"), e))?;
            }
        }
        for at in self.consts.keys() {
            port(format!("consts.{at}"), at)?;
        }
        Ok(())
    }

    /// `stage.port`, checked against the stages.
    fn port(&self, key: &str, text: &str) -> Result<StagePortDoc, Error> {
        let Some((stage, port)) = text.split_once('.') else {
            return Err(self.error(key, format!("{text:?} is not `stage.port`")));
        };
        if !self.stages.contains_key(stage) {
            let known: Vec<&str> = self.stages.keys().map(String::as_str).collect();
            return Err(self.error(
                key,
                format!("no stage {stage:?}; the stages are {}", known.join(", ")),
            ));
        }
        Ok(StagePortDoc {
            stage: stage.to_string(),
            port: port.to_string(),
        })
    }
}

pub(crate) fn group(recipe: &Recipe, values: &toml::Table, from: &Path) -> Result<GroupDoc, Error> {
    let param_error = |name: &str, message: String| {
        Error::new(from, format!("parameters.{name}"), message).recipe(&recipe.name)
    };
    for name in values.keys() {
        if !recipe.parameters.contains_key(name) {
            let known: Vec<&str> = recipe.parameters.keys().map(String::as_str).collect();
            let hint = closest(name, &known)
                .map(|near| format!("; did you mean {near}?"))
                .unwrap_or_else(|| {
                    if known.is_empty() {
                        "; it takes none".to_string()
                    } else {
                        format!("; it takes {}", known.join(", "))
                    }
                });
            return Err(param_error(
                name,
                format!("not a parameter of this recipe{hint}"),
            ));
        }
    }
    let mut consts = Vec::new();
    for (at, value) in &recipe.consts {
        consts.push(StageConstDoc {
            at: recipe.port(&format!("consts.{at}"), at)?,
            value: literal(value).map_err(|e| recipe.error(format!("consts.{at}"), e))?,
        });
    }
    for (name, p) in &recipe.parameters {
        let lit = match (values.get(name), &p.default) {
            (Some(value), _) => p.kind.lit(value).map_err(|e| param_error(name, e))?,
            (None, Some(default)) => p
                .kind
                .lit(default)
                .map_err(|e| recipe.error(format!("parameters.{name}.default"), e))?,
            (None, None) => {
                return Err(param_error(
                    name,
                    format!("missing; it has no default and wants {}", p.kind),
                ));
            }
        };
        for target in &p.sets {
            consts.push(StageConstDoc {
                at: recipe.port(&format!("parameters.{name}.sets"), target)?,
                value: lit.clone(),
            });
        }
    }
    let mut connects = Vec::new();
    for (i, c) in recipe.connects.iter().enumerate() {
        let to = recipe.port(&format!("connects[{i}].to"), &c.to)?;
        let fan_in = matches!(c.from, Sources::Many(_));
        for (n, from) in c.from.ports().iter().enumerate() {
            let from = recipe.port(&format!("connects[{i}].from"), from)?;
            let to = if fan_in {
                StagePortDoc {
                    stage: to.stage.clone(),
                    port: format!("{}{n}", to.port),
                }
            } else {
                to.clone()
            };
            connects.push((from, to));
        }
    }
    let inputs = recipe
        .inputs
        .iter()
        .map(|(name, targets)| {
            let ports = targets
                .iter()
                .map(|t| recipe.port(&format!("inputs.{name}"), t))
                .collect::<Result<_, _>>()?;
            Ok((name.clone(), ports))
        })
        .collect::<Result<_, Error>>()?;
    let outputs = recipe
        .outputs
        .iter()
        .map(|(name, t)| Ok((name.clone(), recipe.port(&format!("outputs.{name}"), t)?)))
        .collect::<Result<_, Error>>()?;
    Ok(GroupDoc {
        id: recipe.name.clone(),
        stages: recipe.stages.clone(),
        connects,
        inputs,
        outputs,
        consts,
    })
}

/// A fixed constant: any TOML value but a date or a table.
fn literal(value: &toml::Value) -> Result<daedalus::layers::graph::Lit, String> {
    use daedalus::layers::graph::Lit;
    Ok(match value {
        toml::Value::Boolean(v) => Lit::Bool(*v),
        toml::Value::Integer(v) => Lit::Int(*v),
        toml::Value::Float(v) => Lit::Float(*v),
        toml::Value::String(v) => Lit::Str(v.clone()),
        toml::Value::Array(items) => {
            Lit::List(items.iter().map(literal).collect::<Result<_, _>>()?)
        }
        _ => return Err("a constant is a bool, int, float, string or list".into()),
    })
}

/// The known name nearest `name`, when one is close.
fn closest<'a>(name: &str, known: &[&'a str]) -> Option<&'a str> {
    known
        .iter()
        .map(|k| (distance(name, k), *k))
        .filter(|(d, k)| *d <= 2.max(k.len() / 3))
        .min()
        .map(|(_, k)| k)
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// Every recipe in a directory, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipes {
    pub by_name: BTreeMap<String, Recipe>,
}

impl Recipes {
    /// Reads every `*.toml` file in `dir`. Two recipes with one name is an
    /// error naming both files.
    pub fn load_dir(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| Error::new(dir, "", format!("cannot read: {e}")))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        let mut by_name: BTreeMap<String, Recipe> = BTreeMap::new();
        for file in files {
            let recipe = Recipe::read(&file)?;
            if let Some(first) = by_name.get(&recipe.name) {
                return Err(
                    recipe.error("name", format!("also the name of {}", first.file.display()))
                );
            }
            by_name.insert(recipe.name.clone(), recipe);
        }
        Ok(Self { by_name })
    }

    /// The recipe `name`, or an error naming the file that asked for it.
    pub fn get(&self, name: &str, asked_in: &Path) -> Result<&Recipe, Error> {
        self.by_name.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.by_name.keys().map(String::as_str).collect();
            let hint = closest(name, &known)
                .map(|near| format!("; did you mean {near}?"))
                .unwrap_or_default();
            Error::new(asked_in, "recipe", format!("no recipe {name:?}{hint}"))
        })
    }
}
