use super::*;
use crate::{ExecutionPlan, OperationId, PlannedOperation, operation_components};
use gaia_spec::{
    ArtifactOutputSpec, ArtifactSpec, BuildModeSpec, BuildrootExternalTreeModeSpec,
    BuildrootImageSpec, GitBackendSpec, GitSourceSpec, ImageSpec, JavaArtifactSpec,
    SourcePinPolicySpec, SourceRefreshPolicySpec,
};
use std::collections::BTreeSet;

/// A spec whose fingerprints read nothing from disk or the network: a
/// workspace that does not exist, a remote git source pinned by `rev`, a
/// Java artifact without scripts, and a Buildroot image.
fn spec() -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("fingerprints");
    spec.workspace.root_dir = "/nonexistent/gaia-fingerprint-test".into();
    spec.sources.push(SourceSpec {
        id: "upstream".into(),
        definition: SourceDefinition::Git(GitSourceSpec {
            repo: "https://example.invalid/upstream.git".into(),
            branch: None,
            tag: None,
            rev: Some("0123456789abcdef".into()),
            subdir: None,
            update: false,
            refresh_policy: SourceRefreshPolicySpec::Auto,
            pin_policy: SourcePinPolicySpec::Locked,
            locked_commit: None,
            git_backend: GitBackendSpec::Host,
        }),
        variant_scope: None,
    });
    spec.artifacts.push(ArtifactSpec::new(
        "app",
        ArtifactDefinition::Java(JavaArtifactSpec {
            build_target: "jar".into(),
            build_args: vec!["--offline".into()],
            build_command: Vec::new(),
            build_env: vec![("GRADLE_OPTS".into(), "-Xmx1g".into())],
        }),
        None,
        ArtifactOutputSpec {
            path: "out/app.jar".into(),
        },
    ));
    spec.image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        defconfig: Some("raspberrypi4_64_defconfig".into()),
        config_overrides: vec![("BR2_TARGET_GENERIC_ISSUE".into(), "\"gaia\"".into())],
        ..BuildrootImageSpec::default()
    }));
    spec
}

fn kinds() -> Vec<OperationKind> {
    vec![
        OperationKind::ResolveBuild,
        OperationKind::MaterializeSource {
            source_id: "upstream".into(),
        },
        OperationKind::BuildArtifact {
            artifact_id: "app".into(),
        },
        OperationKind::PrepareImage,
        OperationKind::BuildImage,
        OperationKind::EmitReport,
    ]
}

fn fingerprints(spec: &ResolvedBuildSpec) -> Vec<u64> {
    kinds()
        .iter()
        .map(|kind| operation_fingerprint(spec, kind))
        .collect()
}

/// A named edit of a spec part.
type Edit<T> = (&'static str, fn(&mut T));

/// A named image edit, and whether it also changes image:prepare.
type ImageEdit = (&'static str, fn(&mut ImageSpec), bool);

const CHILD_ENV: &str = "GAIA_FINGERPRINT_TEST_CHILD";

/// The fingerprints another process computes for the same spec are the same:
/// nothing in them is seeded per process.
#[test]
fn fingerprints_are_identical_across_processes() {
    let own = fingerprints(&spec());
    if std::env::var_os(CHILD_ENV).is_some() {
        println!("FINGERPRINTS={own:?}");
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "reuse_inputs::tests::fingerprints_are_identical_across_processes",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("child test process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let child = stdout
        .lines()
        .find_map(|line| line.split_once("FINGERPRINTS=").map(|(_, values)| values))
        .unwrap_or_else(|| panic!("no fingerprints printed: {stdout}"));
    assert_eq!(child, format!("{own:?}"));
}

/// Pinned so a change to the encoding (or to how a spec serializes) is
/// noticed: it must come with a `FINGERPRINT_SCHEME` bump. The report
/// operation reads only spec values, so the value is the same on any host.
#[test]
fn the_report_fingerprint_is_pinned() {
    assert_eq!(
        operation_fingerprint(
            &ResolvedBuildSpec::new("pinned"),
            &OperationKind::EmitReport
        ),
        PINNED_REPORT_FINGERPRINT
    );
}

const PINNED_REPORT_FINGERPRINT: u64 = 18209953397920241403;

/// Structured inputs are canonical JSON, never `Debug` text.
#[test]
fn inputs_are_canonical_json_not_debug_text() {
    let spec = spec();
    for kind in kinds() {
        for (name, text) in operation_inputs(&spec, &kind) {
            assert!(
                !text.contains("Spec {") && !text.contains("Some(") && !text.contains("SourceId("),
                "{name} of {kind:?} looks like Debug text: {text}"
            );
        }
    }
    let inputs = operation_inputs(&spec, &OperationKind::BuildImage);
    let defconfig = inputs
        .iter()
        .find(|(name, _)| name == "defconfig")
        .map(|(_, text)| text.as_str());
    assert_eq!(
        defconfig,
        Some(r#"{"defconfig":"raspberrypi4_64_defconfig"}"#)
    );
}

/// Each artifact field changes the artifact's fingerprint.
#[test]
fn each_artifact_field_changes_its_fingerprint() {
    let kind = OperationKind::BuildArtifact {
        artifact_id: "app".into(),
    };
    let edits: Vec<Edit<ArtifactSpec>> = vec![
        ("source", |artifact| {
            artifact.source = Some(gaia_spec::SourceRef::new("upstream"))
        }),
        ("extra_sources", |artifact| {
            artifact.extra_sources = vec!["upstream".into()]
        }),
        ("execution", |artifact| {
            artifact.execution = Some(gaia_spec::ArtifactExecutionSpec::Host)
        }),
        ("target", |artifact| {
            artifact.target = Some("aarch64".into())
        }),
        ("build_mode", |artifact| {
            artifact.build_mode = Some(BuildModeSpec::Release)
        }),
        ("after_image_prepare", |artifact| {
            artifact.after_image_prepare = true
        }),
        ("output", |artifact| {
            artifact.output.path = "out/other.jar".into()
        }),
        ("build_target", |artifact| {
            if let ArtifactDefinition::Java(java) = &mut artifact.definition {
                java.build_target = "shadowJar".into();
            }
        }),
        ("build_args", |artifact| {
            if let ArtifactDefinition::Java(java) = &mut artifact.definition {
                java.build_args.clear();
            }
        }),
        ("build_env", |artifact| {
            if let ArtifactDefinition::Java(java) = &mut artifact.definition {
                java.build_env[0].1 = "-Xmx2g".into();
            }
        }),
    ];
    assert_each_edit_changes(&kind, edits, |spec| &mut spec.artifacts[0]);
}

/// Each git source field changes the source's fingerprint, except where git
/// runs, which never changes the checkout.
#[test]
fn each_git_source_field_changes_its_fingerprint() {
    let kind = OperationKind::MaterializeSource {
        source_id: "upstream".into(),
    };
    let edits: Vec<Edit<GitSourceSpec>> = vec![
        ("repo", |git| {
            git.repo = "https://example.invalid/other.git".into()
        }),
        ("rev", |git| git.rev = Some("fedcba9876543210".into())),
        ("branch", |git| git.branch = Some("main".into())),
        ("tag", |git| git.tag = Some("v1".into())),
        ("subdir", |git| git.subdir = Some("sub".into())),
        ("update", |git| git.update = true),
        ("refresh_policy", |git| {
            git.refresh_policy = SourceRefreshPolicySpec::Never
        }),
        ("locked_commit", |git| {
            git.locked_commit = Some("abc".into())
        }),
    ];
    assert_each_edit_changes(&kind, edits, |spec| {
        let SourceDefinition::Git(git) = &mut spec.sources[0].definition else {
            unreachable!("the test source is a git source");
        };
        git
    });

    let mut spec = spec();
    let before = operation_fingerprint(&spec, &kind);
    if let SourceDefinition::Git(git) = &mut spec.sources[0].definition {
        git.git_backend = GitBackendSpec::Execution;
    }
    assert_eq!(before, operation_fingerprint(&spec, &kind));
    spec.sources[0].variant_scope = Some("v2".into());
    assert_ne!(before, operation_fingerprint(&spec, &kind));
}

/// Each Buildroot field changes both image fingerprints, except the inputs
/// only image:build applies, and the assembly, which neither does.
#[test]
fn image_fields_change_the_fingerprints_of_the_steps_that_apply_them() {
    let edits: Vec<ImageEdit> = vec![
        ("defconfig", |image| buildroot(image).defconfig = None, true),
        (
            "config_overrides",
            |image| buildroot(image).config_overrides[0].1 = "\"other\"".into(),
            true,
        ),
        (
            "config_fragments",
            |image| buildroot(image).config_fragments = vec!["a.fragment".into()],
            true,
        ),
        (
            "allow_fallback",
            |image| buildroot(image).allow_fallback = true,
            true,
        ),
        (
            "external_tree_mode",
            |image| buildroot(image).external_tree_mode = BuildrootExternalTreeModeSpec::Disabled,
            true,
        ),
        (
            "rootfs_exclude_packages",
            |image| buildroot(image).rootfs_exclude_packages = vec!["busybox".into()],
            false,
        ),
        (
            "output",
            |image| image.output.archive_name = Some("other.tar".into()),
            false,
        ),
        (
            "feed",
            |image| image.feed.stage_files = vec!["motd".into()],
            false,
        ),
    ];
    let base = spec();
    let prepare = operation_fingerprint(&base, &OperationKind::PrepareImage);
    let build = operation_fingerprint(&base, &OperationKind::BuildImage);
    for (name, edit, applies_to_prepare) in edits {
        let mut spec = base.clone();
        edit(&mut spec.image);
        assert_ne!(
            build,
            operation_fingerprint(&spec, &OperationKind::BuildImage),
            "{name} must change image:build"
        );
        assert_eq!(
            applies_to_prepare,
            prepare != operation_fingerprint(&spec, &OperationKind::PrepareImage),
            "{name} and image:prepare"
        );
    }
    let mut spec = base.clone();
    spec.image.assembly = Some(gaia_spec::ImageAssemblySpec {
        out_dir: Some(gaia_spec::AssemblyPathTemplate::new("$assembly.work/out")),
        ..Default::default()
    });
    assert_eq!(
        prepare,
        operation_fingerprint(&spec, &OperationKind::PrepareImage)
    );
    assert_eq!(
        build,
        operation_fingerprint(&spec, &OperationKind::BuildImage)
    );
}

fn buildroot(image: &mut ImageSpec) -> &mut BuildrootImageSpec {
    match &mut image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot,
        ImageDefinition::StartingPoint(_) => unreachable!("the test image is Buildroot"),
    }
}

/// Every edit changes the fingerprint, and no two edits collide.
fn assert_each_edit_changes<T>(
    kind: &OperationKind,
    edits: Vec<Edit<T>>,
    target: fn(&mut ResolvedBuildSpec) -> &mut T,
) {
    let base = spec();
    let mut seen = BTreeSet::from([operation_fingerprint(&base, kind)]);
    for (name, edit) in edits {
        let mut spec = base.clone();
        edit(target(&mut spec));
        assert!(
            seen.insert(operation_fingerprint(&spec, kind)),
            "changing {name} did not change the fingerprint of {kind:?}"
        );
    }
}

/// The recorded components are the fingerprint's inputs, digested, plus the
/// dependencies: a changed input changes a component of the same name.
#[test]
fn components_are_the_fingerprint_inputs_plus_dependencies() {
    let spec = spec();
    let mut operation = PlannedOperation::new(
        OperationId::new("artifact:app"),
        OperationKind::BuildArtifact {
            artifact_id: "app".into(),
        },
    );
    operation.depends_on = vec![OperationId::new("source:upstream")];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![operation.clone()],
    };
    let inputs = operation_inputs(&spec, &operation.kind);
    let components = operation_components(&spec, &plan, &operation);
    assert_eq!(components.len(), inputs.len() + 1);
    for ((input, text), (component, digest)) in inputs.iter().zip(&components) {
        assert_eq!(input, component);
        assert_eq!(*digest, gaia_spec::sha256_hex(text));
    }
    assert_eq!(
        components.last().map(|(name, _)| name.as_str()),
        Some("dependency source:upstream")
    );

    let mut changed = spec.clone();
    if let ArtifactDefinition::Java(java) = &mut changed.artifacts[0].definition {
        java.build_env[0].1 = "-Xmx2g".into();
    }
    let after = operation_components(&changed, &plan, &operation);
    let differing = components
        .iter()
        .zip(&after)
        .filter(|(before, after)| before != after)
        .map(|((name, _), _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(differing, ["build_env", "build_env[GRADLE_OPTS]"]);
}
