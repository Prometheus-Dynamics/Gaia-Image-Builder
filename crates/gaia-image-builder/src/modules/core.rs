use crate::config::ConfigDoc;
use crate::error::Result;
use crate::modules::Module;
use crate::planner::{Plan, Task};

pub struct CoreModule;

impl Module for CoreModule {
    fn id(&self) -> &'static str {
        "core"
    }

    fn detect(&self, _doc: &ConfigDoc) -> bool {
        true
    }

    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()> {
        for forbidden in [
            "frontend",
            "cross",
            "plugins",
            "install",
            "env",
            "services",
            "system",
            "target",
            "builder",
            "hooks",
            "image",
            "packages",
            "record",
            "sdk",
            "diagnostics",
            "sensors",
            "version",
        ] {
            if doc.has_table_path(forbidden) {
                return Err(crate::Error::msg(format!(
                    "config table '{}' is not supported in this schema; use build (metadata), buildroot, buildroot.<target>, program.*, and stage",
                    forbidden
                )));
            }
        }

        plan.add(Task {
            id: "core.init".into(),
            label: "Init".into(),
            module: self.id().into(),
            phase: "init".into(),
            after: vec![],
            provides: vec!["core:initialized".into()],
        })?;
        Ok(())
    }
}
