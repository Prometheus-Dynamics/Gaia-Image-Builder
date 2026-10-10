use super::*;
use gaia_spec::{ArtifactOutputSpec, ArtifactVariantSpec, RustArtifactSpec};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{nonce}"))
}

#[test]
fn rust_provider_fails_for_missing_source_dir_when_nested_build_enabled() {
    let output_path = temp_path("gaia-rust-provider-output");
    let missing_source_dir = temp_path("gaia-rust-provider-missing-source");
    let artifact = ArtifactSpec::new(
        "gaia-app",
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: "gaia".into(),
            target_name: Some("gaia".into()),
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: output_path.display().to_string(),
        },
    );
    let contract = ArtifactExecutionContract::from_spec(
        &artifact,
        Some(missing_source_dir.display().to_string()),
        true,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );

    let error = RustProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect_err("missing source dir should fail");

    assert_eq!(
        error.kind,
        gaia_artifact_providers::ArtifactProviderErrorKind::ToolStart
    );
    assert!(error.message.contains("failed to start cargo build"));
}

#[test]
fn rust_artifact_state_persists_backend_native_fields() {
    let output_path = temp_path("gaia-rust-provider-state");
    fs::write(&output_path, "artifact").expect("output");
    let mut artifact = ArtifactSpec::new(
        "gaia-app",
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: "gaia".into(),
            target_name: Some("gaia".into()),
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: output_path.display().to_string(),
        },
    );
    artifact.target = Some("aarch64-unknown-linux-gnu".into());
    let contract = ArtifactExecutionContract::from_spec(
        &artifact,
        None,
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );

    let state = artifact_state_contents(
        "artifact.rust",
        artifact.id.as_str(),
        &contract,
        "gaia",
        "gaia",
        "cargo",
        &CargoFeatureFlags::default(),
    );

    assert!(state.contains("resolved_identifier_kind=package-target"));
    assert!(state.contains("resolved_identifier=gaia:gaia"));
    assert!(state.contains("produced_filename="));
    assert!(state.contains("output_class=binary"));
    assert!(state.contains("build_tool=cargo"));
    assert!(state.contains("compiler_tool=rustc"));
    assert!(state.contains("artifact_target=aarch64-unknown-linux-gnu"));
}

#[test]
fn nested_rust_build_does_not_accept_stale_existing_output() {
    let output_path = temp_path("gaia-rust-provider-stale-output");
    fs::write(&output_path, "stale").expect("output");
    let missing_source_dir = temp_path("gaia-rust-provider-missing-source-with-output");
    let artifact = ArtifactSpec::new(
        "gaia-app",
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: "gaia".into(),
            target_name: Some("gaia".into()),
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: output_path.display().to_string(),
        },
    );
    let contract = ArtifactExecutionContract::from_spec(
        &artifact,
        Some(missing_source_dir.display().to_string()),
        true,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );

    let error = RustProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect_err("nested build should not reuse stale output");

    assert_eq!(
        error.kind,
        gaia_artifact_providers::ArtifactProviderErrorKind::ToolStart
    );
    assert!(error.message.contains("failed to start cargo build"));
}

fn rust_artifact(id: &str, package: &str, output: &Path) -> ArtifactSpec {
    ArtifactSpec::new(
        id,
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: package.into(),
            target_name: Some(package.into()),
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: output.display().to_string(),
        },
    )
}

fn nested_contract(artifact: &ArtifactSpec, source_dir: &Path) -> ArtifactExecutionContract {
    ArtifactExecutionContract::from_spec(
        artifact,
        Some(source_dir.display().to_string()),
        true,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    )
}

/// A two-member cargo workspace with dependency-free binaries, so the
/// build is fast and offline. `broken` members fail to compile.
fn tiny_workspace(prefix: &str, members: &[(&str, bool)]) -> PathBuf {
    let root = temp_path(prefix);
    let names = members
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [{names}]\nresolver = \"2\"\n"),
    )
    .expect("workspace manifest");
    for (name, broken) in members {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("src")).expect("member src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member manifest");
        let body = if *broken {
            "fn main() { let x: u32 = \"not a number\"; }\n".to_string()
        } else {
            format!("fn main() {{ println!(\"{name}\"); }}\n")
        };
        fs::write(dir.join("src/main.rs"), body).expect("member main");
    }
    root
}

fn capture_sink() -> (
    ProcessLogSink,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = lines.clone();
    let sink: ProcessLogSink = std::sync::Arc::new(move |line: ProcessLogLine| {
        captured.lock().expect("lines").push(line.line);
    });
    (sink, lines)
}

#[test]
fn batch_key_groups_only_compatible_nested_builds() {
    let source = temp_path("gaia-rust-batch-key");
    let alpha = rust_artifact("alpha", "alpha", &source.join("out/alpha"));
    let beta = rust_artifact("beta", "beta", &source.join("out/beta"));
    let alpha_contract = nested_contract(&alpha, &source);
    let beta_contract = nested_contract(&beta, &source);

    let alpha_key = RustProvider.batch_key(&alpha, &alpha_contract);
    assert!(alpha_key.is_some());
    assert_eq!(alpha_key, RustProvider.batch_key(&beta, &beta_contract));

    let mut release = beta_contract.clone();
    release.build_mode = Some(BuildModeSpec::Release);
    assert_ne!(alpha_key, RustProvider.batch_key(&beta, &release));
    let mut cross = beta_contract.clone();
    cross.artifact_target = Some("aarch64-unknown-linux-gnu".into());
    assert_ne!(alpha_key, RustProvider.batch_key(&beta, &cross));
    let mut other_source = beta_contract.clone();
    other_source.source_dir = Some(temp_path("other").display().to_string());
    assert_ne!(alpha_key, RustProvider.batch_key(&beta, &other_source));
    let mut placeholder = beta_contract;
    placeholder.allow_nested_build = false;
    assert_eq!(RustProvider.batch_key(&beta, &placeholder), None);
}

#[test]
fn cargo_build_command_selects_every_batched_package() {
    let source = temp_path("gaia-rust-batch-command");
    let alpha = rust_artifact("alpha", "alpha", &source.join("out/alpha"));
    let mut contract = nested_contract(&alpha, &source);
    contract.build_mode = Some(BuildModeSpec::Release);
    let command = cargo_build_command(
        &source.display().to_string(),
        &["alpha".to_string(), "beta".to_string()],
        &CargoFeatureFlags::default(),
        &contract,
        &source.join(".gaia/cargo-target"),
    );
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(&args[..5], ["build", "-p", "alpha", "-p", "beta"]);
    assert!(args.contains(&"--release".to_string()));
}

#[test]
fn batched_cargo_build_matches_individual_outputs_and_state() {
    let workspace = tiny_workspace("gaia-rust-batch-ok", &[("alpha", false), ("beta", false)]);
    let alpha = rust_artifact("alpha", "alpha", &workspace.join("out/alpha"));
    let beta = rust_artifact("beta", "beta", &workspace.join("out/beta"));
    let alpha_contract = nested_contract(&alpha, &workspace);
    let beta_contract = nested_contract(&beta, &workspace);
    let (leader_sink, leader_lines) = capture_sink();
    let (member_sink, member_lines) = capture_sink();

    let results = RustProvider.execute_artifact_batch(
        &[
            ArtifactBatchItem {
                artifact: &alpha,
                contract: &alpha_contract,
                log_sink: Some(leader_sink),
            },
            ArtifactBatchItem {
                artifact: &beta,
                contract: &beta_contract,
                log_sink: Some(member_sink),
            },
        ],
        None,
    );

    assert_eq!(results.len(), 2);
    for (result, artifact, contract, package) in [
        (&results[0], &alpha, &alpha_contract, "alpha"),
        (&results[1], &beta, &beta_contract, "beta"),
    ] {
        let messages = result.as_ref().expect("batched build succeeds");
        assert!(messages.last().expect("summary").ends_with("(cargo)"));
        assert!(workspace.join("out").join(package).is_file());
        let state = fs::read_to_string(gaia_artifact_providers::artifact_state_path(contract))
            .expect("state file");
        assert_eq!(
            state,
            artifact_state_contents(
                "artifact.rust",
                artifact.id.as_str(),
                contract,
                package,
                package,
                "cargo",
                &CargoFeatureFlags::default(),
            )
        );
    }
    let leader_lines = leader_lines.lock().expect("lines");
    assert!(
        leader_lines.iter().any(|line| line.contains("alpha"))
            && leader_lines.iter().any(|line| line.contains("beta")),
        "one cargo invocation compiles both packages: {leader_lines:?}"
    );
    assert!(
        member_lines
            .lock()
            .expect("lines")
            .iter()
            .any(|line| line.contains("cargo build batched"))
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn failed_batch_falls_back_to_individual_builds() {
    let workspace = tiny_workspace("gaia-rust-batch-fail", &[("good", false), ("bad", true)]);
    let good = rust_artifact("good", "good", &workspace.join("out/good"));
    let bad = rust_artifact("bad", "bad", &workspace.join("out/bad"));
    let good_contract = nested_contract(&good, &workspace);
    let bad_contract = nested_contract(&bad, &workspace);

    let results = RustProvider.execute_artifact_batch(
        &[
            ArtifactBatchItem {
                artifact: &good,
                contract: &good_contract,
                log_sink: None,
            },
            ArtifactBatchItem {
                artifact: &bad,
                contract: &bad_contract,
                log_sink: None,
            },
        ],
        None,
    );

    assert!(results[0].is_ok(), "{:?}", results[0]);
    assert!(workspace.join("out/good").is_file());
    let error = results[1].as_ref().expect_err("broken package fails");
    assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
    assert!(error.message.contains("'bad'"), "{}", error.message);
    let _ = fs::remove_dir_all(workspace);
}

fn with_features(
    mut artifact: ArtifactSpec,
    features: &[&str],
    no_default_features: bool,
    all_features: bool,
) -> ArtifactSpec {
    if let ArtifactDefinition::Rust(rust) = &mut artifact.definition {
        rust.features = features.iter().map(|feature| feature.to_string()).collect();
        rust.no_default_features = no_default_features;
        rust.all_features = all_features;
    }
    artifact
}

#[test]
fn cargo_command_passes_feature_flags_through() {
    let source = temp_path("gaia-rust-features-command");
    let artifact = with_features(
        rust_artifact("node", "orion-node", &source.join("out/node")),
        &["metrics", "tls"],
        true,
        false,
    );
    let contract = nested_contract(&artifact, &source);
    let args = cargo_build_command(
        &source.display().to_string(),
        &["orion-node".to_string()],
        &CargoFeatureFlags::of(&artifact),
        &contract,
        &source.join(".gaia/cargo-target"),
    )
    .get_args()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect::<Vec<_>>();
    assert_eq!(
        &args[..6],
        [
            "build",
            "-p",
            "orion-node",
            "--no-default-features",
            "--features",
            "metrics,tls"
        ]
    );

    let all = with_features(artifact, &[], false, true);
    let args = cargo_build_command(
        &source.display().to_string(),
        &["orion-node".to_string()],
        &CargoFeatureFlags::of(&all),
        &contract,
        &source.join(".gaia/cargo-target"),
    )
    .get_args()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect::<Vec<_>>();
    assert!(args.contains(&"--all-features".to_string()));
    assert!(!args.contains(&"--features".to_string()));
}

#[test]
fn validation_rejects_all_features_with_other_feature_flags() {
    let base = rust_artifact("node", "orion-node", Path::new("/tmp/out/node"));
    for artifact in [
        with_features(base.clone(), &["tls"], false, true),
        with_features(base.clone(), &[], true, true),
    ] {
        assert!(
            RustProvider
                .validate_artifact(&artifact)
                .iter()
                .any(|issue| issue.code == "rust_features_conflict")
        );
    }
    assert!(
        RustProvider
            .validate_artifact(&with_features(base, &["tls"], true, false))
            .is_empty()
    );
}

#[test]
fn batching_and_state_respect_feature_flags() {
    let source = temp_path("gaia-rust-features-batch");
    let plain = rust_artifact("a", "a", &source.join("out/a"));
    let featured = with_features(
        rust_artifact("b", "b", &source.join("out/b")),
        &["x"],
        false,
        false,
    );
    let plain_key = RustProvider.batch_key(&plain, &nested_contract(&plain, &source));
    let featured_key = RustProvider.batch_key(&featured, &nested_contract(&featured, &source));
    assert!(plain_key.is_some() && featured_key.is_some());
    assert_ne!(
        plain_key, featured_key,
        "mixed feature sets must not share a cargo build"
    );

    let contract = nested_contract(&plain, &source);
    let plain_state = artifact_state_contents(
        "artifact.rust",
        "a",
        &contract,
        "a",
        "a",
        "cargo",
        &CargoFeatureFlags::of(&plain),
    );
    assert!(!plain_state.contains("\nfeatures="), "{plain_state}");
    assert!(!plain_state.contains("default_features="), "{plain_state}");
    let featured_state = artifact_state_contents(
        "artifact.rust",
        "b",
        &contract,
        "b",
        "b",
        "cargo",
        &CargoFeatureFlags::of(&featured),
    );
    assert!(
        featured_state.contains("\nfeatures=x\n"),
        "{featured_state}"
    );
}

#[test]
fn no_default_features_reaches_cargo() {
    // The default feature makes the crate fail to compile, so the build
    // only succeeds when --no-default-features is passed through.
    let root = temp_path("gaia-rust-no-default-features");
    fs::create_dir_all(root.join("src")).expect("src");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"gated\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"broken\"]\nbroken = []\n\n[workspace]\n",
    )
    .expect("manifest");
    fs::write(
        root.join("src/main.rs"),
        "#[cfg(feature = \"broken\")]\ncompile_error!(\"default feature enabled\");\nfn main() {}\n",
    )
    .expect("main");
    let artifact = with_features(
        rust_artifact("gated", "gated", &root.join("out/gated")),
        &[],
        true,
        false,
    );
    let contract = nested_contract(&artifact, &root);

    RustProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect("build without default features succeeds");
    assert!(root.join("out/gated").is_file());

    let with_defaults = with_features(artifact, &[], false, false);
    let error = RustProvider
        .execute_artifact(&with_defaults, &contract, None, None)
        .expect_err("default features fail to compile");
    assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
    let _ = fs::remove_dir_all(root);
}

mod groups;
mod shared_target;
