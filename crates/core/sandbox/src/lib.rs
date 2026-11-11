//! How a builder step runs its tool inside a pinned root filesystem.
//!
//! A [`Sandbox`] turns a tool into a [`Tool`] that runs `bwrap`
//! (bubblewrap, pinned by path like any tool) around it:
//!
//! - **Rootless:** a new user namespace, the tool as uid 0 inside it and
//!   the caller outside; every other namespace new too (mount, PID, IPC,
//!   UTS, cgroup, and network unless the step declares it).
//! - **The pinned root filesystem** (a tree from an image, C3) is the root,
//!   read-only. Each of its top-level entries is bound in on a fresh root,
//!   so mount points for the inputs and the output exist in any image.
//! - **Inputs** are bound read-only where the step says.
//! - **The output directory is the only writable path that outlives the
//!   tool.** `/tmp` is a private tmpfs, gone with the sandbox; `/proc` and
//!   `/dev` are the sandbox's own.
//! - **Environment:** only what the step sets.
//! - **Host limits** ([`Limits`]): the CPU set (applied with the `taskset`
//!   program given) and the job count (as `GAIA_JOBS` and `MAKEFLAGS`).
//!   They are part of the step's key ([`Sandbox::key`]), as the network
//!   switch and the mount points are.
//!
//! The root filesystem's and the inputs' digests are the step's inputs; the
//! step adds them to its key itself, as it does for any input.

use gaia_step::{KeyBuilder, Tool};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The host limits a step runs under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    /// A CPU list (`6-31`, `0,2,4`) and the `taskset` program that applies
    /// it.
    pub cpus: Option<(String, PathBuf)>,
    /// How many jobs a build tool runs at once.
    pub jobs: Option<u32>,
}

/// A tool's sandbox.
#[derive(Debug, Clone)]
pub struct Sandbox {
    bwrap: PathBuf,
    rootfs: PathBuf,
    inputs: Vec<(PathBuf, String)>,
    output: Option<(PathBuf, String)>,
    network: bool,
    env: Vec<(String, String)>,
    cwd: Option<String>,
    limits: Limits,
}

impl Sandbox {
    /// A sandbox with `rootfs` as its root, run by the `bwrap` program at
    /// `bwrap`. No inputs, no output, no network.
    pub fn new(bwrap: impl Into<PathBuf>, rootfs: impl Into<PathBuf>) -> Self {
        Self {
            bwrap: bwrap.into(),
            rootfs: rootfs.into(),
            inputs: Vec::new(),
            output: None,
            network: false,
            env: Vec::new(),
            cwd: None,
            limits: Limits::default(),
        }
    }

    /// Binds the host path `from` read-only at `at` (absolute) inside.
    pub fn input(mut self, from: impl Into<PathBuf>, at: impl Into<String>) -> Self {
        self.inputs.push((from.into(), at.into()));
        self
    }

    /// Binds the host directory `from` writable at `at` inside: the step's
    /// output directory.
    pub fn output(mut self, from: impl Into<PathBuf>, at: impl Into<String>) -> Self {
        self.output = Some((from.into(), at.into()));
        self
    }

    /// Lets the tool reach the network. Off unless the step declares it.
    pub fn network(mut self, on: bool) -> Self {
        self.network = on;
        self
    }

    /// Sets an environment variable inside.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// The directory the tool starts in, inside. The output directory when
    /// not set, else `/`.
    pub fn cwd(mut self, dir: impl Into<String>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Adds what the sandbox changes about a run to a step's key: the
    /// network switch, the mount points, the environment and the limits.
    pub fn key(&self, key: KeyBuilder) -> KeyBuilder {
        let mut key = key.config("sandbox.network", if self.network { "on" } else { "off" });
        for (index, (_, at)) in self.inputs.iter().enumerate() {
            key = key.config(&format!("sandbox.input.{index}"), at);
        }
        if let Some((_, at)) = &self.output {
            key = key.config("sandbox.output", at);
        }
        for (name, value) in &self.env {
            key = key.config(&format!("sandbox.env.{name}"), value);
        }
        if let Some(cwd) = &self.cwd {
            key = key.config("sandbox.cwd", cwd);
        }
        if let Some((cpus, _)) = &self.limits.cpus {
            key = key.config("sandbox.cpus", cpus);
        }
        if let Some(jobs) = self.limits.jobs {
            key = key.config("sandbox.jobs", jobs.to_string());
        }
        key
    }

    /// The tool that runs `program` (a path inside) in the sandbox. Add the
    /// program's arguments to it with [`Tool::arg`].
    pub fn tool(&self, program: &str) -> std::io::Result<Tool> {
        let mut args: Vec<OsString> = Vec::new();
        let mut push = |parts: &[&dyn AsRef<std::ffi::OsStr>]| {
            args.extend(parts.iter().map(|part| part.as_ref().to_os_string()));
        };
        push(&[&"--unshare-all"]);
        if self.network {
            push(&[&"--share-net"]);
        }
        push(&[
            &"--uid",
            &"0",
            &"--gid",
            &"0",
            &"--die-with-parent",
            &"--new-session",
            &"--clearenv",
        ]);
        // A fresh root, the image's top-level entries bound into it.
        push(&[&"--tmpfs", &"/"]);
        let mut entries: Vec<_> = std::fs::read_dir(&self.rootfs)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name();
            let inside = Path::new("/").join(&name);
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                let target = std::fs::read_link(entry.path())?;
                push(&[&"--symlink", &target, &inside]);
            } else if matches!(name.to_str(), Some("proc" | "dev" | "tmp")) {
                // The sandbox's own, below.
            } else {
                push(&[&"--ro-bind", &entry.path(), &inside]);
            }
        }
        push(&[&"--proc", &"/proc", &"--dev", &"/dev", &"--tmpfs", &"/tmp"]);
        for (from, at) in &self.inputs {
            push(&[&"--ro-bind", from, at]);
        }
        if let Some((from, at)) = &self.output {
            push(&[&"--bind", from, at]);
        }
        push(&[&"--remount-ro", &"/"]);
        let mut env = self.env.clone();
        if let Some(jobs) = self.limits.jobs {
            env.push(("GAIA_JOBS".into(), jobs.to_string()));
            env.push(("MAKEFLAGS".into(), format!("-j{jobs}")));
        }
        for (name, value) in &env {
            push(&[&"--setenv", name, value]);
        }
        let cwd = self
            .cwd
            .clone()
            .or_else(|| self.output.as_ref().map(|(_, at)| at.clone()))
            .unwrap_or_else(|| "/".into());
        push(&[&"--chdir", &cwd, &"--", &program]);
        let tool = match &self.limits.cpus {
            Some((cpus, taskset)) => Tool::new(taskset)
                .arg("--cpu-list")
                .arg(cpus)
                .arg(&self.bwrap),
            None => Tool::new(&self.bwrap),
        };
        Ok(tool.args(args))
    }
}
