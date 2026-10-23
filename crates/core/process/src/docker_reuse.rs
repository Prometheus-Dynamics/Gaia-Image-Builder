//! `reuse_container`: one long-lived container per image, mount set and user, which
//! the wrapped commands run in with `docker exec`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use crate::docker::{
    DockerRunError, DockerRunSpec, docker_workspace_user_args, normalized_docker_mounts,
    read_only_docker_mounts,
};

/// Long-lived containers of this process, keyed by image, mounts and user:
/// the containers `reuse_container` runs execute their commands in.
static REUSED_CONTAINERS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> =
    std::sync::OnceLock::new();

fn reused_containers() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    REUSED_CONTAINERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The wrapped command for `reuse_container`: `docker exec` into a container
/// that stays up for the run. The first command with a given image, mount set
/// and user starts it (`docker run -d ... sleep infinity`); later commands
/// with the same key reuse it. The working directory, the environment and the
/// command itself are passed to each `docker exec`, so they need no new
/// container. Containers are removed by [`remove_reused_containers`].
pub fn docker_reused_exec_command(
    command: &Command,
    spec: &DockerRunSpec,
) -> Result<Command, DockerRunError> {
    if spec.image.trim().is_empty() {
        return Err(DockerRunError::EmptyImage);
    }
    let user = docker_user_args(spec);
    let mounts = docker_mount_flags(spec, command);
    let key = format!("{}\0{mounts:?}\0{user:?}", spec.image);
    let container = {
        let mut containers = reused_containers();
        match containers.get(&key) {
            Some(id) => id.clone(),
            None => {
                let id = start_reused_container(spec, &user, &mounts)?;
                containers.insert(key, id.clone());
                id
            }
        }
    };

    Ok(exec_command_in(&container, command, spec, &user))
}

/// `docker exec` of `command` in `container`, in the working directory, user
/// and environment a one-shot `docker run` of it would have.
fn exec_command_in(
    container: &str,
    command: &Command,
    spec: &DockerRunSpec,
    user: &[String],
) -> Command {
    let mut exec = Command::new("docker");
    exec.arg("exec").arg("-w").arg(&spec.current_dir);
    exec.args(user);
    for (key, value) in &spec.extra_env {
        crate::docker::push_docker_env(&mut exec, key, value);
    }
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            crate::docker::push_docker_env(&mut exec, key, value);
        }
    }
    exec.arg(container);
    exec.arg(command.get_program());
    exec.args(command.get_args());
    exec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_runs_the_command_in_the_container_with_the_run_settings() {
        let spec = DockerRunSpec {
            image: "image:latest".into(),
            workspace_root: PathBuf::from("/work"),
            current_dir: PathBuf::from("/work/pkg"),
            mounts: Vec::new(),
            extra_env: vec![("EXTRA".into(), "1".into())],
            map_workspace_user: false,
            pull_never: false,
        };
        let mut command = Command::new("make");
        command.arg("-j4").env("MAKEFLAGS", "");
        let exec = exec_command_in("cid-1", &command, &spec, &["--user".into(), "0:0".into()]);
        let args: Vec<String> = exec
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(exec.get_program(), "docker");
        assert_eq!(
            args,
            [
                "exec",
                "-w",
                "/work/pkg",
                "--user",
                "0:0",
                "-e",
                "EXTRA=1",
                "-e",
                "MAKEFLAGS=",
                "cid-1",
                "make",
                "-j4",
            ]
        );
    }
}

fn docker_user_args(spec: &DockerRunSpec) -> Vec<String> {
    if spec.map_workspace_user {
        docker_workspace_user_args(&spec.workspace_root)
    } else {
        Vec::new()
    }
}

/// Existing mount directories with whether each is read-only: what a run
/// mounts.
fn docker_mount_flags(spec: &DockerRunSpec, command: &Command) -> Vec<(PathBuf, bool)> {
    let read_only = read_only_docker_mounts();
    normalized_docker_mounts(&spec.mounts, command)
        .into_iter()
        .filter(|mount| mount.is_dir())
        .map(|mount| {
            let read_only = read_only.contains(&mount);
            (mount, read_only)
        })
        .collect()
}

/// `docker run -d` of the reused container: it runs `sleep infinity` so it
/// stays up, with the same init, user and mounts a one-shot run would get.
fn start_reused_container(
    spec: &DockerRunSpec,
    user: &[String],
    mounts: &[(PathBuf, bool)],
) -> Result<String, DockerRunError> {
    let mut run = Command::new("docker");
    run.arg("run")
        .arg("-d")
        .arg("--rm")
        .arg("--init")
        .args(user);
    if spec.pull_never {
        run.arg("--pull=never");
    }
    for (mount, read_only) in mounts {
        let suffix = if *read_only { ":ro" } else { "" };
        run.arg("-v")
            .arg(format!("{0}:{0}{suffix}", mount.display()));
    }
    run.arg(&spec.image).args(["sleep", "infinity"]);
    run.stdin(std::process::Stdio::null());
    let output = run.output().map_err(|error| {
        DockerRunError::ContainerStart(format!("docker could not be started: {error}"))
    })?;
    if !output.status.success() {
        return Err(DockerRunError::ContainerStart(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if id.is_empty() {
        return Err(DockerRunError::ContainerStart(
            "docker returned no container id".into(),
        ));
    }
    Ok(id)
}

/// Removes every container [`docker_reused_exec_command`] started in this
/// process, killing whatever still runs in them. A container that is already
/// gone is not an error.
pub fn remove_reused_containers() {
    let ids: Vec<String> = reused_containers().drain().map(|(_, id)| id).collect();
    for id in ids {
        let _ = Command::new("docker")
            .args(["rm", "-f"])
            .arg(&id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Removes the reused containers when dropped. A run holds one for its whole
/// duration, so the containers go at its end, when it is cancelled, and when it
/// unwinds from a panic.
#[must_use = "dropping the guard removes the run's containers"]
#[derive(Debug, Default)]
pub struct ReusedContainersGuard;

impl Drop for ReusedContainersGuard {
    fn drop(&mut self) {
        remove_reused_containers();
    }
}

#[cfg(test)]
mod docker_tests {
    use super::*;
    use gaia_testkit::TempDir;

    fn docker_has_image(image: &str) -> bool {
        Command::new("docker")
            .args(["image", "inspect", image])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Two commands run in one container, the container is reused for the
    /// second, and removing the reused containers leaves none behind. Skipped
    /// when the host has no docker daemon or no `debian:sid` image.
    #[test]
    fn reused_container_serves_every_command_until_removed() {
        const IMAGE: &str = "debian:sid";
        if !docker_has_image(IMAGE) {
            return;
        }
        let scratch = TempDir::new("docker-reuse");
        let spec = DockerRunSpec::workspace_mount(IMAGE, scratch.path(), &Command::new("true"));

        let mut first = Command::new("sh");
        first.args(["-c", "exit 3"]);
        let status = docker_reused_exec_command(&first, &spec)
            .expect("exec command")
            .status()
            .expect("docker exec");
        assert_eq!(status.code(), Some(3));

        let mut second = Command::new("sh");
        second.args(["-c", "exit 0"]);
        let status = docker_reused_exec_command(&second, &spec)
            .expect("exec command")
            .status()
            .expect("docker exec");
        assert!(status.success());
        assert_eq!(reused_containers().len(), 1, "one container serves both");

        remove_reused_containers();
        assert!(reused_containers().is_empty());
    }
}
