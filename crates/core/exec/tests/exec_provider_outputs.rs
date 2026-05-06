pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::plan_build;
use std::fs;
use std::path::Path;
use std::process::Command;
use support::{provider_catalogs, test_spec};

#[test]
fn provider_execution_materializes_real_outputs() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let _ = fs::remove_file(Path::new(&spec.workspace.out_dir).join("artifacts/gaia"));
    let _ = fs::remove_file(Path::new(&spec.workspace.out_dir).join("images/default-2.0.0.tar"));
    let _ = fs::remove_file(Path::new(&spec.workspace.out_dir).join("images/image-provider.txt"));

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert!(outcome.errors.is_empty());

    let git_source = fs::read_to_string(
        Path::new(&spec.workspace.build_dir).join("sources/gaia-upstream/source.txt"),
    )
    .expect("git source marker");
    assert!(git_source.contains("provider=source.git"));

    let path_source = fs::read_to_string(
        Path::new(&spec.workspace.build_dir).join("sources/workspace-root/source.txt"),
    )
    .expect("path source marker");
    assert!(path_source.contains("provider=source.path"));
    let path_source_state = fs::read_to_string(
        Path::new(&spec.workspace.build_dir).join("sources/workspace-root/.gaia-source-state.txt"),
    )
    .expect("path source state");
    assert!(path_source_state.contains("provider=source.path"));

    assert!(
        Path::new(&spec.workspace.out_dir)
            .join("artifacts/gaia")
            .is_file()
    );
    let artifact_state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join("artifacts/.gaia/gaia.gaia-state.txt"),
    )
    .expect("artifact state");
    assert!(artifact_state.contains("provider=artifact.rust"));
    assert!(artifact_state.contains("output_sha256="));
    assert!(artifact_state.contains("output_bytes="));
    let artifact_marker = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join("artifacts/.gaia/gaia.gaia-build.txt"),
    )
    .expect("artifact marker");
    assert!(artifact_marker.contains("provider=artifact.rust"));

    let image_marker =
        fs::read_to_string(Path::new(&spec.workspace.out_dir).join("images/image-provider.txt"))
            .expect("image marker");
    assert!(image_marker.contains("provider=image.buildroot"));
    let image_state =
        fs::read_to_string(Path::new(&spec.workspace.out_dir).join("images/.gaia-image-state.txt"))
            .expect("image state");
    assert!(image_state.contains("provider=image.buildroot"));
    assert!(image_state.contains("collect_digest="));
    assert!(image_state.contains("archive_sha256="));

    let archive_path = Path::new(&spec.workspace.out_dir).join("images/default-2.0.0.tar");
    assert!(
        archive_path.is_file(),
        "expected fallback image archive to exist"
    );
    let tar_listing = Command::new("tar")
        .arg("-tf")
        .arg(&archive_path)
        .output()
        .expect("list fallback image tar");
    assert!(
        tar_listing.status.success(),
        "expected fallback image to be a valid tar archive: {}",
        String::from_utf8_lossy(&tar_listing.stderr)
    );
    let tar_listing = String::from_utf8_lossy(&tar_listing.stdout);
    assert!(tar_listing.contains("./usr/bin/default") || tar_listing.contains("usr/bin/default"));
    assert!(tar_listing.contains("./etc/motd") || tar_listing.contains("etc/motd"));
}
