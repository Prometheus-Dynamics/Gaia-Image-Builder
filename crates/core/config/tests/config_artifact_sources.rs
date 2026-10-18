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
id = "myapp-source"
kind = "path"
path = "/tmp/myapp"

[[sources]]
id = "corelib"
kind = "path"
path = "/tmp/corelib"

[[sources]]
id = "unused"
kind = "path"
path = "/tmp/unused"

[[artifacts]]
id = "myapp-jar"
kind = "java"
source = "myapp-source"
build_target = "build/libs/myapp.jar"
build_command = ["./gradlew", "jar"]
build_env = [["CORELIB_PATH", "${source.corelib.path}"]]
output_path = "out/myapp.jar"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    assert_eq!(extra_sources(&spec, "myapp-jar"), ["corelib"]);
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
id = "corelib"
kind = "path"
path = "/tmp/corelib"

[[sources]]
id = "bsp"
kind = "path"
path = "/tmp/bsp"

[[artifacts]]
id = "node"
kind = "rust"
source = "app"
package = "worker-node"
sources = ["corelib", "bsp", "app", "corelib"]
output_path = "out/node"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    assert_eq!(extra_sources(&spec, "node"), ["bsp", "corelib"]);
    assert!(
        spec.artifacts[0]
            .extra_sources
            .contains(&SourceId::new("corelib"))
    );
}
