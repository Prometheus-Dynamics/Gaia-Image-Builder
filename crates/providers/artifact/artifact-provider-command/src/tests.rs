use super::*;
use gaia_testkit::TempDir;
use std::fs;

fn sh(script: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), script.into()]
}

fn artifact(
    id: &str,
    command: Vec<String>,
    collect: Option<&str>,
    exclude: &[&str],
    output: &Path,
) -> ArtifactSpec {
    ArtifactSpec::new(
        id,
        ArtifactDefinition::Command(CommandArtifactSpec {
            command,
            build_env: vec![("GREETING".into(), "hello".into())],
            collect: collect.map(str::to_string),
            collect_exclude: exclude.iter().map(|path| path.to_string()).collect(),
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: output.display().to_string(),
        },
    )
}

fn run(artifact: &ArtifactSpec, source: &Path) -> Result<Vec<String>, ArtifactProviderError> {
    let contract = ArtifactExecutionContract::from_spec(
        artifact,
        Some(source.display().to_string()),
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );
    CommandProvider.execute_artifact(artifact, &contract, None, None)
}

#[test]
fn collects_a_directory_without_the_excluded_paths() {
    let scratch = TempDir::new("gaia-command-collect-dir");
    let source = scratch.join("src");
    fs::create_dir_all(&source).expect("source");
    let output = scratch.join("out/ui");
    // A stale output from an earlier run is replaced, not merged.
    fs::create_dir_all(&output).expect("stale output");
    fs::write(output.join("stale.txt"), "old").expect("stale file");
    let artifact = artifact(
        "ui",
        sh(
            "mkdir -p dist/mock dist/assets && echo \"$GREETING\" > dist/index.html \
            && echo js > dist/assets/app.js && echo fake > dist/mock/data.json \
            && ln -s index.html dist/home.html",
        ),
        Some("./dist/"),
        &["mock/"],
        &output,
    );

    let messages = run(&artifact, &source).expect("command artifact");

    assert_eq!(
        fs::read_to_string(output.join("index.html")).expect("index"),
        "hello\n"
    );
    assert_eq!(
        fs::read_to_string(output.join("assets/app.js")).expect("asset"),
        "js\n"
    );
    assert!(
        !output.join("mock").exists(),
        "the excluded path is left out"
    );
    assert!(!output.join("stale.txt").exists(), "the old output is gone");
    assert_eq!(
        fs::read_link(output.join("home.html")).expect("link"),
        Path::new("index.html")
    );
    // Its state sits in the tree's `.gaia`, as for every directory artifact.
    let state = fs::read_to_string(output.join(".gaia/artifact.gaia-state.txt")).expect("state");
    assert!(state.contains("output_class=command-directory"), "{state}");
    assert!(state.contains("build_tool=command"), "{state}");
    assert!(output.join(".gaia/artifact.gaia-build.txt").is_file());
    assert!(messages[0].contains("(directory)"), "{messages:?}");
}

#[test]
fn the_command_writes_the_output_it_is_given() {
    let scratch = TempDir::new("gaia-command-env-output");
    let source = scratch.join("src");
    fs::create_dir_all(&source).expect("source");
    // A relative output resolves against the source dir; the command sees it
    // absolute.
    let artifact = artifact(
        "scene",
        sh(
            "case \"$GAIA_ARTIFACT_OUTPUT\" in /*) ;; *) exit 3 ;; esac; \
            printf '%s %s' \"$GREETING\" \"$(pwd)\" > \"$GAIA_ARTIFACT_OUTPUT\"",
        ),
        None,
        &[],
        Path::new("build/scene.bin"),
    );

    run(&artifact, &source).expect("command artifact");

    let output = source.join("build/scene.bin");
    let written = fs::read_to_string(&output).expect("output");
    assert!(written.starts_with("hello "), "{written}");
    assert!(
        written.ends_with("/src"),
        "the command runs in the source dir: {written}"
    );
    let state =
        fs::read_to_string(source.join("build/.gaia/scene.bin.gaia-state.txt")).expect("state");
    assert!(state.contains("output_class=command-file"), "{state}");
}

#[test]
fn a_missing_output_names_the_artifact() {
    let scratch = TempDir::new("gaia-command-missing-output");
    let source = scratch.join("src");
    fs::create_dir_all(&source).expect("source");
    let output = scratch.join("out/never");
    let error = run(
        &artifact("forgetful", sh("true"), None, &[], &output),
        &source,
    )
    .expect_err("no output");
    assert!(error.message.contains("'forgetful'"), "{}", error.message);
    assert!(
        error.message.contains("GAIA_ARTIFACT_OUTPUT"),
        "{}",
        error.message
    );

    let error = run(
        &artifact("no-dist", sh("true"), Some("dist"), &[], &output),
        &source,
    )
    .expect_err("no collect path");
    assert!(error.message.contains("'no-dist'"), "{}", error.message);
    assert!(
        error.message.contains("does not exist"),
        "{}",
        error.message
    );
}

#[test]
fn a_failing_command_fails_the_artifact() {
    let scratch = TempDir::new("gaia-command-failing");
    let source = scratch.join("src");
    fs::create_dir_all(&source).expect("source");
    let output = scratch.join("out/file");
    let error = run(
        &artifact(
            "broken",
            sh("echo partial > \"$GAIA_ARTIFACT_OUTPUT\"; exit 7"),
            None,
            &[],
            &output,
        ),
        &source,
    )
    .expect_err("failing command");
    assert!(error.message.contains("broken"), "{}", error.message);
}

#[test]
fn docker_execution_is_refused() {
    let output = Path::new("/nonexistent/out");
    let artifact = artifact("ui", sh("true"), None, &[], output);
    let mut contract = ArtifactExecutionContract::from_spec(
        &artifact,
        Some("/nonexistent".into()),
        false,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );
    contract.execution_backend =
        ArtifactExecutionBackend::Docker(gaia_artifact_providers::ArtifactDockerExecution {
            image: "alpine".into(),
            build: None,
        });
    let error = CommandProvider
        .execute_artifact(&artifact, &contract, None, None)
        .expect_err("docker");
    assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("host only"), "{}", error.message);
}
