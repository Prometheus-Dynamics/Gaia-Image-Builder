//! The component file: one piece of software and what it installs.

use crate::common::{FileRef, Mode, Source, check_source, is_input_name, resolve_sources};
use crate::{Error, read};
use gaia_lock::Want;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    /// The component file. Not written in it.
    #[serde(skip)]
    pub file: PathBuf,
    #[serde(skip_serializing)]
    version: i64,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub sources: BTreeMap<String, Source>,
    pub build: Option<Build>,
    /// Files put into the image.
    #[serde(default)]
    pub install: Vec<Install>,
    /// Services enabled in the image.
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub users: Vec<User>,
    #[serde(default)]
    pub groups: Vec<Group>,
    /// What the component says about itself as a package for another
    /// system's installer (`.ipk`, `.deb`).
    pub package: Option<Package>,
    /// What the component needs of the image it is in.
    pub requires: Option<Requires>,
    /// Files a tool of the component's writes from the board's description.
    pub generate: Option<Generate>,
}

/// Files derived from one of the board's descriptions by a tool built from
/// one of the component's sources, for the build machine, and run once per
/// image. Installs take them with `from = { generated = "<path>" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generate {
    pub tool: GenerateTool,
    /// Which of the board's `[descriptions]` the tool reads.
    pub description: String,
    /// The tool's arguments. `@source@` is the tool's source tree,
    /// `@description@` the board's description file, `@out@` the directory
    /// it writes into, `@slot@` the image's slot id.
    pub args: Vec<String>,
    /// What the tool writes at the top of `@out@`: each must be there when
    /// it ends, and nothing else.
    pub outputs: Vec<String>,
}

/// The tool: a package of one of the component's sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateTool {
    pub source: String,
    /// The package to build; its binary of the same name is the tool.
    pub package: String,
}

/// What a component needs of the image it is in, checked by the build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    /// A file in one of the component's sources that lists the kernel
    /// options its boot path needs built in. The kernel of every image
    /// with the component is checked against it.
    pub kernel_contract: Option<InSource>,
}

/// A file in one of a description's sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InSource {
    pub source: String,
    pub path: String,
}

/// A component as a package: the control data opkg and dpkg read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// The package name; the component's name when not set.
    pub name: Option<String>,
    pub version: String,
    /// The installer's name for the architecture (`aarch64`, `arm64`, `all`).
    pub architecture: String,
    pub maintainer: String,
    /// The synopsis, then further lines; the component's description when
    /// not set.
    pub description: Option<String>,
    /// Other packages it needs, each one name or `name (constraint)`.
    #[serde(default)]
    pub depends: Vec<String>,
}

/// How the component is built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    /// The builder node that makes it (`cargo`, `cmake`, `go`).
    pub builder: String,
    /// The source it builds.
    pub source: String,
    /// The builder's own settings. The builder checks them when the graph
    /// is planned; this crate does not know them.
    #[serde(default)]
    pub settings: toml::Table,
}

/// One file put into the image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Install {
    /// A path in the build's output, or `{ file = ... }` beside this file,
    /// or `{ source, path }` in a source, or `{ input = ... }`: a file the
    /// command line names (`gaia plan --input NAME=PATH`), such as a
    /// developer's key, which is not committed.
    pub from: InstallFrom,
    /// Where it goes in the image: an absolute path.
    pub to: String,
    pub mode: Option<Mode>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

/// Where an installed file comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum InstallFrom {
    /// A path in the build's output (`bin/server`).
    Built(String),
    File(FileRef),
    /// Nothing is copied: the path is a symbolic link to this target
    /// (`{ link = "/run/board" }`), which need not be in the image.
    Link(String),
}

// Read by hand, not untagged: a table that fails as a file reference then
// keeps its own message and key, not "did not match any variant".
impl<'de> Deserialize<'de> for InstallFrom {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match toml::Value::deserialize(deserializer)? {
            toml::Value::String(path) => Ok(Self::Built(path)),
            toml::Value::Table(table) if table.contains_key("link") => {
                match (table.len(), table.get("link").and_then(toml::Value::as_str)) {
                    (1, Some(target)) => Ok(Self::Link(target.to_string())),
                    (1, None) => Err(de::Error::custom("`link` is the link's target, a string")),
                    _ => Err(de::Error::custom(
                        "a link is not combined with another kind of source",
                    )),
                }
            }
            table @ toml::Value::Table(_) => FileRef::deserialize(table)
                .map(Self::File)
                .map_err(de::Error::custom),
            _ => Err(de::Error::custom(
                "`from` is a path in the build's output, or a table: `{ file = ... }`, `{ source, path }` or `{ input = ... }`",
            )),
        }
    }
}

/// A service unit, enabled or only installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    /// The unit's name (`server.service`).
    pub unit: String,
    #[serde(default = "yes")]
    pub enable: bool,
}

fn yes() -> bool {
    true
}

/// A user the component needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub name: String,
    pub uid: Option<u32>,
    /// Its primary group; a group of its own name when not set.
    pub group: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    pub home: Option<String>,
    pub shell: Option<String>,
}

/// A group the component needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub gid: Option<u32>,
}

impl Component {
    /// Reads the component file at `path`.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let mut component: Self = read::read(path)?;
        component.file = path.to_path_buf();
        let dir = path.parent().unwrap_or(Path::new(""));
        resolve_sources(&mut component.sources, dir);
        if let Some(build) = &component.build {
            check_source(
                path,
                "build.source",
                &build.source,
                component.sources.keys(),
            )?;
        }
        if let Some(generate) = &component.generate {
            check_source(
                path,
                "generate.tool.source",
                &generate.tool.source,
                component.sources.keys(),
            )?;
            if !is_input_name(&generate.description) {
                return Err(Error::new(
                    path,
                    "generate.description",
                    format!(
                        "`{}` is not a description's name: lowercase letters, digits and `-`",
                        generate.description
                    ),
                ));
            }
            for (index, arg) in generate.args.iter().enumerate() {
                let unknown = gaia_types::unknown_generate_names(arg);
                if !unknown.is_empty() {
                    return Err(Error::new(
                        path,
                        format!("generate.args[{index}]"),
                        format!(
                            "names {}; the names are {}",
                            unknown.join(", "),
                            gaia_types::GENERATE_NAMES.join(", ")
                        ),
                    ));
                }
            }
            if generate.outputs.is_empty() {
                return Err(Error::new(
                    path,
                    "generate.outputs",
                    "no output is declared; the tool's files are named here",
                ));
            }
            for (index, output) in generate.outputs.iter().enumerate() {
                if output.is_empty() || output.contains('/') || output.starts_with('.') {
                    return Err(Error::new(
                        path,
                        format!("generate.outputs[{index}]"),
                        format!("`{output}` is not a name at the top of the output directory"),
                    ));
                }
            }
        }
        let declared: Option<Vec<String>> = component.generate.as_ref().map(|g| g.outputs.clone());
        for (index, install) in component.install.iter_mut().enumerate() {
            let key = format!("install[{index}]");
            match &mut install.from {
                InstallFrom::Built(_) if component.build.is_none() => {
                    return Err(Error::new(
                        path,
                        format!("{key}.from"),
                        "names a build output, but there is no [build]",
                    ));
                }
                InstallFrom::Built(_) => {}
                InstallFrom::Link(target) => {
                    if target.is_empty() {
                        return Err(Error::new(
                            path,
                            format!("{key}.from.link"),
                            "a link needs a target",
                        ));
                    }
                    if install.mode.is_some() {
                        return Err(Error::new(
                            path,
                            format!("{key}.mode"),
                            "a link has no mode of its own",
                        ));
                    }
                }
                InstallFrom::File(file) => {
                    if let FileRef::Input { name } = file
                        && !is_input_name(name)
                    {
                        return Err(Error::new(
                            path,
                            format!("{key}.from.input"),
                            format!(
                                "`{name}` is not an input name: lowercase letters, digits and `-`, like `bench-key`"
                            ),
                        ));
                    }
                    if let FileRef::Generated { path: generated } = file {
                        let top = generated.split('/').next().unwrap_or_default();
                        match &declared {
                            None => {
                                return Err(Error::new(
                                    path,
                                    format!("{key}.from.generated"),
                                    "names a generated file, but there is no [generate]",
                                ));
                            }
                            Some(outputs) if !outputs.iter().any(|output| output == top) => {
                                return Err(Error::new(
                                    path,
                                    format!("{key}.from.generated"),
                                    format!(
                                        "`{generated}` is in no declared output; [generate] declares {}",
                                        outputs.join(", ")
                                    ),
                                ));
                            }
                            Some(_) => {}
                        }
                    }
                    if let FileRef::Description { name } = file
                        && !is_input_name(name)
                    {
                        return Err(Error::new(
                            path,
                            format!("{key}.from.description"),
                            format!(
                                "`{name}` is not a description's name: lowercase letters, digits and `-`"
                            ),
                        ));
                    }
                    file.resolve(dir);
                    if let FileRef::Local { file } = file {
                        crate::common::check_exists(
                            path,
                            &format!("{key}.from.file"),
                            file.path(),
                        )?;
                    }
                    if let Some(name) = file.source() {
                        check_source(
                            path,
                            &format!("{key}.from.source"),
                            name,
                            component.sources.keys(),
                        )?;
                    }
                }
            }
            if !install.to.starts_with('/') {
                return Err(Error::new(
                    path,
                    format!("{key}.to"),
                    "a path in the image is absolute",
                ));
            }
        }
        if let Some(package) = &component.package
            && package.description.is_none()
            && component.description.is_none()
        {
            return Err(Error::new(
                path,
                "package.description",
                "a package needs a description; set it here or the component's",
            ));
        }
        Ok(component)
    }

    /// What the lock must hold for the component's sources, named
    /// `<component>.<source>`.
    pub fn wants(&self) -> Vec<(String, Want)> {
        self.sources
            .iter()
            .map(|(name, source)| (format!("{}.{name}", self.name), source.want()))
            .collect()
    }
}
