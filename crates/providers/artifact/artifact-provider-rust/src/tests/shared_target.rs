//! Shared cargo target dirs (`[providers.rust] shared_target_dir`).

use super::*;
use gaia_artifact_providers::ArtifactDockerExecution;

fn shared_contract(artifact: &ArtifactSpec, source: &Path) -> ArtifactExecutionContract {
    let mut contract = nested_contract(artifact, source);
    contract.rust_shared_target_dir = true;
    contract
}

fn flags() -> CargoFeatureFlags {
    CargoFeatureFlags::default()
}

/// Builds `package` of `workspace` into `target` the way `execute_artifact`
/// does, and returns the built binary's output for `run` to show it.
fn build_into(
    workspace: &Path,
    package: &str,
    contract: &ArtifactExecutionContract,
    target: &CargoTarget,
) -> String {
    let source = workspace.display().to_string();
    run_cargo_build(
        &source,
        &[package.to_string()],
        &flags(),
        contract,
        target,
        None,
        None,
    )
    .expect("cargo build succeeds");
    collect_cargo_output(&source, &target.dir, package, contract, package)
        .expect("output collected");
    let output = Command::new(artifact_output_path(contract, &source))
        .output()
        .expect("built binary runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn per_source_target_dir_stays_the_default() {
    let source = temp_path("gaia-rust-target-default");
    let artifact = rust_artifact("a", "a", &source.join("out/a"));
    let contract = nested_contract(&artifact, &source);

    let target = cargo_target_for_in(
        Some(&temp_path("gaia-rust-target-default-cache")),
        &source.display().to_string(),
        &contract,
    );

    assert_eq!(target.dir, source.join(".gaia/cargo-target"));
    assert!(!target.shared);
    assert!(target.note.is_none());
}

#[test]
fn shared_key_depends_on_toolchain_target_profile_and_backend_only() {
    let source = temp_path("gaia-rust-target-key");
    let artifact = rust_artifact("a", "a", &source.join("out/a"));
    let base = nested_contract(&artifact, &source);
    let key = shared_target_key(&base);
    assert_eq!(key.len(), 24, "{key}");

    // The source directory is not part of the key: that is what lets
    // sources share one dir.
    let elsewhere = nested_contract(&artifact, &temp_path("gaia-rust-target-key-other"));
    assert_eq!(key, shared_target_key(&elsewhere));

    let mut release = base.clone();
    release.build_mode = Some(BuildModeSpec::Release);
    assert_ne!(key, shared_target_key(&release));

    let mut cross = base.clone();
    cross.artifact_target = Some("aarch64-unknown-linux-gnu".into());
    assert_ne!(key, shared_target_key(&cross));

    let docker = |image: &str| {
        let mut contract = base.clone();
        contract.execution_backend = ArtifactExecutionBackend::Docker(ArtifactDockerExecution {
            image: image.into(),
            build: None,
        });
        shared_target_key(&contract)
    };
    let image = docker("gaia-rust:1.99.0");
    assert_ne!(key, image);
    assert_ne!(image, docker("gaia-rust:1.98.0"));
    assert_eq!(image, docker("gaia-rust:1.99.0"));
}

#[test]
fn shared_target_dir_lives_under_the_user_cache_and_claims_its_packages() {
    let workspace = tiny_workspace("gaia-rust-target-shared", &[("solo", false)]);
    let root = temp_path("gaia-rust-target-shared-cache");
    let artifact = rust_artifact("solo", "solo", &workspace.join("out/solo"));
    let contract = shared_contract(&artifact, &workspace);

    let target = cargo_target_for_in(Some(&root), &workspace.display().to_string(), &contract);

    assert!(target.shared, "{:?}", target.note);
    assert_eq!(
        target.dir,
        root.join("cargo-target").join(shared_target_key(&contract))
    );
    assert!(
        fs::read_to_string(target.dir.join(SHARED_OWNERS_DIR).join("solo-0.1.0"))
            .expect("claim written")
            .ends_with("/solo"),
        "the claim records the package directory"
    );
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shared_target_dir_needs_a_user_cache() {
    let workspace = tiny_workspace("gaia-rust-target-no-cache", &[("solo", false)]);
    let artifact = rust_artifact("solo", "solo", &workspace.join("out/solo"));
    let contract = shared_contract(&artifact, &workspace);

    let target = cargo_target_for_in(None, &workspace.display().to_string(), &contract);

    assert!(!target.shared);
    assert_eq!(target.dir, workspace.join(".gaia/cargo-target"));
    assert!(
        target
            .note
            .as_deref()
            .is_some_and(|note| note.contains("user cache directory")),
        "{:?}",
        target.note
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn shared_target_dir_is_mounted_into_docker_containers() {
    let workspace = tiny_workspace("gaia-rust-target-docker", &[("solo", false)]);
    let root = temp_path("gaia-rust-target-docker-cache");
    let artifact = rust_artifact("solo", "solo", &workspace.join("out/solo"));
    let mut contract = shared_contract(&artifact, &workspace);
    contract.workspace_root = Some(workspace.display().to_string());
    contract.execution_backend = ArtifactExecutionBackend::Docker(ArtifactDockerExecution {
        image: "gaia-rust:test".into(),
        build: None,
    });
    let target = cargo_target_for_in(Some(&root), &workspace.display().to_string(), &contract);
    assert!(target.shared, "{:?}", target.note);
    prepare_cargo_target(&target).expect("shared dir prepared");
    let command = cargo_build_command(
        &workspace.display().to_string(),
        &["solo".to_string()],
        &flags(),
        &contract,
        &target.dir,
    );

    let docker = gaia_artifact_providers::command_for_execution(&command, &contract)
        .expect("docker command");
    let args = docker
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let dir = target.dir.display().to_string();
    assert!(
        args.iter().any(|arg| arg.contains(&dir)),
        "the shared target dir is mounted at its own path: {args:?}"
    );
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn recorded_state_does_not_depend_on_the_target_dir_mode() {
    let source = temp_path("gaia-rust-target-state");
    let artifact = rust_artifact("a", "a", &source.join("out/a"));
    let per_source = nested_contract(&artifact, &source);
    let shared = shared_contract(&artifact, &source);

    assert_eq!(
        artifact_state_contents(
            "artifact.rust",
            "a",
            &per_source,
            "a",
            "a",
            "cargo",
            &flags()
        ),
        artifact_state_contents("artifact.rust", "a", &shared, "a", "a", "cargo", &flags()),
    );
}

#[test]
fn distinct_packages_share_one_target_dir_and_each_gets_its_own_output() {
    let root = temp_path("gaia-rust-target-two-cache");
    let first = tiny_workspace("gaia-rust-target-two-first", &[("alpha", false)]);
    let second = tiny_workspace("gaia-rust-target-two-second", &[("beta", false)]);
    fs::write(
        second.join("beta/src/main.rs"),
        "fn main() { println!(\"beta\"); }\n",
    )
    .expect("beta main");
    fs::write(
        first.join("alpha/src/main.rs"),
        "fn main() { println!(\"alpha\"); }\n",
    )
    .expect("alpha main");
    let alpha_contract = shared_contract(
        &rust_artifact("alpha", "alpha", &first.join("out/alpha")),
        &first,
    );
    let beta_contract = shared_contract(
        &rust_artifact("beta", "beta", &second.join("out/beta")),
        &second,
    );

    let alpha_target =
        cargo_target_for_in(Some(&root), &first.display().to_string(), &alpha_contract);
    let beta_target =
        cargo_target_for_in(Some(&root), &second.display().to_string(), &beta_contract);

    assert!(alpha_target.shared && beta_target.shared);
    assert_eq!(alpha_target.dir, beta_target.dir);
    assert_eq!(
        build_into(&first, "alpha", &alpha_contract, &alpha_target),
        "alpha"
    );
    assert_eq!(
        build_into(&second, "beta", &beta_contract, &beta_target),
        "beta"
    );
    // Rebuilding the first source after the second keeps its own output.
    assert_eq!(
        build_into(&first, "alpha", &alpha_contract, &alpha_target),
        "alpha"
    );
    let _ = fs::remove_dir_all(first);
    let _ = fs::remove_dir_all(second);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn same_named_package_from_another_directory_never_reuses_the_first_build() {
    // Both packages are `app` 0.1.0 with identical manifests and older
    // sources than the first build's outputs: cargo would consider the
    // second one fresh and link the first binary, were it not for the claim.
    let root = temp_path("gaia-rust-target-collide-cache");
    let first = tiny_workspace("gaia-rust-target-collide-first", &[("app", false)]);
    let second = tiny_workspace("gaia-rust-target-collide-second", &[("app", false)]);
    fs::write(
        first.join("app/src/main.rs"),
        "fn main() { println!(\"first\"); }\n",
    )
    .expect("first main");
    fs::write(
        second.join("app/src/main.rs"),
        "fn main() { println!(\"second\"); }\n",
    )
    .expect("second main");
    let first_contract =
        shared_contract(&rust_artifact("app", "app", &first.join("out/app")), &first);
    let second_contract = shared_contract(
        &rust_artifact("app", "app", &second.join("out/app")),
        &second,
    );

    let first_target =
        cargo_target_for_in(Some(&root), &first.display().to_string(), &first_contract);
    assert!(first_target.shared, "{:?}", first_target.note);
    assert_eq!(
        build_into(&first, "app", &first_contract, &first_target),
        "first"
    );

    let second_target =
        cargo_target_for_in(Some(&root), &second.display().to_string(), &second_contract);
    assert!(!second_target.shared);
    assert_eq!(second_target.dir, second.join(".gaia/cargo-target"));
    assert!(
        second_target
            .note
            .as_deref()
            .is_some_and(|note| note.contains("built from")),
        "{:?}",
        second_target.note
    );
    assert_eq!(
        build_into(&second, "app", &second_contract, &second_target),
        "second"
    );
    let _ = fs::remove_dir_all(first);
    let _ = fs::remove_dir_all(second);
    let _ = fs::remove_dir_all(root);
}
