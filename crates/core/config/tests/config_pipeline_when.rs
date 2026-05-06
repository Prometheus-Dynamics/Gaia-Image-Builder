pub mod support;

use gaia_config::resolve_config;
use support::write_temp_config;

#[test]
fn when_filters_artifacts_installs_stage_and_feed_by_build_context() {
    let path = write_temp_config(
        r#"
build_name = "when-filter"
target = "cm5"
profile = "dev"
branch = "dev-branch"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "qemu_x86_64_defconfig"

[image.feed]
install_entries = ["install-kept", "install-dropped"]
stage_files = ["motd-kept", "motd-dropped"]
stage_env_sets = ["env-kept", "env-dropped"]
stage_services = ["svc-kept", "svc-dropped"]

[[artifacts]]
id = "kept-artifact"
kind = "rust"
package = "gaia"
output_path = "out/kept"
when = { all = [ { target = "cm5" }, { profile = "dev" }, { branch = "dev-branch" }, { image_kind = "buildroot" } ] }

[[artifacts]]
id = "dropped-artifact"
kind = "rust"
package = "gaia"
output_path = "out/dropped"
when = { target = "rpi5" }

[[install]]
id = "install-kept"
artifact = "kept-artifact"
dest = "/usr/bin/kept"
when = { any = [ { profile = "release" }, { profile = "dev" } ] }

[[install]]
id = "install-dropped"
artifact = "dropped-artifact"
dest = "/usr/bin/dropped"
when = { not = { branch = "dev-branch" } }

[[stage.files]]
id = "motd-kept"
src = "README.md"
dest = "/etc/motd"
mode = 493
when = { image_kind = "buildroot" }

[[stage.files]]
id = "motd-dropped"
src = "README.md"
dest = "/etc/motd.d/dropped"
when = { image_kind = "starting-point" }

[[stage.env_sets]]
id = "env-kept"
name = "kept"
entries = [["A", "1"]]
when = { target = "cm5" }

[[stage.env_sets]]
id = "env-dropped"
name = "dropped"
entries = [["B", "2"]]
when = { target = "x86" }

[[stage.services]]
id = "svc-kept"
name = "kept.service"
unit_path = "README.md"
when = { branch = "dev-branch" }

[[stage.services]]
id = "svc-dropped"
name = "dropped.service"
unit_path = "README.md"
when = { branch = "main" }
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));

    assert_eq!(
        spec.artifacts
            .iter()
            .map(|artifact| artifact.id.as_str())
            .collect::<Vec<_>>(),
        vec!["kept-artifact"]
    );
    assert_eq!(
        spec.install
            .entries
            .iter()
            .map(|install| install.id.as_str())
            .collect::<Vec<_>>(),
        vec!["install-kept"]
    );
    assert_eq!(
        spec.stage
            .files
            .iter()
            .map(|file| file.id.as_str())
            .collect::<Vec<_>>(),
        vec!["motd-kept"]
    );
    assert_eq!(
        spec.stage
            .env_sets
            .iter()
            .map(|env_set| env_set.id.as_str())
            .collect::<Vec<_>>(),
        vec!["env-kept"]
    );
    assert_eq!(
        spec.stage
            .services
            .iter()
            .map(|service| service.id.as_str())
            .collect::<Vec<_>>(),
        vec!["svc-kept"]
    );
    assert_eq!(
        spec.image
            .feed
            .install_entries
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["install-kept"]
    );
    assert_eq!(
        spec.image
            .feed
            .stage_files
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["motd-kept"]
    );
    assert_eq!(spec.stage.files[0].mode, Some(493));
    assert_eq!(
        spec.image
            .feed
            .stage_env_sets
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["env-kept"]
    );
    assert_eq!(
        spec.image
            .feed
            .stage_services
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["svc-kept"]
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn when_supports_interpolated_conditions() {
    let path = write_temp_config(
        r#"
build_name = "when-interpolated"
target = "cm5"
profile = "release"
branch = "feature-a"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "qemu_x86_64_defconfig"

[[artifacts]]
id = "selected"
kind = "rust"
package = "gaia"
output_path = "out/selected"
when = { all = [ { target = "${build.target}" }, { profile = "${build.profile}" }, { branch = "${build.branch}" } ] }

[[artifacts]]
id = "excluded"
kind = "rust"
package = "gaia"
output_path = "out/excluded"
when = { target = "rpi5" }
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));

    assert_eq!(spec.artifacts.len(), 1);
    assert_eq!(spec.artifacts[0].id.as_str(), "selected");

    let _ = std::fs::remove_file(path);
}
