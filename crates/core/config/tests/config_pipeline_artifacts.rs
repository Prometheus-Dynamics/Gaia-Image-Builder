pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{ArtifactExecutionSpec, BuildModeSpec};
use std::time::{SystemTime, UNIX_EPOCH};
use support::write_temp_config;

#[test]
fn resolves_artifact_target_field() {
    let path = write_temp_config(
        r#"
build_name = "artifact-target"
target = "cm5"
profile = "release"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[artifacts]]
id = "cross-bin"
kind = "rust"
package = "gaia"
target = "aarch64-unknown-linux-gnu"
output_path = "out/cross-bin"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));

    assert_eq!(spec.artifacts.len(), 1);
    assert_eq!(
        spec.artifacts[0].target.as_deref(),
        Some("aarch64-unknown-linux-gnu")
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn parses_typed_artifact_build_modes() {
    let config = r#"
build_name = "build-modes"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[artifacts]]
id = "debug-artifact"
kind = "rust"
package = "gaia"
profile = "debug"
output_path = "out/debug-artifact"

[[artifacts]]
id = "release-artifact"
kind = "rust"
package = "gaia"
profile = "release"
output_path = "out/release-artifact"

[[artifacts]]
id = "custom-artifact"
kind = "rust"
package = "gaia"
profile = "dist"
output_path = "out/custom-artifact"
"#;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gaia-build-modes-{nonce}.toml"));
    std::fs::write(&path, config).expect("temp config should be written");

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    let debug = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "debug-artifact")
        .expect("debug artifact");
    let release = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "release-artifact")
        .expect("release artifact");
    let custom = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "custom-artifact")
        .expect("custom artifact");

    assert_eq!(debug.build_mode, Some(BuildModeSpec::Debug));
    assert_eq!(release.build_mode, Some(BuildModeSpec::Release));
    assert_eq!(
        custom.build_mode,
        Some(BuildModeSpec::Custom("dist".into()))
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_mixed_artifact_execution_backends() {
    let path = write_temp_config(
        r#"
build_name = "mixed-execution"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[execution.docker]
enabled = true
image = "docker.io/library/rust:1.90"

[[sources]]
id = "app"
kind = "path"
path = "."

[[artifacts]]
id = "host-tool"
kind = "rust"
package = "host-tool"
source = "app"
output_path = "out/host-tool"

[artifacts.execution]
backend = "host"

[[artifacts]]
id = "docker-tool"
kind = "rust"
package = "docker-tool"
source = "app"
output_path = "out/docker-tool"

[artifacts.execution]
backend = "docker"

[[artifacts]]
id = "docker-tool-custom"
kind = "rust"
package = "docker-tool-custom"
source = "app"
output_path = "out/docker-tool-custom"

[artifacts.execution]
backend = "docker"

[artifacts.execution.docker]
image = "ghcr.io/example/custom:latest"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    assert_eq!(spec.artifacts.len(), 3);
    let host_tool = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "host-tool")
        .expect("host-tool artifact should exist");
    let docker_tool = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "docker-tool")
        .expect("docker-tool artifact should exist");
    let docker_tool_custom = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == "docker-tool-custom")
        .expect("docker-tool-custom artifact should exist");
    assert!(matches!(
        host_tool.execution,
        Some(ArtifactExecutionSpec::Host)
    ));
    assert!(matches!(
        docker_tool.execution,
        Some(ArtifactExecutionSpec::Docker(_))
    ));
    match docker_tool_custom
        .execution
        .as_ref()
        .expect("explicit execution should be set")
    {
        ArtifactExecutionSpec::Docker(docker) => {
            assert_eq!(
                docker.image.as_deref(),
                Some("ghcr.io/example/custom:latest")
            );
        }
        ArtifactExecutionSpec::Host => panic!("expected docker execution"),
    }

    let _ = std::fs::remove_file(path);
}
