use gaia_exec::ExecutionOutcome;
use gaia_plan::{ExecutionPlan, ReuseState, spec_fingerprint};
use gaia_spec::ResolvedBuildSpec;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

/// Reuse-state line prefix for the last wall-clock duration of an operation
/// that executed: `dur=<operation id>;<milliseconds>`.
const DURATION_PREFIX: &str = "dur=";

/// Last recorded wall-clock duration (ms) per operation id. Read tolerantly:
/// malformed lines are skipped, and durations are kept even when the rest of
/// the state no longer matches the spec, since they are only estimates.
pub fn load_operation_durations(spec: &ResolvedBuildSpec) -> BTreeMap<String, u64> {
    fs::read_to_string(reuse_state_path(spec))
        .map(|contents| parse_operation_durations(&contents))
        .unwrap_or_default()
}

fn parse_operation_durations(contents: &str) -> BTreeMap<String, u64> {
    contents
        .lines()
        .filter_map(|line| line.strip_prefix(DURATION_PREFIX))
        .filter_map(|line| line.rsplit_once(';'))
        .filter_map(|(operation_id, value)| {
            let value = value.trim().parse::<u64>().ok()?;
            (!operation_id.is_empty()).then(|| (operation_id.to_string(), value))
        })
        .collect()
}

pub fn load_reuse_state(spec: &ResolvedBuildSpec) -> Option<ReuseState> {
    let path = reuse_state_path(spec);
    let contents = fs::read_to_string(path).ok()?;
    let mut lines = contents.lines();
    let fingerprint_line = lines.next()?;
    let fingerprint = fingerprint_line
        .strip_prefix("fingerprint=")?
        .parse::<u64>()
        .ok()?;
    let completed_operation_ids = lines
        .clone()
        .filter(|line| {
            !line.trim().is_empty()
                && !line.starts_with("op=")
                && !line.starts_with("out=")
                && !line.starts_with("in=")
                && !line.starts_with(DURATION_PREFIX)
        })
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let parse_u64_entries = |prefix: &str| {
        lines
            .clone()
            .filter_map(|line| line.strip_prefix(prefix))
            .filter_map(|line| line.split_once(';'))
            .filter_map(|(operation_id, value)| {
                value
                    .parse::<u64>()
                    .ok()
                    .map(|value| (operation_id.to_string(), value))
            })
            .collect::<BTreeMap<_, _>>()
    };
    let operation_fingerprints = parse_u64_entries("op=");
    let operation_input_signatures = parse_u64_entries("in=");
    let operation_output_signatures = lines
        .filter_map(|line| line.strip_prefix("out="))
        .filter_map(|line| line.split_once(';'))
        .map(|(operation_id, signature)| {
            (
                operation_id.to_string(),
                decode_signature(signature).unwrap_or_else(|| signature.to_string()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Some(ReuseState {
        spec_fingerprint: fingerprint,
        completed_operation_ids,
        operation_fingerprints,
        operation_output_signatures,
        operation_input_signatures,
    })
}

/// Persists reuse state after a run, including failed, cancelled and partial
/// (`--only`) runs, so finished work is never thrown away.
///
/// Operations that finished (built or reused, and not rolled back) are
/// recorded with their plan-time fingerprint, current output signature, and
/// the content signature of the inputs they consumed. Entries from `previous`
/// are kept for operations this run did not attempt; their recorded input
/// signature makes the planner rebuild them if something they depend on has
/// changed since.
pub fn save_reuse_state(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    outcome: &ExecutionOutcome,
    previous: Option<&ReuseState>,
) {
    let path = reuse_state_path(spec);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut durations = load_operation_durations(spec);
    for timing in &outcome.operation_timings {
        if timing.status == gaia_exec::OperationTimingStatus::Built {
            durations.insert(
                timing.operation_id.as_str().to_string(),
                u64::try_from(timing.duration.as_millis()).unwrap_or(u64::MAX),
            );
        }
    }
    let rolled_back = outcome
        .rolled_back_ids
        .iter()
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();
    let finished = outcome
        .completed_ids
        .iter()
        .chain(&outcome.reused_ids)
        .map(|id| id.as_str())
        .filter(|id| !rolled_back.contains(id))
        .collect::<BTreeSet<_>>();
    let attempted = attempted_operation_ids(outcome);

    let mut body = format!("fingerprint={}\n", spec_fingerprint(spec));
    for operation in &plan.operations {
        let id = operation.id.as_str();
        if !finished.contains(id) {
            continue;
        }
        body.push_str(id);
        body.push('\n');
        body.push_str(&format!("op={id};{}\n", operation.fingerprint));
        if let Some(signature) = gaia_plan::operation_output_signature(spec, &operation.kind) {
            body.push_str(&format!("out={id};{}\n", encode_signature(&signature)));
        }
        body.push_str(&format!(
            "in={id};{}\n",
            gaia_plan::operation_input_signature(spec, plan, operation)
        ));
    }
    if let Some(previous) = previous {
        for id in &previous.completed_operation_ids {
            if finished.contains(id.as_str())
                || attempted.contains(id.as_str())
                || rolled_back.contains(id.as_str())
            {
                continue;
            }
            body.push_str(id);
            body.push('\n');
            if let Some(fingerprint) = previous.operation_fingerprints.get(id) {
                body.push_str(&format!("op={id};{fingerprint}\n"));
            }
            if let Some(signature) = previous.operation_output_signatures.get(id) {
                body.push_str(&format!("out={id};{}\n", encode_signature(signature)));
            }
            if let Some(signature) = previous.operation_input_signatures.get(id) {
                body.push_str(&format!("in={id};{signature}\n"));
            }
        }
    }
    // Durations are estimates only: keep them for every operation, including
    // ones whose reuse entry was dropped, so `gaia plan` can still estimate.
    for (id, milliseconds) in &durations {
        body.push_str(&format!("{DURATION_PREFIX}{id};{milliseconds}\n"));
    }
    // Write-then-rename so an interrupted save never leaves a truncated file.
    let temporary = path.with_extension("reuse-state.tmp");
    if fs::write(&temporary, body).is_ok() && fs::rename(&temporary, &path).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

/// Operations this run started, failed, or cancelled; their previous outputs
/// may have been partially overwritten, so old entries must not be kept.
fn attempted_operation_ids(outcome: &ExecutionOutcome) -> BTreeSet<&str> {
    let mut attempted = outcome
        .events
        .iter()
        .filter_map(|event| match event {
            gaia_exec::ExecutionEvent::Started { operation_id } => Some(operation_id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    attempted.extend(
        outcome
            .errors
            .iter()
            .map(|error| error.operation_id.as_str()),
    );
    attempted.extend(outcome.cancelled_operation_id.iter().map(|id| id.as_str()));
    attempted
}

fn encode_signature(signature: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity("hex:".len() + signature.len() * 2);
    encoded.push_str("hex:");
    for byte in signature.as_bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_signature(signature: &str) -> Option<String> {
    let hex = signature.strip_prefix("hex:")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().as_chunks::<2>().0 {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn reuse_state_path(spec: &ResolvedBuildSpec) -> PathBuf {
    gaia_spec::resolve_workspace_path(&spec.workspace, &spec.workspace.out_dir)
        .unwrap_or_else(|_| {
            let path = PathBuf::from(&spec.workspace.out_dir);
            if path.is_absolute() {
                path
            } else {
                PathBuf::from(&spec.workspace.root_dir).join(path)
            }
        })
        .join(".gaia")
        .join(format!("{}.reuse-state", spec.build_name()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_config::{ResolveOptions, resolve_config_with_options};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(prefix: &str) -> String {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let count = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join("gaia-tests")
            .join(format!("{prefix}-{nonce}-{count}"))
            .display()
            .to_string()
    }

    fn test_spec() -> ResolvedBuildSpec {
        resolve_config_with_options(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../examples/default-workspace/configs/default.toml")
                .display()
                .to_string(),
            &ResolveOptions {
                explicit_overrides: vec![
                    ("workspace.build_dir".into(), unique_dir("gaia-state-build")),
                    ("workspace.out_dir".into(), unique_dir("gaia-state-out")),
                ],
                ..ResolveOptions::default()
            },
        )
    }

    #[test]
    fn load_reuse_state_ignores_malformed_entries_and_out_lines() {
        let spec = test_spec();
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
        let spec = test_spec();
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

        let spec = test_spec();
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
        let spec = test_spec();
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
        let spec = test_spec();
        let path = reuse_state_path(&spec);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("reuse state dir");
        }
        fs::write(&path, "fingerprint=not-a-number\nartifact:gaia-app\n")
            .expect("reuse state write");

        assert!(load_reuse_state(&spec).is_none());
    }

    #[test]
    fn save_carries_over_unattempted_operations_only() {
        let spec = test_spec();
        let previous = ReuseState {
            spec_fingerprint: spec_fingerprint(&spec),
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
}
