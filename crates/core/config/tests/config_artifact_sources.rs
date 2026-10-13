pub mod support;

use gaia_config::resolve_config;
use gaia_spec::SourceId;
use support::write_temp_config;

fn extra_sources(spec: &gaia_spec::ResolvedBuildSpec, id: &str) -> Vec<String> {
    spec.artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == id)
        .expect("artifact")
        .extra_sources
        .iter()
        .map(|source| source.as_str().to_string())
        .collect()
}

#[test]
fn source_token_in_java_build_fields_adds_the_source_as_an_extra_source() {
    let path = write_temp_config(
        r#"
build_name = "java-extra-source"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "photonvision-source"
kind = "path"
path = "/tmp/photonvision"

[[sources]]
id = "eidos"
kind = "path"
path = "/tmp/eidos"

[[sources]]
id = "unused"
kind = "path"
path = "/tmp/unused"

[[artifacts]]
id = "photonvision-jar"
kind = "java"
source = "photonvision-source"
build_target = "build/libs/photonvision.jar"
build_command = ["./gradlew", "jar"]
build_env = [["EIDOS_PATH", "${source.eidos.path}"]]
output_path = "out/photonvision.jar"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    assert_eq!(extra_sources(&spec, "photonvision-jar"), ["eidos"]);

    let _ = std::fs::remove_file(path);
}

#[test]
fn explicit_sources_are_sorted_deduplicated_and_exclude_the_own_source() {
    let path = write_temp_config(
        r#"
build_name = "rust-extra-sources"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "app"
kind = "path"
path = "."

[[sources]]
id = "eidos"
kind = "path"
path = "/tmp/eidos"

[[sources]]
id = "atlas"
kind = "path"
path = "/tmp/atlas"

[[artifacts]]
id = "node"
kind = "rust"
source = "app"
package = "orion-node"
sources = ["eidos", "atlas", "app", "eidos"]
output_path = "out/node"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    assert_eq!(extra_sources(&spec, "node"), ["atlas", "eidos"]);
    assert!(
        spec.artifacts[0]
            .extra_sources
            .contains(&SourceId::new("eidos"))
    );

    let _ = std::fs::remove_file(path);
}
