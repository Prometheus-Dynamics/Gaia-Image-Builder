use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerRunSpec {
    pub image: String,
    pub workspace_root: PathBuf,
    pub current_dir: PathBuf,
    pub mounts: Vec<PathBuf>,
    pub extra_env: Vec<(OsString, OsString)>,
    pub map_workspace_user: bool,
    /// `--pull=never`: an offline run uses the image as it is, never pulls it.
    pub pull_never: bool,
}

impl DockerRunSpec {
    pub fn discovered_mounts(
        image: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        command: &Command,
    ) -> Self {
        let workspace_root = workspace_root.into();
        let current_dir = command
            .get_current_dir()
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.clone());
        let mounts = discover_docker_mounts(command, &workspace_root, &current_dir);
        Self {
            image: image.into(),
            workspace_root,
            current_dir,
            mounts,
            extra_env: Vec::new(),
            map_workspace_user: true,
            pull_never: false,
        }
    }

    pub fn workspace_mount(
        image: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        command: &Command,
    ) -> Self {
        let workspace_root = workspace_root.into();
        let current_dir = command
            .get_current_dir()
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.clone());
        Self {
            image: image.into(),
            mounts: vec![workspace_root.clone()],
            workspace_root,
            current_dir,
            extra_env: Vec::new(),
            map_workspace_user: true,
            pull_never: false,
        }
    }

    /// Offline runs never pull: a missing image fails the run instead.
    pub fn with_pull_never(mut self, pull_never: bool) -> Self {
        self.pull_never = pull_never;
        self
    }

    pub fn with_extra_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerRunError {
    EmptyImage,
    /// A reused container could not be started; the message is docker's.
    ContainerStart(String),
}

impl std::fmt::Display for DockerRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerRunError::EmptyImage => {
                formatter.write_str("docker execution requires a non-empty image")
            }
            DockerRunError::ContainerStart(message) => write!(
                formatter,
                "docker execution container failed to start: {message}"
            ),
        }
    }
}

impl std::error::Error for DockerRunError {}

pub fn docker_run_command(
    command: &Command,
    spec: &DockerRunSpec,
) -> Result<Command, DockerRunError> {
    if spec.image.trim().is_empty() {
        return Err(DockerRunError::EmptyImage);
    }

    let mut wrapped = Command::new("docker");
    // `--init` runs a minimal init as PID 1 so signals reach the build and
    // zombies are reaped. `--cidfile` records the container id: killing the
    // docker client alone (timeout, cancel, Ctrl-C) leaves the container
    // running with nobody reading its output, so cleanup removes it by id.
    wrapped
        .arg("run")
        .arg("--rm")
        .arg("--init")
        .arg("--cidfile")
        .arg(new_container_id_file());
    if spec.map_workspace_user {
        wrapped.args(docker_workspace_user_args(&spec.workspace_root));
    }
    if spec.pull_never {
        wrapped.arg("--pull=never");
    }
    // A missing source would be created by docker as root (see
    // `docker_mount_candidates`): only existing directories are mounted.
    let read_only = read_only_docker_mounts();
    for mount in normalized_docker_mounts(&spec.mounts, command)
        .into_iter()
        .filter(|mount| mount.is_dir())
    {
        let suffix = if read_only.contains(&mount) {
            ":ro"
        } else {
            ""
        };
        wrapped
            .arg("-v")
            .arg(format!("{0}:{0}{suffix}", mount.display()));
    }
    wrapped.arg("-w").arg(&spec.current_dir);
    for (key, value) in &spec.extra_env {
        push_docker_env(&mut wrapped, key, value);
    }
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            push_docker_env(&mut wrapped, key, value);
        }
    }
    wrapped.arg(&spec.image);
    wrapped.arg(command.get_program());
    wrapped.args(command.get_args());
    Ok(wrapped)
}

/// A fresh path for `docker run --cidfile`; docker refuses an existing file.
/// Passes `key=value` to the container. A value that holds a secret goes as
/// `-e KEY` with the value in the docker client's own environment, which
/// docker reads it from, so it is never on a command line (visible to every
/// process on the host); other values stay `-e KEY=VALUE`.
pub(crate) fn push_docker_env(
    docker: &mut Command,
    key: &std::ffi::OsStr,
    value: &std::ffi::OsStr,
) {
    if crate::secrets::contains_secret(&value.to_string_lossy()) {
        docker.arg("-e").arg(key).env(key, value);
    } else {
        docker.arg("-e").arg(format!(
            "{}={}",
            key.to_string_lossy(),
            value.to_string_lossy()
        ));
    }
}

fn new_container_id_file() -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "gaia-docker-{}-{nonce}-{count}.cid",
        std::process::id()
    ))
}

/// The container a `docker run --cidfile <file>` command starts, so the
/// process runner can remove it when it stops the command early.
#[derive(Debug, Clone)]
pub(crate) struct DockerContainer {
    docker: OsString,
    cidfile: PathBuf,
}

impl DockerContainer {
    pub(crate) fn for_command(command: &Command) -> Option<Self> {
        let program = Path::new(command.get_program());
        if program.file_name()? != "docker" {
            return None;
        }
        let mut args = command.get_args();
        if args.next()? != "run" {
            return None;
        }
        let mut args = args.skip_while(|arg| *arg != "--cidfile");
        args.next()?;
        Some(Self {
            docker: command.get_program().to_os_string(),
            cidfile: PathBuf::from(args.next()?),
        })
    }

    /// Force-removes the container (killing it first) after the docker
    /// client was stopped early. The id file may lag slightly behind a
    /// container that is just starting, so wait briefly for it.
    pub(crate) fn remove(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let id = loop {
            let id = std::fs::read_to_string(&self.cidfile)
                .map(|contents| contents.trim().to_string())
                .unwrap_or_default();
            if !id.is_empty() || std::time::Instant::now() >= deadline {
                break id;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        if !id.is_empty() {
            let result = Command::new(&self.docker)
                .args(["rm", "--force", &id])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match result {
                Ok(status) if status.success() => {
                    tracing::warn!(container = %id, "removed docker container of a stopped command");
                }
                Ok(status) => {
                    tracing::warn!(container = %id, %status, "failed to remove docker container")
                }
                Err(error) => {
                    tracing::warn!(container = %id, %error, "failed to run docker rm")
                }
            }
        }
        self.discard_id_file();
    }

    /// `docker pause` or `docker unpause` the container, when it has
    /// started.
    pub(crate) fn set_paused(&self, paused: bool) {
        let id = std::fs::read_to_string(&self.cidfile)
            .map(|contents| contents.trim().to_string())
            .unwrap_or_default();
        if id.is_empty() {
            return;
        }
        let _ = Command::new(&self.docker)
            .args([if paused { "pause" } else { "unpause" }, &id])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    pub(crate) fn discard_id_file(&self) {
        let _ = std::fs::remove_file(&self.cidfile);
    }
}

pub(crate) fn discover_docker_mounts(
    command: &Command,
    workspace_root: &Path,
    current_dir: &Path,
) -> Vec<PathBuf> {
    let mut mounts = BTreeSet::new();
    mounts.insert(workspace_root.to_path_buf());
    mounts.insert(current_dir.to_path_buf());
    for arg in command.get_args() {
        let arg = arg.to_string_lossy();
        mounts.extend(docker_mount_candidates(&arg));
    }
    mounts.into_iter().collect()
}

/// The existing directories to mount for the absolute paths in `arg`: a
/// path itself, `file://<path>`, or `NAME=<value>` whose value may list
/// several whitespace-separated paths (`BR2_ROOTFS_POST_BUILD_SCRIPT=/a.sh
/// board/b.sh`). A path that does not exist (nor its parent) is not
/// mounted: docker would create a missing bind-mount source as root, and a
/// value read as one path (`/a.sh board`) left a root-owned directory the
/// build user can no longer remove.
pub(crate) fn docker_mount_candidates(arg: &str) -> Vec<PathBuf> {
    let value = arg
        .strip_prefix("file://")
        .or_else(|| arg.split_once('=').map(|(_, value)| value))
        .unwrap_or(arg);
    value
        .split_whitespace()
        .map(|part| part.strip_prefix("file://").unwrap_or(part))
        .filter(|part| Path::new(part).is_absolute())
        .filter_map(|part| existing_mount_dir(Path::new(part)))
        .collect()
}

/// `path` when it is a directory, else its parent when that is one.
fn existing_mount_dir(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    path.parent()
        .filter(|parent| parent.is_dir())
        .map(Path::to_path_buf)
}

/// Directories every container of this process mounts at their own path:
/// trees Gaia placed outside the workspace (a Buildroot tree in RAM) that
/// paths in the workspace reach through links.
static EXTRA_MOUNTS: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());

/// Mounts `dir` into every container this process runs from now on.
pub fn register_docker_mount(dir: &Path) {
    EXTRA_MOUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(dir.to_path_buf());
}

/// Directories mounted read-only into every container this process runs
/// from now on, for inputs a build must not change (a git mirror it fetches
/// from).
static READ_ONLY_MOUNTS: std::sync::Mutex<BTreeSet<PathBuf>> =
    std::sync::Mutex::new(BTreeSet::new());

/// Mounts `dir` read-only (`:ro`) into every container this process runs
/// from now on. A directory already mounted through the workspace is mounted
/// again inside that mount, read-only.
pub fn register_read_only_docker_mount(dir: &Path) {
    READ_ONLY_MOUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(dir.to_path_buf());
}

pub(crate) fn read_only_docker_mounts() -> BTreeSet<PathBuf> {
    READ_ONLY_MOUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// The spec's mounts, the registered ones, and the targets of links inside
/// them that paths in the command's arguments and environment go through
/// (a link in the workspace to a directory outside it would otherwise
/// dangle in the container). Links outside the mounts (`/bin` -> `/usr/bin`)
/// are never followed.
pub(crate) fn normalized_docker_mounts(mounts: &[PathBuf], command: &Command) -> Vec<PathBuf> {
    let mut all = mounts.iter().cloned().collect::<BTreeSet<_>>();
    all.extend(
        EXTRA_MOUNTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned(),
    );
    all.extend(read_only_docker_mounts());
    let named = command
        .get_args()
        .chain(command.get_envs().filter_map(|(_, value)| value))
        .flat_map(|text| {
            text.to_string_lossy()
                .split([':', ' ', '='])
                .filter(|part| part.starts_with('/'))
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut targets = Vec::new();
    for path in &named {
        for ancestor in path.ancestors() {
            let inside = all
                .iter()
                .any(|mount| ancestor.starts_with(mount) && ancestor != mount);
            if inside
                && std::fs::symlink_metadata(ancestor)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                && let Ok(target) = std::fs::canonicalize(ancestor)
                && target.is_dir()
            {
                targets.push(target);
            }
        }
    }
    all.extend(targets);
    all.into_iter().collect()
}

pub(crate) fn docker_workspace_user_args(workspace_root: &Path) -> Vec<String> {
    #[cfg(unix)]
    {
        if let Ok(metadata) = std::fs::metadata(workspace_root) {
            return vec![
                "--user".to_string(),
                format!("{}:{}", metadata.uid(), metadata.gid()),
            ];
        }
    }
    Vec::new()
}

/// Tag for a Gaia-built execution image: `gaia-local/<name>:<hash prefix>`.
/// `name` is reduced to characters Docker accepts in a repository name.
pub fn docker_local_image_tag(name: &str, content_hash: &str) -> String {
    let mut repository = name
        .chars()
        .map(|character| match character.to_ascii_lowercase() {
            lower @ ('a'..='z' | '0'..='9' | '.' | '_' | '-') => lower,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches(|character| matches!(character, '-' | '.' | '_'))
        .to_string();
    if repository.is_empty() {
        repository = "image".into();
    }
    let short_hash = &content_hash[..content_hash.len().min(16)];
    format!("gaia-local/{repository}:{short_hash}")
}

/// `docker image inspect` printing the image id; fails when the tag is missing.
pub fn docker_image_id_command(program: &std::ffi::OsStr, tag: &str) -> Command {
    let mut command = Command::new(program);
    command
        .arg("image")
        .arg("inspect")
        .arg("--format")
        .arg("{{.Id}}")
        .arg(tag);
    command
}

/// `docker build -f <dockerfile> -t <tag> <context>`.
pub fn docker_image_build_command(
    program: &std::ffi::OsStr,
    dockerfile: &Path,
    context: &Path,
    tag: &str,
) -> Command {
    let mut command = Command::new(program);
    command
        .arg("build")
        .arg("-f")
        .arg(dockerfile)
        .arg("-t")
        .arg(tag)
        .arg(context);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker_context::docker_build_context_hash;
    use gaia_testkit::TempDir;

    #[test]
    fn docker_build_context_hash_tracks_dockerfile_and_context_changes() {
        let scratch = TempDir::new("gaia-process-test");
        let dir = scratch.join("docker-context-hash");
        let context = dir.join("ctx");
        fs::create_dir_all(context.join("scripts")).expect("context");
        fs::create_dir_all(context.join(".git")).expect("git dir");
        let dockerfile = context.join("Dockerfile");
        fs::write(&dockerfile, "FROM rust:1.94\n").expect("dockerfile");
        fs::write(context.join("scripts/setup.sh"), "echo hi\n").expect("script");

        let first = docker_build_context_hash(&dockerfile, &context).expect("hash");
        assert_eq!(first.len(), 64);
        assert_eq!(
            first,
            docker_build_context_hash(&dockerfile, &context).expect("hash")
        );
        fs::write(context.join(".git/HEAD"), "ignored").expect("ignored file");
        assert_eq!(
            first,
            docker_build_context_hash(&dockerfile, &context).expect("hash")
        );
        fs::write(context.join("scripts/setup.sh"), "echo changed\n").expect("script");
        let second = docker_build_context_hash(&dockerfile, &context).expect("hash");
        assert_ne!(first, second);
        fs::write(&dockerfile, "FROM rust:1.95\n").expect("dockerfile");
        assert_ne!(
            second,
            docker_build_context_hash(&dockerfile, &context).expect("hash")
        );
        assert!(docker_build_context_hash(&dir.join("missing"), &context).is_err());
    }

    #[test]
    fn docker_local_image_tag_sanitizes_names() {
        assert_eq!(
            docker_local_image_tag("Acme Cross/Rust", &"ab".repeat(32)),
            "gaia-local/acme-cross-rust:abababababababab"
        );
        assert_eq!(docker_local_image_tag("!!", "123"), "gaia-local/image:123");
    }
    use std::ffi::OsStr;
    use std::fs;

    #[test]
    fn a_secret_env_value_never_appears_on_the_docker_command_line() {
        let scratch = TempDir::new("gaia-docker-secret");
        let workspace = scratch.join("ws");
        fs::create_dir_all(&workspace).expect("workspace");
        let secret_file = scratch.join("token");
        fs::write(&secret_file, "s3cr3t-docker-token\n").expect("secret file");
        let mut permissions = fs::metadata(&secret_file).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o600);
        fs::set_permissions(&secret_file, permissions).expect("chmod");
        crate::secrets::install(
            vec![crate::secrets::SecretDef {
                name: "docker_test_token".into(),
                source: crate::secrets::SecretSource::File(secret_file),
                required: true,
            }],
            "***",
        );
        let value = crate::secrets::resolve("docker_test_token")
            .expect("resolve")
            .expect("value");

        let mut command = Command::new("build-tool");
        command
            .current_dir(&workspace)
            .env("API_TOKEN", value.expose())
            .env("PLAIN", "visible");
        let spec = DockerRunSpec::discovered_mounts("image:latest", &workspace, &command);
        let wrapped = docker_run_command(&command, &spec).expect("docker command");
        let args: Vec<String> = wrapped
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|arg| arg.contains("s3cr3t-docker-token")),
            "{args:?}"
        );
        assert!(
            args.windows(2)
                .any(|window| window[0] == "-e" && window[1] == "API_TOKEN")
        );
        assert!(
            args.windows(2)
                .any(|window| window[0] == "-e" && window[1] == "PLAIN=visible")
        );
        assert!(
            wrapped.get_envs().any(|(key, value)| key == "API_TOKEN"
                && value == Some(OsStr::new("s3cr3t-docker-token")))
        );
    }

    #[test]
    fn docker_run_command_wraps_program_with_mounts_env_and_workdir() {
        let scratch = TempDir::new("gaia-process-test");
        let workspace = scratch.join("docker-run-workspace");
        let output_file = workspace.join("out/image.tar");
        let current_dir = workspace.join("work");
        fs::create_dir_all(output_file.parent().expect("output parent")).expect("output parent");
        fs::create_dir_all(&current_dir).expect("current dir");

        let mut command = Command::new("build-tool");
        command
            .arg("--output")
            .arg(&output_file)
            .arg(format!("cache={}", workspace.join("cache/file").display()))
            .current_dir(&current_dir)
            .env("GAIA_DOCKER_TEST", "yes");

        let spec = DockerRunSpec::discovered_mounts("image:latest", &workspace, &command)
            .with_extra_env("HOME", workspace.join(".gaia/docker-home"));

        let wrapped = docker_run_command(&command, &spec).expect("docker command");
        let args = wrapped
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(wrapped.get_program(), OsStr::new("docker"));
        assert!(args.starts_with(&["run".to_string(), "--rm".to_string()]));
        assert!(
            args.windows(2)
                .any(|window| { window[0] == "-w" && window[1] == current_dir.to_string_lossy() })
        );
        assert!(args.windows(2).any(|window| {
            window[0] == "-e"
                && window[1] == format!("HOME={}", workspace.join(".gaia/docker-home").display())
        }));
        assert!(
            args.windows(2)
                .any(|window| { window[0] == "-e" && window[1] == "GAIA_DOCKER_TEST=yes" })
        );
        assert!(args.iter().any(|arg| arg == "image:latest"));
        assert!(args.iter().any(|arg| arg == "build-tool"));
        assert!(args.iter().any(|arg| arg == "--output"));
        assert!(
            args.iter()
                .any(|arg| arg == output_file.to_str().expect("utf8 path"))
        );
        assert!(args.windows(2).any(|window| {
            window[0] == "-v"
                && window[1] == format!("{}:{}", workspace.display(), workspace.display())
        }));
    }

    #[test]
    fn a_value_listing_several_paths_mounts_each_existing_one_and_no_missing_one() {
        let scratch = TempDir::new("gaia-process-test");
        let root = scratch.join("docker-mount-candidates");
        let scripts = root.join("exclude");
        std::fs::create_dir_all(&scripts).expect("scripts dir");
        std::fs::write(scripts.join("exclude.sh"), "#!/bin/sh\n").expect("script");
        let arg = format!(
            "BR2_ROOTFS_POST_BUILD_SCRIPT={} board/evk/post-build.sh {}",
            scripts.join("exclude.sh").display(),
            root.join("missing/dir/post.sh").display()
        );
        // The script's directory; never `<script> board/...` (read as one
        // path, docker created it as root) and never the missing path.
        assert_eq!(docker_mount_candidates(&arg), vec![scripts.clone()]);
    }

    #[test]
    fn a_read_only_registered_mount_is_mounted_with_ro() {
        let scratch = TempDir::new("gaia-process-test");
        let workspace = scratch.join("docker-ro-mount");
        let mirror = workspace.join(".gaia/cache/git/mirror.git");
        fs::create_dir_all(&mirror).expect("mirror dir");
        register_read_only_docker_mount(&mirror);

        let command = Command::new("build-tool");
        let spec = DockerRunSpec::workspace_mount("image:latest", &workspace, &command);
        let wrapped = docker_run_command(&command, &spec).expect("docker command");
        let args = wrapped
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        let read_only = format!("{0}:{0}:ro", mirror.display());
        assert!(
            args.windows(2)
                .any(|window| window[0] == "-v" && window[1] == read_only),
            "{args:?}"
        );
        // The workspace itself stays read-write.
        assert!(args.windows(2).any(|window| {
            window[0] == "-v" && window[1] == format!("{0}:{0}", workspace.display())
        }));
    }

    #[test]
    fn docker_run_command_rejects_empty_image() {
        let command = Command::new("echo");
        let spec = DockerRunSpec::workspace_mount("", PathBuf::from("/workspace"), &command);

        assert!(matches!(
            docker_run_command(&command, &spec),
            Err(DockerRunError::EmptyImage)
        ));
    }
}
