//! Toolchain identity for artifact fingerprints.
//!
//! Host-built artifacts hash the versions of their build tools on the host.
//! Docker-built artifacts never touch those host tools, so they hash the
//! execution image instead: the image id from `docker image inspect` for a
//! plain `image`, or nothing extra for a Dockerfile-built image, whose
//! content hash is already part of the fingerprint.
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gaia_spec::{ArtifactDefinition, ArtifactExecutionSpec, ArtifactSpec, ResolvedBuildSpec};

use crate::reuse::{COMMAND_SIGNATURE_TIMEOUT_SECONDS, command_signature};

/// Where an artifact's build tools come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArtifactToolchain<'a> {
    Host,
    /// Built by Gaia from `[artifacts.execution.docker] dockerfile`.
    Dockerfile,
    /// A prebuilt image (artifact `image` or `[execution.docker] image`).
    DockerImage(&'a str),
}

/// Mirrors how the artifact providers pick the execution backend: an
/// explicit `execution` wins, otherwise the build-wide docker policy.
pub(crate) fn artifact_toolchain<'a>(
    spec: &'a ResolvedBuildSpec,
    artifact: &'a ArtifactSpec,
) -> ArtifactToolchain<'a> {
    let policy_image = spec
        .policy
        .execution
        .docker
        .as_ref()
        .map(|docker| docker.image.as_str());
    match &artifact.execution {
        Some(ArtifactExecutionSpec::Host) => ArtifactToolchain::Host,
        Some(ArtifactExecutionSpec::Docker(docker)) if docker.dockerfile.is_some() => {
            ArtifactToolchain::Dockerfile
        }
        Some(ArtifactExecutionSpec::Docker(docker)) => ArtifactToolchain::DockerImage(
            docker
                .image
                .as_deref()
                .filter(|image| !image.trim().is_empty())
                .or(policy_image)
                .unwrap_or_default(),
        ),
        None => policy_image
            .map(ArtifactToolchain::DockerImage)
            .unwrap_or(ArtifactToolchain::Host),
    }
}

pub(crate) fn artifact_backend_signature(
    spec: &ResolvedBuildSpec,
    artifact: &ArtifactSpec,
) -> String {
    artifact_backend_signature_with(spec, artifact, docker_image_signature)
}

fn artifact_backend_signature_with(
    spec: &ResolvedBuildSpec,
    artifact: &ArtifactSpec,
    image_signature: impl Fn(&str) -> String,
) -> String {
    match artifact_toolchain(spec, artifact) {
        ArtifactToolchain::Host => host_tool_signature(artifact),
        ArtifactToolchain::Dockerfile => "docker-build".to_string(),
        ArtifactToolchain::DockerImage(image) => image_signature(image),
    }
}

fn host_tool_signature(artifact: &ArtifactSpec) -> String {
    match &artifact.definition {
        ArtifactDefinition::Rust(_) => format!(
            "{}|{}",
            command_signature("cargo", ["--version"]),
            command_signature("rustc", ["--version"])
        ),
        ArtifactDefinition::Go(_) => command_signature("go", ["version"]),
        ArtifactDefinition::Python(_) => command_signature("python3", ["--version"]),
        ArtifactDefinition::Node(_) => format!(
            "{}|{}",
            command_signature("npm", ["--version"]),
            command_signature("node", ["--version"])
        ),
        ArtifactDefinition::Java(_) => format!(
            "{}|{}",
            command_signature("mvn", ["-version"]),
            command_signature("gradle", ["--version"])
        ),
    }
}

/// Image id of a docker execution image, probed once per process.
fn docker_image_signature(image: &str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(signature) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(image).cloned())
    {
        return signature;
    }
    let signature = docker_image_signature_with(OsStr::new("docker"), image);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(image.to_string(), signature.clone());
    }
    signature
}

/// `docker-image:<id>`, or `image-missing:<tag>` when the image (or docker
/// itself) is not available, so planning never fails on it.
pub(crate) fn docker_image_signature_with(docker: &OsStr, image: &str) -> String {
    let image = image.trim();
    let missing = || format!("image-missing:{image}");
    if image.is_empty() || !program_on_path(&docker.to_string_lossy()) {
        return missing();
    }
    let mut command: Command = gaia_process::docker_image_id_command(docker, image);
    let retention = gaia_process::ProcessOutputRetention {
        stdout_bytes: 4096,
        stderr_bytes: 4096,
        stdout_lines: 8,
        stderr_lines: 8,
    };
    match gaia_process::run_command_with_timeout_and_retention(
        &mut command,
        Duration::from_secs(COMMAND_SIGNATURE_TIMEOUT_SECONDS),
        "reuse docker image signature",
        retention,
        None,
        None,
    ) {
        Ok(result) if result.output.status.success() => {
            let id = String::from_utf8_lossy(&result.output.stdout)
                .trim()
                .to_string();
            if id.is_empty() {
                missing()
            } else {
                format!("docker-image:{id}")
            }
        }
        _ => missing(),
    }
}

/// Whether `program` would be found when spawned: a path is checked
/// directly, a bare name is looked up on `PATH`.
pub(crate) fn program_on_path(program: &str) -> bool {
    if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::{
        ArtifactOutputSpec, DockerArtifactExecutionSpec, DockerExecutionSpec, JavaArtifactSpec,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn java_artifact(execution: Option<ArtifactExecutionSpec>) -> ArtifactSpec {
        let mut artifact = ArtifactSpec::new(
            "photonvision",
            ArtifactDefinition::Java(JavaArtifactSpec {
                build_target: "jar".into(),
                build_args: Vec::new(),
                build_command: Vec::new(),
                build_env: Vec::new(),
            }),
            None,
            ArtifactOutputSpec {
                path: "out/photonvision.jar".into(),
            },
        );
        artifact.execution = execution;
        artifact
    }

    fn docker(image: Option<&str>, dockerfile: Option<&str>) -> Option<ArtifactExecutionSpec> {
        Some(ArtifactExecutionSpec::Docker(DockerArtifactExecutionSpec {
            image: image.map(str::to_string),
            dockerfile: dockerfile.map(str::to_string),
            context: None,
        }))
    }

    fn fake_docker() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir()
            .join("gaia-tests")
            .join(format!("fake-docker-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let staged = dir.join("docker.body");
        fs::write(
            &staged,
            "#!/bin/sh\n[ \"$1 $2 $3 $4\" = 'image inspect --format {{.Id}}' ] || exit 2\n\
             if [ \"$5\" = 'maven:3' ]; then echo sha256:abc123; exit 0; fi\n\
             echo 'Error: No such image' >&2\nexit 1\n",
        )
        .expect("script");
        let path = dir.join("docker");
        let status = Command::new("install")
            .arg("-m")
            .arg("0755")
            .arg(&staged)
            .arg(&path)
            .status()
            .expect("install");
        assert!(status.success());
        path
    }

    #[test]
    fn toolchain_follows_explicit_execution_then_build_policy() {
        let mut spec = ResolvedBuildSpec::new("toolchain");
        assert_eq!(
            artifact_toolchain(&spec, &java_artifact(None)),
            ArtifactToolchain::Host
        );
        spec.policy.execution.docker = Some(DockerExecutionSpec {
            image: "gaia/build:1".into(),
        });
        assert_eq!(
            artifact_toolchain(&spec, &java_artifact(None)),
            ArtifactToolchain::DockerImage("gaia/build:1")
        );
        assert_eq!(
            artifact_toolchain(&spec, &java_artifact(Some(ArtifactExecutionSpec::Host))),
            ArtifactToolchain::Host
        );
        assert_eq!(
            artifact_toolchain(&spec, &java_artifact(docker(Some("maven:3"), None))),
            ArtifactToolchain::DockerImage("maven:3")
        );
        assert_eq!(
            artifact_toolchain(&spec, &java_artifact(docker(None, None))),
            ArtifactToolchain::DockerImage("gaia/build:1")
        );
        assert_eq!(
            artifact_toolchain(
                &spec,
                &java_artifact(docker(Some("maven"), Some("docker/Dockerfile")))
            ),
            ArtifactToolchain::Dockerfile
        );
    }

    #[test]
    fn docker_artifacts_never_probe_host_tools() {
        let spec = ResolvedBuildSpec::new("toolchain");
        let probed = std::cell::RefCell::new(Vec::new());
        let probe = |image: &str| {
            probed.borrow_mut().push(image.to_string());
            format!("probed:{image}")
        };

        let image = artifact_backend_signature_with(
            &spec,
            &java_artifact(docker(Some("maven:3"), None)),
            probe,
        );
        let dockerfile = artifact_backend_signature_with(
            &spec,
            &java_artifact(docker(None, Some("docker/Dockerfile"))),
            probe,
        );

        assert_eq!(image, "probed:maven:3");
        assert_eq!(dockerfile, "docker-build");
        assert_eq!(*probed.borrow(), vec!["maven:3".to_string()]);
    }

    #[test]
    fn docker_image_signature_uses_the_image_id_or_a_stable_missing_marker() {
        let docker = fake_docker();
        assert_eq!(
            docker_image_signature_with(docker.as_os_str(), "maven:3"),
            "docker-image:sha256:abc123"
        );
        assert_eq!(
            docker_image_signature_with(docker.as_os_str(), "gaia/missing:1"),
            "image-missing:gaia/missing:1"
        );
        assert_eq!(
            docker_image_signature_with(OsStr::new("/nonexistent/docker"), "maven:3"),
            "image-missing:maven:3"
        );
        let _ = fs::remove_dir_all(docker.parent().expect("dir"));
    }

    #[test]
    fn program_on_path_checks_paths_and_path_entries() {
        assert!(program_on_path("sh"));
        assert!(!program_on_path("gaia-definitely-not-a-tool"));
        assert!(!program_on_path("/nonexistent/gaia-tool"));
    }
}
