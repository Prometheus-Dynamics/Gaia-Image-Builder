use crate::config::ConfigDoc;
use crate::error::Result;
use crate::planner::Plan;

pub mod buildroot;
pub mod buildroot_rpi;
pub mod checkpoints;
pub mod core;
pub mod program;
pub mod stage;
pub mod util;

pub trait Module {
    fn id(&self) -> &'static str;
    fn detect(&self, doc: &ConfigDoc) -> bool;
    fn plan(&self, doc: &ConfigDoc, plan: &mut Plan) -> Result<()>;
}

pub fn builtin_modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(core::CoreModule),
        Box::new(program::lint::ProgramLintModule),
        Box::new(program::rust::ProgramRustModule),
        Box::new(program::java::ProgramJavaModule),
        Box::new(program::custom::ProgramCustomModule),
        Box::new(program::install::ProgramInstallModule),
        Box::new(stage::StageModule),
        Box::new(buildroot_rpi::BuildrootRpiModule),
        Box::new(checkpoints::CheckpointsModule),
        Box::new(buildroot::BuildrootModule),
    ]
}
