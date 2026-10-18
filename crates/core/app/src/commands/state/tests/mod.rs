use super::*;
use gaia_config::{ResolveOptions, resolve_config_with_options};
use gaia_testkit::TempDir;

#[path = "state_assembly_tests.rs"]
mod assembly_tests;

/// The spec's build and out dirs live in the returned guard, which the test must keep.
fn test_spec() -> (TempDir, ResolvedBuildSpec) {
    let scratch = TempDir::new("gaia-state");
    let spec = resolve_config_with_options(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/default-workspace/configs/default.toml")
            .display()
            .to_string(),
        &ResolveOptions {
            explicit_overrides: vec![
                (
                    "workspace.build_dir".into(),
                    scratch.join("build").display().to_string(),
                ),
                (
                    "workspace.out_dir".into(),
                    scratch.join("out").display().to_string(),
                ),
            ],
            ..ResolveOptions::default()
        },
    );
    (scratch, spec)
}

#[test]
fn load_reuse_state_ignores_malformed_entries_and_out_lines() {
    let (_scratch, spec) = test_spec();
    let path = reuse_state_path(&spec);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("reuse state dir");
    }
    fs::write(
        &path,
        concat!(
            "fingerprint=123\n",
            "artifact:gaia-app\n",
            "out=artifact:gaia-app;signature-1\n",
            "op=artifact:gaia-app;456\n",
            "op=broken-no-separator\n",
            "out=install:install-gaia-app;signature-2\n",
            "junk-line-without-prefix\n"
        ),
    )
    .expect("reuse state write");

    let state = load_reuse_state(&spec).expect("reuse state");

    assert_eq!(state.spec_fingerprint, 123);
    assert!(state.completed_operation_ids.contains("artifact:gaia-app"));
    assert!(
        state
            .completed_operation_ids
            .contains("junk-line-without-prefix")
    );
    assert!(
        !state
            .completed_operation_ids
            .contains("out=artifact:gaia-app;signature-1")
    );
    assert_eq!(
        state.operation_fingerprints.get("artifact:gaia-app"),
        Some(&456)
    );
    assert_eq!(
        state
            .operation_output_signatures
            .get("artifact:gaia-app")
            .map(String::as_str),
        Some("signature-1")
    );
    assert_eq!(
        state
            .operation_output_signatures
            .get("install:install-gaia-app")
            .map(String::as_str),
        Some("signature-2")
    );
}

#[test]
fn load_reuse_state_decodes_multiline_output_signatures() {
    let (_scratch, spec) = test_spec();
    let path = reuse_state_path(&spec);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("reuse state dir");
    }
    let signature = "state:provider=artifact.rust\noutput_sha256=abc123\n|deadbeef";
    fs::write(
        &path,
        format!(
            "fingerprint=123\nartifact:gaia-app\nop=artifact:gaia-app;456\nout=artifact:gaia-app;{}\n",
            encode_signature(signature)
        ),
    )
    .expect("reuse state write");

    let state = load_reuse_state(&spec).expect("reuse state");

    assert_eq!(
        state
            .operation_output_signatures
            .get("artifact:gaia-app")
            .map(String::as_str),
        Some(signature)
    );
}

#[test]
fn durations_are_parsed_tolerantly_and_not_taken_as_operation_ids() {
    let parsed = parse_operation_durations(concat!(
        "fingerprint=1\n",
        "dur=artifact:app;1500\n",
        "dur=image:build;not-a-number\n",
        "dur=broken-no-separator\n",
        "dur=;12\n",
        "dur=stage:a;b;7\n",
    ));
    assert_eq!(parsed.get("artifact:app"), Some(&1500));
    assert_eq!(parsed.get("stage:a;b"), Some(&7));
    assert_eq!(parsed.len(), 2);

    let (_scratch, spec) = test_spec();
    let path = reuse_state_path(&spec);
    fs::create_dir_all(path.parent().expect("parent")).expect("reuse state dir");
    fs::write(&path, "fingerprint=5\nartifact:app\ndur=artifact:app;10\n")
        .expect("reuse state write");
    let state = load_reuse_state(&spec).expect("reuse state");
    assert_eq!(
        state.completed_operation_ids,
        ["artifact:app".to_string()].into_iter().collect()
    );
}

#[test]
fn save_records_built_durations_and_keeps_older_ones() {
    let (_scratch, spec) = test_spec();
    let path = reuse_state_path(&spec);
    fs::create_dir_all(path.parent().expect("parent")).expect("reuse state dir");
    fs::write(
        &path,
        "fingerprint=5\ndur=image:build;90000\ndur=artifact:app;10\n",
    )
    .expect("reuse state write");
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: Vec::new(),
    };
    let timing = |id: &str, ms: u64, status| gaia_exec::OperationTiming {
        steps: Vec::new(),
        operation_id: gaia_plan::OperationId::new(id),
        started: std::time::Duration::ZERO,
        duration: std::time::Duration::from_millis(ms),
        paused: std::time::Duration::ZERO,
        status,
    };
    let outcome = ExecutionOutcome {
        operation_timings: vec![
            timing(
                "artifact:app",
                2500,
                gaia_exec::OperationTimingStatus::Built,
            ),
            timing("artifact:lib", 1, gaia_exec::OperationTimingStatus::Reused),
            timing("artifact:bad", 3, gaia_exec::OperationTimingStatus::Failed),
        ],
        ..ExecutionOutcome::default()
    };

    save_reuse_state(&spec, &plan, &outcome, None);
    let durations = load_operation_durations(&spec);

    assert_eq!(durations.get("artifact:app"), Some(&2500));
    assert_eq!(durations.get("image:build"), Some(&90000));
    assert!(!durations.contains_key("artifact:lib"));
    assert!(!durations.contains_key("artifact:bad"));
}

#[test]
fn load_reuse_state_returns_none_for_invalid_fingerprint() {
    let (_scratch, spec) = test_spec();
    let path = reuse_state_path(&spec);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("reuse state dir");
    }
    fs::write(&path, "fingerprint=not-a-number\nartifact:gaia-app\n").expect("reuse state write");

    assert!(load_reuse_state(&spec).is_none());
}

#[test]
fn save_carries_over_unattempted_operations_only() {
    let (_scratch, spec) = test_spec();
    let previous = ReuseState {
        spec_fingerprint: spec_fingerprint(&spec),
        fingerprint_scheme: gaia_spec::FINGERPRINT_SCHEME,
        completed_operation_ids: ["image:build".to_string(), "artifact:old".to_string()]
            .into_iter()
            .collect(),
        operation_fingerprints: [("image:build".to_string(), 77)].into_iter().collect(),
        operation_output_signatures: [("image:build".to_string(), "sig\nline".to_string())]
            .into_iter()
            .collect(),
        operation_input_signatures: [("image:build".to_string(), 5)].into_iter().collect(),
    };
    let artifact = gaia_plan::PlannedOperation::new(
        gaia_plan::OperationId::new("artifact:old"),
        gaia_plan::OperationKind::ResolveBuild,
    );
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![artifact],
    };
    // The planned artifact was attempted and failed this time, so its old
    // entry must not be carried over either.
    let outcome = ExecutionOutcome {
        events: vec![gaia_exec::ExecutionEvent::Started {
            operation_id: gaia_plan::OperationId::new("artifact:old"),
        }],
        ..ExecutionOutcome::default()
    };

    save_reuse_state(&spec, &plan, &outcome, Some(&previous));
    let state = load_reuse_state(&spec).expect("reuse state");

    assert!(state.completed_operation_ids.contains("image:build"));
    assert!(!state.completed_operation_ids.contains("artifact:old"));
    assert_eq!(state.operation_fingerprints.get("image:build"), Some(&77));
    assert_eq!(
        state.operation_input_signatures.get("image:build"),
        Some(&5)
    );
    assert_eq!(
        state
            .operation_output_signatures
            .get("image:build")
            .map(String::as_str),
        Some("sig\nline")
    );

    save_reuse_state(&spec, &plan, &outcome, None);
    let state = load_reuse_state(&spec).expect("reuse state");
    assert!(!state.completed_operation_ids.contains("image:build"));
}

fn started(id: &str) -> gaia_exec::ExecutionEvent {
    gaia_exec::ExecutionEvent::Started {
        operation_id: gaia_plan::OperationId::new(id),
    }
}

#[test]
fn save_keeps_completed_operations_of_a_failed_run_for_reuse() {
    let (_scratch, spec) = test_spec();
    let kept = gaia_plan::PlannedOperation::new(
        gaia_plan::OperationId::new("artifact:kept"),
        gaia_plan::OperationKind::ResolveBuild,
    );
    let kept_fingerprint = kept.fingerprint;
    let failed = gaia_plan::PlannedOperation::new(
        gaia_plan::OperationId::new("artifact:bad"),
        gaia_plan::OperationKind::ResolveBuild,
    );
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![kept, failed],
    };
    // The kept operation completed, so it also emitted `Started`; that
    // must not make it count as merely attempted.
    let outcome = ExecutionOutcome {
        completed_operations: 1,
        completed_ids: vec![gaia_plan::OperationId::new("artifact:kept")],
        events: vec![started("artifact:kept"), started("artifact:bad")],
        ..ExecutionOutcome::default()
    };

    save_reuse_state(&spec, &plan, &outcome, None);
    let state = load_reuse_state(&spec).expect("reuse state");

    assert!(state.completed_operation_ids.contains("artifact:kept"));
    assert!(!state.completed_operation_ids.contains("artifact:bad"));
    assert_eq!(
        state.operation_fingerprints.get("artifact:kept"),
        Some(&kept_fingerprint)
    );
}

#[test]
fn save_keeps_completed_operations_of_a_cancelled_run_for_reuse() {
    let (_scratch, spec) = test_spec();
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            gaia_plan::PlannedOperation::new(
                gaia_plan::OperationId::new("artifact:done"),
                gaia_plan::OperationKind::ResolveBuild,
            ),
            gaia_plan::PlannedOperation::new(
                gaia_plan::OperationId::new("artifact:slow"),
                gaia_plan::OperationKind::ResolveBuild,
            ),
        ],
    };
    let outcome = ExecutionOutcome {
        cancelled: true,
        cancelled_operation_id: Some(gaia_plan::OperationId::new("artifact:slow")),
        completed_operations: 1,
        completed_ids: vec![gaia_plan::OperationId::new("artifact:done")],
        events: vec![started("artifact:done"), started("artifact:slow")],
        ..ExecutionOutcome::default()
    };

    save_reuse_state(&spec, &plan, &outcome, None);
    let state = load_reuse_state(&spec).expect("reuse state");

    assert!(state.completed_operation_ids.contains("artifact:done"));
    assert!(!state.completed_operation_ids.contains("artifact:slow"));
}

#[test]
fn save_drops_rolled_back_operations_from_reuse_state() {
    let (_scratch, spec) = test_spec();
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![gaia_plan::PlannedOperation::new(
            gaia_plan::OperationId::new("artifact:unwound"),
            gaia_plan::OperationKind::ResolveBuild,
        )],
    };
    let outcome = ExecutionOutcome {
        completed_ids: vec![gaia_plan::OperationId::new("artifact:unwound")],
        rolled_back_ids: vec![gaia_plan::OperationId::new("artifact:unwound")],
        events: vec![started("artifact:unwound")],
        ..ExecutionOutcome::default()
    };

    save_reuse_state(&spec, &plan, &outcome, None);
    let state = load_reuse_state(&spec).expect("reuse state");

    assert!(!state.completed_operation_ids.contains("artifact:unwound"));
}

#[test]
fn components_round_trip_through_the_details_sidecar() {
    let (_scratch, spec) = test_spec();
    let context = crate::AppContext::with_defaults();
    let plan = gaia_plan::plan_build(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    let outcome = ExecutionOutcome {
        completed_ids: plan
            .operations
            .iter()
            .map(|operation| operation.id.clone())
            .collect(),
        ..ExecutionOutcome::default()
    };

    // Taken before the save: writing the state can touch a path source
    // tree, which would change the expected digests.
    let expected = plan
        .operations
        .iter()
        .map(|operation| {
            (
                operation.id.as_str().to_string(),
                gaia_plan::operation_components(&spec, &plan, operation),
            )
        })
        .collect::<BTreeMap<_, _>>();
    save_reuse_state(&spec, &plan, &outcome, None);
    let recorded = load_operation_components(&spec);

    assert_eq!(recorded.len(), plan.operations.len());
    assert!(
        recorded
            .values()
            .any(|record| !record.components.is_empty())
    );
    for operation in &plan.operations {
        let record = recorded
            .get(operation.id.as_str())
            .expect("every finished operation has a record");
        assert_eq!(record.fingerprint, operation.fingerprint);
        assert_eq!(&record.components, &expected[operation.id.as_str()]);
    }
    // The state file itself is unchanged in shape: it still loads.
    assert!(load_reuse_state(&spec).is_some());
}

#[test]
fn a_state_without_details_and_junk_in_the_details_still_load() {
    let (_scratch, spec) = test_spec();
    let path = reuse_state_path(&spec);
    fs::create_dir_all(path.parent().expect("parent")).expect("reuse state dir");
    fs::write(&path, "fingerprint=7\nimage:build\nop=image:build;9\n").expect("state");

    let details = reuse_details_path(&spec);
    let _ = fs::remove_file(&details);
    assert!(load_reuse_state(&spec).is_some());
    assert!(load_operation_components(&spec).is_empty());

    fs::write(
        &details,
        concat!(
            "junk\n",
            "part\timage:build\tdefconfig\tbeef\n",
            "op\timage:build\t9\n",
            "part\timage:build\tdefconfig\tabc\n",
            "part\tunknown:op\tdefconfig\tabc\n",
            "op\tbroken\tnot-a-number\n",
        ),
    )
    .expect("details");
    let recorded = load_operation_components(&spec);

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded.get("image:build"),
        Some(&RecordedComponents {
            fingerprint: 9,
            components: vec![("defconfig".to_string(), "abc".to_string())],
        })
    );
}

/// State written before fingerprint schemes were recorded loads without an
/// error, as scheme 0, so the planner re-evaluates it once.
#[test]
fn state_without_a_scheme_line_loads_as_scheme_zero() {
    let (_scratch, spec) = test_spec();
    let path = reuse_state_path(&spec);
    fs::create_dir_all(path.parent().expect("reuse state dir")).expect("reuse state dir");
    fs::write(
        &path,
        "fingerprint=123\nimage:build\nop=image:build;77\nin=image:build;5\n",
    )
    .expect("reuse state write");

    let state = load_reuse_state(&spec).expect("legacy reuse state");
    assert_eq!(state.fingerprint_scheme, 0);
    assert_eq!(
        state.completed_operation_ids,
        ["image:build".to_string()].into_iter().collect()
    );
    assert_eq!(state.operation_fingerprints.get("image:build"), Some(&77));
}

/// A save records the current scheme, and entries carried over from an
/// older scheme's state are dropped instead of being relabelled current.
#[test]
fn save_records_the_scheme_and_drops_entries_of_an_older_one() {
    let (_scratch, spec) = test_spec();
    let kept = gaia_plan::PlannedOperation::new(
        gaia_plan::OperationId::new("artifact:kept"),
        gaia_plan::OperationKind::ResolveBuild,
    );
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![kept],
    };
    let outcome = ExecutionOutcome {
        completed_operations: 1,
        completed_ids: vec![gaia_plan::OperationId::new("artifact:kept")],
        ..ExecutionOutcome::default()
    };
    let previous = ReuseState {
        spec_fingerprint: 1,
        fingerprint_scheme: 0,
        completed_operation_ids: ["image:build".to_string()].into_iter().collect(),
        operation_fingerprints: [("image:build".to_string(), 77)].into_iter().collect(),
        ..ReuseState::default()
    };

    save_reuse_state(&spec, &plan, &outcome, Some(&previous));
    let state = load_reuse_state(&spec).expect("reuse state");
    assert_eq!(state.fingerprint_scheme, gaia_spec::FINGERPRINT_SCHEME);
    assert!(state.completed_operation_ids.contains("artifact:kept"));
    assert!(!state.completed_operation_ids.contains("image:build"));
    assert!(
        !state
            .completed_operation_ids
            .iter()
            .any(|id| id.starts_with("scheme="))
    );

    // Carried over once the previous state is of the current scheme.
    let current = ReuseState {
        fingerprint_scheme: gaia_spec::FINGERPRINT_SCHEME,
        ..previous
    };
    save_reuse_state(&spec, &plan, &outcome, Some(&current));
    let state = load_reuse_state(&spec).expect("reuse state");
    assert_eq!(state.operation_fingerprints.get("image:build"), Some(&77));
}
