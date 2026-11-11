//! The component file: one piece of software and what it installs.

use crate::common::{FileRef, Mode, Source, check_source, resolve_sources};
use crate::{Error, read};
use gaia_lock::Want;
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
    /// or `{ source, path }` in a source.
    pub from: InstallFrom,
    /// Where it goes in the image: an absolute path.
    pub to: String,
    pub mode: Option<Mode>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

/// Where an installed file comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InstallFrom {
    /// A path in the build's output (`bin/server`).
    Built(String),
    File(FileRef),
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
                InstallFrom::File(file) => {
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
