use crate::checkpoints::{self, CheckpointTarget, CheckpointsConfig};
use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::executor::{ExecCtx, ModuleExec, TaskRegistry};
use crate::modules::buildroot::BuildrootConfig;
use crate::planner::{Plan, Task};

const RESTORE_TASK_ID: &str = "checkpoints.restore.buildroot-build";
const CAPTURE_TASK_ID: &str = "checkpoints.capture.buildroot-build";
const RESTORE_TOKEN: &str = "checkpoints:buildroot-build-restored";

pub struct CheckpointsModule;

impl crate::modules::Module for CheckpointsModule {
    fn id(&self) -> &'static str {
        "checkpoints"
    }

    fn detect(&self, doc: &ConfigDoc) -> bool {
        doc.has_table_path("checkpoints")
    }

    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()> {
        let cfg: CheckpointsConfig = doc.deserialize_path("checkpoints")?.unwrap_or_default();
        if !cfg.enabled {
            return Ok(());
        }

        let has_buildroot_build_anchor = cfg
            .points
            .iter()
            .any(|p| p.anchor_task.trim() == "buildroot.build");

        if has_buildroot_build_anchor {
            plan.add(Task {
                id: RESTORE_TASK_ID.into(),
                label: "Restore checkpoint (buildroot.build)".into(),
                module: self.id().into(),
                phase: "restore".into(),
                after: vec![
                    "core.init".into(),
                    "buildroot.fetch".into(),
                    "buildroot:target-prepared?".into(),
                ],
                provides: vec![RESTORE_TOKEN.into()],
            })?;

            plan.add(Task {
                id: CAPTURE_TASK_ID.into(),
                label: "Capture checkpoint (buildroot.build)".into(),
                module: self.id().into(),
                phase: "capture".into(),
                after: vec!["buildroot.build".into()],
                provides: vec![],
            })?;
        }

        Ok(())
    }
}

impl ModuleExec for CheckpointsModule {
    fn register_tasks(reg: &mut TaskRegistry) -> Result<()> {
        reg.add(RESTORE_TASK_ID, exec_restore)?;
        reg.add(CAPTURE_TASK_ID, exec_capture)?;
        Ok(())
    }
}

fn buildroot_checkpoint_targets(doc: &ConfigDoc, ctx: &ExecCtx) -> Result<Vec<CheckpointTarget>> {
    let out_dir = crate::modules::buildroot::checkpoint_buildroot_out_dir(doc, ctx)?;
    Ok(vec![CheckpointTarget::new("buildroot_out_dir", out_dir)])
}

fn exec_restore(doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
    ctx.set_task(RESTORE_TASK_ID);
    let br: BuildrootConfig = doc.deserialize_path("buildroot")?.unwrap_or_default();
    if br.starting_point.enabled {
        ctx.log(
            "checkpoint restore skipped for buildroot.build (buildroot.starting_point.enabled=true)",
        );
        return Ok(());
    }
    let targets = buildroot_checkpoint_targets(doc, ctx)?;
    checkpoints::maybe_restore_anchor(doc, ctx, "buildroot.build", &targets)?;
    Ok(())
}

fn exec_capture(doc: &ConfigDoc, ctx: &mut ExecCtx) -> Result<()> {
    ctx.set_task(CAPTURE_TASK_ID);
    let br: BuildrootConfig = doc.deserialize_path("buildroot")?.unwrap_or_default();
    if br.starting_point.enabled {
        ctx.log(
            "checkpoint capture skipped for buildroot.build (buildroot.starting_point.enabled=true)",
        );
        return Ok(());
    }

    // Only capture if this run actually built the anchor.
    if checkpoints::anchor_restored(doc, ctx, "buildroot.build")? {
        ctx.log("checkpoint capture skipped (restored checkpoint was used)");
        return Ok(());
    }

    let targets = buildroot_checkpoint_targets(doc, ctx)?;
    if targets.is_empty() {
        return Err(Error::msg(
            "no checkpoint targets resolved for buildroot.build capture",
        ));
    }

    checkpoints::capture_anchor(doc, ctx, "buildroot.build", &targets)
}
