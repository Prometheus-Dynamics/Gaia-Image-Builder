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
        }
    }

    pub fn with_extra_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerRunError {
    EmptyImage,
}

impl std::fmt::Display for DockerRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerRunError::EmptyImage => {
                formatter.write_str("docker execution requires a non-empty image")
            }
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
    wrapped.arg("run").arg("--rm");
    if spec.map_workspace_user {
        wrapped.args(docker_workspace_user_args(&spec.workspace_root));
    }
    for mount in normalized_docker_mounts(&spec.mounts) {
        wrapped
            .arg("-v")
            .arg(format!("{}:{}", mount.display(), mount.display()));
    }
    wrapped.arg("-w").arg(&spec.current_dir);
    for (key, value) in &spec.extra_env {
        wrapped.arg("-e").arg(format!(
            "{}={}",
            key.to_string_lossy(),
            value.to_string_lossy()
        ));
    }
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            wrapped.arg("-e").arg(format!(
                "{}={}",
                key.to_string_lossy(),
                value.to_string_lossy()
            ));
        }
    }
    wrapped.arg(&spec.image);
    wrapped.arg(command.get_program());
    wrapped.args(command.get_args());
    Ok(wrapped)
}

pub fn discover_docker_mounts(
    command: &Command,
    workspace_root: &Path,
    current_dir: &Path,
) -> Vec<PathBuf> {
    let mut mounts = BTreeSet::new();
    mounts.insert(workspace_root.to_path_buf());
    mounts.insert(current_dir.to_path_buf());
    for arg in command.get_args() {
        let arg = arg.to_string_lossy();
        if let Some(path) = absolute_docker_mount_candidate(&arg) {
            mounts.insert(path);
        }
    }
    mounts.into_iter().collect()
}

pub fn absolute_docker_mount_candidate(arg: &str) -> Option<PathBuf> {
    if let Some(path) = arg.strip_prefix("file://")
        && Path::new(path).is_absolute()
    {
        return Some(normalize_docker_mount_path(Path::new(path)));
    }
    if let Some((_, value)) = arg.split_once('=')
        && Path::new(value).is_absolute()
    {
        return Some(normalize_docker_mount_path(Path::new(value)));
    }
    Path::new(arg)
        .is_absolute()
        .then(|| normalize_docker_mount_path(Path::new(arg)))
}

pub fn normalize_docker_mount_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    }
}

fn normalized_docker_mounts(mounts: &[PathBuf]) -> Vec<PathBuf> {
    mounts
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn docker_workspace_user_args(workspace_root: &Path) -> Vec<String> {
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

/// Directory names skipped when hashing a Docker build context: VCS
/// metadata and build output that never belong in an execution image.
const DOCKER_CONTEXT_IGNORES: &[&str] = &[".git", ".gaia", "target", "node_modules"];

/// SHA-256 over a Dockerfile and every file in its build context (relative
/// path, type, executable bit, contents and symlink targets), in sorted
/// order. Stable across machines, so it can name a locally built image.
pub fn docker_build_context_hash(dockerfile: &Path, context: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"dockerfile\0");
    hasher.update(std::fs::read(dockerfile)?);
    if !context.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "docker build context '{}' is not a directory",
                context.display()
            ),
        ));
    }
    let mut stack = vec![context.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if DOCKER_CONTEXT_IGNORES.contains(&name) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                stack.push(path);
            } else {
                files.push((path, metadata));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    for (path, metadata) in files {
        let relative = path.strip_prefix(context).unwrap_or(&path);
        hasher.update(b"\0entry\0");
        hasher.update(relative.to_string_lossy().as_bytes());
        if metadata.file_type().is_symlink() {
            hasher.update(b"\0symlink\0");
            hasher.update(std::fs::read_link(&path)?.to_string_lossy().as_bytes());
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = metadata.permissions().mode() & 0o111 != 0;
            hasher.update(if executable { b"\0x\0" } else { b"\0f\0" });
        }
        hasher.update(std::fs::read(&path)?);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
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

    #[test]
    fn docker_build_context_hash_tracks_dockerfile_and_context_changes() {
        let dir = unique_dir("docker-context-hash");
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
            docker_local_image_tag("Helios Cross/Rust", &"ab".repeat(32)),
            "gaia-local/helios-cross-rust:abababababababab"
        );
        assert_eq!(docker_local_image_tag("!!", "123"), "gaia-local/image:123");
    }
    use std::ffi::OsStr;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn docker_run_command_wraps_program_with_mounts_env_and_workdir() {
        let workspace = unique_dir("docker-run-workspace");
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

        let _ = fs::remove_dir_all(workspace);
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

    fn unique_dir(name: &str) -> PathBuf {
        let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join("gaia-tests").join(format!(
            "gaia-process-docker-{name}-{}-{counter}",
            std::process::id()
        ))
    }
}
