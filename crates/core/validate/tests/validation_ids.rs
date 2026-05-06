use gaia_spec::{
    ArtifactDefinition, ArtifactOutputSpec, ArtifactSpec, CheckpointAnchorRef, CheckpointId,
    CheckpointPointSpec, CheckpointPolicy, CheckpointSpec, InstallEntrySpec, InstallId,
    ResolvedBuildSpec, SourceDefinition, SourceId, SourceSpec, StageFileSpec, StageItemId,
    WorkspaceNamedPathSpec, WorkspacePathKindSpec,
};
use gaia_validate::validate_spec;

#[test]
fn empty_public_ids_are_validation_errors() {
    let mut spec = ResolvedBuildSpec::new("empty-id-validation");
    spec.sources.push(SourceSpec::new(
        SourceId::new(""),
        SourceDefinition::Path(gaia_spec::PathSourceSpec {
            path: "src".into(),
            identity_ignore: Vec::new(),
            refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
            pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
        }),
    ));
    spec.artifacts.push(ArtifactSpec::new(
        "",
        ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "demo".into(),
            target_name: None,
            variant: gaia_spec::ArtifactVariantSpec::File,
        }),
        None,
        ArtifactOutputSpec {
            path: "out/demo".into(),
        },
    ));
    spec.install.entries.push(InstallEntrySpec {
        id: InstallId::new(""),
        artifact: gaia_spec::ArtifactRef::new(gaia_spec::ArtifactId::new("missing")),
        dest: "/usr/bin/demo".into(),
        replace: false,
        mode: None,
        owner: None,
        group: None,
    });
    spec.stage.files.push(StageFileSpec {
        id: StageItemId::new(""),
        src: "assets/demo".into(),
        dest: "/etc/demo".into(),
        mode: None,
        origin: gaia_spec::StageContentOriginSpec::Generated,
    });
    spec.checkpoints = CheckpointSpec {
        points: vec![CheckpointPointSpec {
            id: CheckpointId::new(""),
            backend: Some(gaia_spec::CheckpointBackendRef {
                backend: "local".into(),
            }),
            use_policy: CheckpointPolicy::Auto,
            upload_policy: CheckpointPolicy::Off,
            anchor: CheckpointAnchorRef::Image,
        }],
    };

    let report = validate_spec(&spec);
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"source_id_empty"));
    assert!(codes.contains(&"artifact_id_empty"));
    assert!(codes.contains(&"install_id_empty"));
    assert!(codes.contains(&"stage_item_id_empty"));
    assert!(codes.contains(&"checkpoint_id_empty"));
}

#[test]
fn workspace_paths_cannot_escape_workspace_or_alias_roots() {
    let mut spec = ResolvedBuildSpec::new("path-invariant-validation");
    spec.workspace.root_dir = "/repo".into();
    spec.workspace.named_paths.push(WorkspaceNamedPathSpec {
        alias: "assets".into(),
        path: "assets".into(),
        kind: WorkspacePathKindSpec::Host,
    });
    spec.sources.push(SourceSpec::new(
        SourceId::new("source-traversal"),
        SourceDefinition::Path(gaia_spec::PathSourceSpec {
            path: "../outside".into(),
            identity_ignore: Vec::new(),
            refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
            pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
        }),
    ));
    spec.sources.push(SourceSpec::new(
        SourceId::new("alias-traversal"),
        SourceDefinition::Archive(gaia_spec::ArchiveSourceSpec {
            path: "@assets/../../outside.tar".into(),
            strip_components: 0,
            refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
            pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
        }),
    ));

    let report = validate_spec(&spec);
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"path_source_invalid"));
    assert!(codes.contains(&"archive_source_invalid"));
}
