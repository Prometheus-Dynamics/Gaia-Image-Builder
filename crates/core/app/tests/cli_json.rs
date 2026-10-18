pub mod support;

use gaia_app::{
    AppArgs, AppCommand, CommandOutcome, JSON_SCHEMA_VERSION, RunFinish, outcome_json,
    run_with_args,
};
use gaia_testkit::TempDir;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use support::{config_path, seed_default_assets, seed_reuse_state};

/// The `--json` document of `args`, as `gaia` prints it.
fn json_of(args: &[String]) -> (CommandOutcome, Value) {
    let mut args = args.to_vec();
    args.push("--json".into());
    let args = AppArgs::parse_from(args);
    assert!(args.usage_errors.is_empty(), "{:?}", args.usage_errors);
    let command = args.command;
    let outcome = run_with_args(args);
    let document = outcome_json(
        command,
        &outcome,
        &RunFinish::default(),
        outcome.exit_code(),
    );
    assert_eq!(document["schema_version"], JSON_SCHEMA_VERSION);
    assert_eq!(document["exit_code"], outcome.exit_code());
    (outcome, document)
}

struct Workspace {
    _scratch: TempDir,
    root: String,
    out: String,
    build: String,
}

impl Workspace {
    fn new(name: &str) -> Self {
        let scratch = TempDir::new(name);
        let root = scratch.join("root").display().to_string();
        let out = scratch.join("out").display().to_string();
        let build = scratch.join("build").display().to_string();
        fs::create_dir_all(PathBuf::from(&root).join("src")).expect("root");
        fs::write(PathBuf::from(&root).join("tracked.txt"), "json").expect("tracked");
        fs::write(
            PathBuf::from(&root).join("Cargo.toml"),
            "[package]\nname = \"gaia\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
        )
        .expect("cargo toml");
        fs::write(PathBuf::from(&root).join("src/main.rs"), "fn main() {}\n").expect("main");
        seed_default_assets(&root);
        seed_reuse_state(&root, &build, &out);
        Self {
            _scratch: scratch,
            root,
            out,
            build,
        }
    }

    fn args(&self, command: &str) -> Vec<String> {
        [
            command,
            &config_path(),
            "--preset",
            "ci",
            "--set",
            "image.allow_fallback=true",
            "--set",
            &format!("workspace.root_dir={}", self.root),
            "--set",
            &format!("workspace.out_dir={}", self.out),
            "--set",
            &format!("workspace.build_dir={}", self.build),
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect()
    }
}

#[test]
fn validate_and_plan_json_documents() {
    let workspace = Workspace::new("gaia-json-plan");
    let (_, validate) = json_of(&workspace.args("validate"));
    assert_eq!(validate["command"], "validate");
    assert_eq!(validate["status"], "ok");
    assert_eq!(validate["error_count"], 0);
    assert!(validate["diagnostics"].is_array());
    assert!(validate["build"].is_string());

    let (_, plan) = json_of(&workspace.args("plan"));
    assert_eq!(plan["command"], "plan");
    let operations = plan["operations"].as_array().expect("operations");
    assert_eq!(plan["operation_count"], operations.len());
    let first = &operations[0];
    for field in [
        "id",
        "depends_on",
        "optionality",
        "execute",
        "reason",
        "reused_from",
    ] {
        assert!(
            first.get(field).is_some(),
            "operation lacks {field}: {first}"
        );
    }
    for field in [
        "executing_operations",
        "timed_operations",
        "total_work_ms",
        "critical_path_ms",
        "critical_path",
        "untimed_operations",
    ] {
        assert!(
            plan["estimate"].get(field).is_some(),
            "estimate lacks {field}"
        );
    }
    assert!(plan["diagnostics"].is_array());
}

#[test]
fn run_json_has_the_summary_sizes_critical_path_and_trace_file() {
    let workspace = Workspace::new("gaia-json-run");
    let (outcome, run) = json_of(&workspace.args("run"));
    assert!(matches!(outcome, CommandOutcome::Ran { .. }), "{outcome:?}");
    assert_eq!(run["command"], "run");
    assert_eq!(run["status"], "succeeded", "{run}");
    assert!(run["run_duration_ms"].is_u64());
    let summary = &run["summary"];
    assert!(summary["sizes"].is_array());
    assert!(summary["operation_timings"].is_array());
    let timing = &summary["operation_timings"][0];
    assert!(timing["start_ms"].is_u64(), "{timing}");
    let path = &summary["critical_path"];
    assert!(path["duration_ms"].is_u64(), "{path}");
    assert!(!path["operations"].as_array().expect("path").is_empty());
    assert!(
        run["execution_errors"]
            .as_array()
            .expect("errors")
            .is_empty()
    );

    let files = run["report_files"].as_array().expect("files");
    let trace = files
        .iter()
        .find(|file| file["kind"] == "trace")
        .expect("trace report file");
    let trace_path = trace["path"].as_str().expect("path");
    assert!(trace_path.ends_with(".trace.json"), "{trace_path}");
    let trace: Value =
        serde_json::from_str(&fs::read_to_string(trace_path).expect("trace")).expect("json");
    let events = trace["traceEvents"].as_array().expect("events");
    let slices = events
        .iter()
        .filter(|event| event["ph"] == "X")
        .collect::<Vec<_>>();
    assert_eq!(
        slices.len(),
        summary["operation_timings"].as_array().map_or(0, Vec::len)
            + slices
                .iter()
                .filter(|event| event["cat"] == "assembly-step")
                .count()
    );
    assert!(
        slices
            .iter()
            .all(|event| event["ts"].is_u64() && event["dur"].is_u64())
    );
}

#[test]
fn failures_status_and_cache_have_json_documents() {
    let failed = outcome_json(
        AppCommand::Validate,
        &CommandOutcome::Failed {
            message: "no such build".into(),
        },
        &RunFinish::default(),
        1,
    );
    assert_eq!(failed["status"], "error");
    assert_eq!(failed["error"], "no such build");
    assert_eq!(failed["command"], "validate");

    let workspace = Workspace::new("gaia-json-cache");
    // A build no registered run uses: its build dir is read.
    let (_, status) = json_of(&workspace.args("status"));
    assert_eq!(status["command"], "status");
    assert_eq!(status["run"]["state"], "none", "{status}");
    assert!(status["run"]["build_dir"].is_string());

    // The project level only: the system level is the real user cache.
    let mut cache = workspace.args("cache");
    cache.extend(["--level".to_string(), "project".to_string()]);
    let (_, cache) = json_of(&cache);
    assert_eq!(cache["command"], "cache");
    assert_eq!(cache["action"], "list");
    assert!(cache["entries"].is_array());
    assert!(cache["levels"].is_array());

    // `cache stats` is covered by its unit test: here it would walk the
    // real user cache.

    let rejected = AppArgs::parse_from(["lock", "build.toml", "--json"]);
    assert!(matches!(
        run_with_args(rejected),
        CommandOutcome::Failed { message } if message.contains("--json applies to")
    ));
}
