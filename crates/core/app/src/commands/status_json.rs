//! `gaia status --json`: the registered runs, or one run, as the fields of
//! the command's JSON document. A run's `status` is its live status snapshot
//! as written to `.gaia-run.status.json` (or the final one of an ended run).

use serde_json::{Value, json};

use super::live_status::{LastRun, LiveRun};
use super::run_registry::ListedRun;

/// `{"runs": [...]}`, numbered as `gaia status` numbers them.
pub(crate) fn listing_json(runs: &[ListedRun]) -> Value {
    json!({
        "runs": runs
            .iter()
            .enumerate()
            .map(|(index, run)| listed_run_json(run, Some(index + 1)))
            .collect::<Vec<_>>(),
    })
}

/// `{"run": {...}}` for one registered run.
pub(crate) fn one_run_json(run: &ListedRun) -> Value {
    json!({ "run": listed_run_json(run, None) })
}

/// `{"run": {...}}` for a build no registered run uses, read from its build
/// dir.
pub(crate) fn build_dir_json(
    display_name: &str,
    build_dir: &std::path::Path,
    live: Option<&LiveRun>,
    last: Option<&LastRun>,
) -> Value {
    let mut run = run_state_json(live, last);
    run["display_name"] = json!(display_name);
    run["build_dir"] = json!(build_dir.display().to_string());
    json!({ "run": run })
}

fn listed_run_json(listed: &ListedRun, number: Option<usize>) -> Value {
    let run = &listed.run;
    let mut value = run_state_json(listed.live.as_ref(), listed.ended.as_ref());
    let fields = json!({
        "number": number,
        "pid": run.pid,
        "build_id": run.build_id,
        "build_name": run.build_name,
        "display_name": run.display_name,
        "build_config": run.build_config.display().to_string(),
        "build_dir": run.build_dir.display().to_string(),
        "cwd": run.cwd.display().to_string(),
        "argv": run.argv,
        "started_at": run.started_at,
    });
    if let (Some(value), Value::Object(fields)) = (value.as_object_mut(), fields) {
        value.extend(fields);
    }
    value
}

/// `state` (`running`, `paused`, `ended` or `none`), the outcome and end of
/// an ended run, its errors, and the status snapshot.
fn run_state_json(live: Option<&LiveRun>, last: Option<&LastRun>) -> Value {
    if let Some(live) = live {
        return json!({
            "state": if live.paused() { "paused" } else { "running" },
            "outcome": null,
            "ended_at": null,
            "errors": live.status.as_ref().map_or(Value::Array(Vec::new()), |status| json!(status.errors)),
            "status": live.status,
        });
    }
    match last {
        Some(last) => json!({
            "state": "ended",
            "outcome": last.outcome,
            "ended_at": last.ended_at,
            "errors": last.errors,
            "status": last.status,
        }),
        None => json!({
            "state": "none",
            "outcome": null,
            "ended_at": null,
            "errors": [],
            "status": null,
        }),
    }
}
