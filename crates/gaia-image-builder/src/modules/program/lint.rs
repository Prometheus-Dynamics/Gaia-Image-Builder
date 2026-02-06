use serde::Deserialize;

use crate::config::ConfigDoc;
use crate::error::Result;
use crate::executor::{ExecCtx, ModuleExec, TaskRegistry};
use crate::modules::program::{load_program_cfg, run_checks};
use crate::planner::{Plan, Task};

const TASK_ID: &str = "program.lint.run";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProgramLintConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub check_ids: Vec<String>,
    pub cwd: Option<String>,
}

impl Default for ProgramLintConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_ids: Vec::new(),
            cwd: None,
        }
    }
}

pub struct ProgramLintModule;

impl crate::modules::Module for ProgramLintModule {
    fn id(&self) -> &'static str {
        "program.lint"
    }

    fn detect(&self, doc: &ConfigDoc) -> bool {
        doc.has_table_path(self.id())
    }

    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()> {
        let cfg: ProgramLintConfig = doc.deserialize_path(self.id())?.unwrap_or_default();
        if !cfg.enabled {
            return Ok(());
        }
        plan.add(Task {
            id: TASK_ID.into(),
            label: "Run program linters/checks".into(),
            module: self.id().into(),
            phase: "lint".into(),
            after: vec!["core.init".into()],
            provides: vec!["program:linted".into()],
        })
    }
}

impl ModuleExec for ProgramLintModule {
    fn register_tasks(reg: &mut TaskRegistry) -> Result<()> {
        reg.add(TASK_ID, exec)
    }
}

fn exec(doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
    ctx.set_task(TASK_ID);
    let cfg: ProgramLintConfig = doc.deserialize_path("program.lint")?.unwrap_or_default();
    if !cfg.enabled {
        return Ok(());
    }

    let ws = ctx.workspace_paths_or_init(doc)?;
    let program_cfg = load_program_cfg(doc)?;

    let selected = if cfg.check_ids.is_empty() {
        program_cfg
            .checks
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>()
    } else {
        cfg.check_ids.clone()
    };

    let cwd = cfg
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| ws.resolve_config_path(p))
        .transpose()?
        .unwrap_or_else(|| ws.root.clone());

    run_checks(doc, ctx, "lint", &cwd, &selected)
}
