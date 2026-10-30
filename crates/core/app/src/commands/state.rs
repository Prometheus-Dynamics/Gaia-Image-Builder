use gaia_exec::ExecutionOutcome;
use gaia_plan::{ExecutionPlan, ReuseState, spec_fingerprint};
use gaia_spec::{FINGERPRINT_SCHEME, ResolvedBuildSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

/// Reuse-state line prefix for the last wall-clock duration of an operation
/// that executed: `dur=<operation id>;<milliseconds>`.
const DURATION_PREFIX: &str = "dur=";

/// Reuse-state line naming the fingerprint scheme the state was recorded
/// under: `scheme=<gaia_spec::FINGERPRINT_SCHEME>`.
const SCHEME_PREFIX: &str = "scheme=";

/// Last recorded wall-clock duration (ms) per operation id. Read tolerantly:
/// malformed lines are skipped, and durations are kept even when the rest of
/// the state no longer matches the spec, since they are only estimates.
pub(crate) fn load_operation_durations(spec: &ResolvedBuildSpec) -> BTreeMap<String, u64> {
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

/// The named inputs an operation had when it last finished, and the operation
/// fingerprint they were recorded under. Read from the `.details` sidecar of
/// the reuse state, which older Gaia versions never read or write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedComponents {
    pub fingerprint: u64,
    /// `(name, digest)` pairs as `gaia_plan::operation_components` lists them.
    pub components: Vec<(String, String)>,
}

/// Recorded inputs per operation id. Missing or unreadable details yield an
/// empty map, so the preview falls back to naming no input.
pub(crate) fn load_operation_components(
    spec: &ResolvedBuildSpec,
) -> BTreeMap<String, RecordedComponents> {
    fs::read_to_string(reuse_details_path(spec))
        .map(|contents| parse_operation_components(&contents))
        .unwrap_or_default()
}

/// Lines are `op\t<operation id>\t<fingerprint>` and
/// `part\t<operation id>\t<name>\t<digest>`. Parts of an unknown operation and
/// malformed lines are skipped.
fn parse_operation_components(contents: &str) -> BTreeMap<String, RecordedComponents> {
    let mut records = BTreeMap::<String, RecordedComponents>::new();
    for line in contents.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["op", id, fingerprint] => {
                if let Ok(fingerprint) = fingerprint.parse::<u64>()
                    && !id.is_empty()
                {
                    records.insert(
                        (*id).to_string(),
                        RecordedComponents {
                            fingerprint,
                            components: Vec::new(),
                        },
                    );
                }
            }
            ["part", id, name, digest] => {
                if let Some(record) = records.get_mut(*id) {
                    record
                        .components
                        .push(((*name).to_string(), (*digest).to_string()));
                }
            }
            _ => {}
        }
    }
    records
}

/// Appends one operation's recorded components to the details body.
fn push_operation_components(
    body: &mut String,
    id: &str,
    fingerprint: u64,
    components: &[(String, String)],
) {
    body.push_str(&format!("op\t{}\t{fingerprint}\n", record_field(id)));
    for (name, digest) in components {
        body.push_str(&format!(
            "part\t{}\t{}\t{}\n",
            record_field(id),
            record_field(name),
            record_field(digest)
        ));
    }
}

/// A tab or line break inside a recorded field would corrupt the line format.
fn record_field(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

fn reuse_details_path(spec: &ResolvedBuildSpec) -> PathBuf {
    PathBuf::from(format!("{}.details", reuse_state_path(spec).display()))
}

pub(crate) fn load_reuse_state(spec: &ResolvedBuildSpec) -> Option<ReuseState> {
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
                && !line.starts_with(SCHEME_PREFIX)
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
    // State without a scheme line predates recorded schemes (scheme 0), so
    // its values are never compared with this Gaia's.
    let fingerprint_scheme = lines
        .clone()
        .find_map(|line| line.strip_prefix(SCHEME_PREFIX))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);
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
        fingerprint_scheme,
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
pub(crate) fn save_reuse_state(
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
    let previous_components = load_operation_components(spec);
    let mut components_body = String::new();

    let mut body = format!(
        "fingerprint={}\n{SCHEME_PREFIX}{FINGERPRINT_SCHEME}\n",
        spec_fingerprint(spec)
    );
    for operation in &plan.operations {
        let id = operation.id.as_str();
        if !finished.contains(id) {
            continue;
        }
        body.push_str(id);
        body.push('\n');
        // Recorded from the state the run left, as the components are.
        let fingerprint = gaia_plan::recorded_fingerprint(spec, operation);
        body.push_str(&format!("op={id};{fingerprint}\n"));
        if let Some(signature) = gaia_plan::operation_output_signature(spec, &operation.kind) {
            body.push_str(&format!("out={id};{}\n", encode_signature(&signature)));
        }
        body.push_str(&format!(
            "in={id};{}\n",
            gaia_plan::operation_input_signature(spec, plan, operation)
        ));
        push_operation_components(
            &mut components_body,
            id,
            fingerprint,
            &gaia_plan::operation_components(spec, plan, operation),
        );
    }
    // Entries of another fingerprint scheme are dropped rather than carried
    // over: under this scheme's line they would read as current values.
    if let Some(previous) =
        previous.filter(|previous| previous.fingerprint_scheme == FINGERPRINT_SCHEME)
    {
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
            if let Some(record) = previous_components.get(id) {
                push_operation_components(
                    &mut components_body,
                    id,
                    record.fingerprint,
                    &record.components,
                );
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
    write_reuse_details(spec, &components_body);
}

/// Writes the components sidecar with the same write-then-rename as the state.
/// Each record carries its fingerprint, so a sidecar left behind by an older
/// run never describes a state entry it does not belong to.
fn write_reuse_details(spec: &ResolvedBuildSpec, body: &str) {
    let path = reuse_details_path(spec);
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
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
        .join(format!("{}.reuse-state", spec.state_name()))
}

#[cfg(test)]
mod tests;
