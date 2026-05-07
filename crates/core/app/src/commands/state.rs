use gaia_exec::ExecutionOutcome;
use gaia_plan::{ExecutionPlan, ReuseState, spec_fingerprint};
use gaia_spec::ResolvedBuildSpec;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

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
            !line.trim().is_empty() && !line.starts_with("op=") && !line.starts_with("out=")
        })
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let operation_fingerprints = lines
        .clone()
        .filter_map(|line| line.strip_prefix("op="))
        .filter_map(|line| line.split_once(';'))
        .filter_map(|(operation_id, fingerprint)| {
            fingerprint
                .parse::<u64>()
                .ok()
                .map(|fingerprint| (operation_id.to_string(), fingerprint))
        })
        .collect::<BTreeMap<_, _>>();
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
    })
}

pub fn save_reuse_state(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    outcome: &ExecutionOutcome,
) {
    let path = reuse_state_path(spec);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut completed_operation_ids = outcome
        .completed_ids
        .iter()
        .map(|id| id.as_str().to_string())
        .collect::<BTreeSet<_>>();
    completed_operation_ids.extend(outcome.reused_ids.iter().map(|id| id.as_str().to_string()));
    let mut body = format!("fingerprint={}\n", spec_fingerprint(spec));
    for operation_id in &completed_operation_ids {
        body.push_str(operation_id);
        body.push('\n');
    }
    for operation in &plan.operations {
        if completed_operation_ids.contains(operation.id.as_str()) {
            body.push_str(&format!(
                "op={};{}\n",
                operation.id.as_str(),
                gaia_plan::operation_fingerprint(spec, &operation.kind)
            ));
            if let Some(signature) = gaia_plan::operation_output_signature(spec, &operation.kind) {
                body.push_str(&format!(
                    "out={};{}\n",
                    operation.id.as_str(),
                    encode_signature(&signature)
                ));
            }
        }
    }
    let _ = fs::write(path, body);
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
    for pair in hex.as_bytes().chunks_exact(2) {
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
}
