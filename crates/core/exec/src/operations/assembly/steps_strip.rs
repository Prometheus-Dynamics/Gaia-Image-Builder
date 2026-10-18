//! The tree strip of an assembly run: a tree declared with `strip = true` has
//! its ELF files stripped right before a filesystem or archive packs it, so
//! every step that fills the tree has run (the step order reads its root).

use super::steps::StepRun;
use super::steps_jobs::JobCtx;
use super::strip::StripSummary;
use super::*;

/// Strips the ELF files of each tree in `trees` whose `strip` is set. Returns
/// each such tree's id and summary, in declaration order.
pub(super) fn strip_trees_job(
    ctx: &JobCtx<'_>,
    trees: &[&str],
) -> Result<Vec<(String, StripSummary)>, AssemblyError> {
    let mut done = Vec::new();
    for tree in ctx
        .assembly
        .trees
        .iter()
        .filter(|tree| tree.strip && trees.contains(&tree.id.as_str()))
    {
        let tree_path = ctx.roots.tree_path(&tree.id)?;
        let files = tree_files(tree_path)?;
        let mut stripper = Stripper::new(
            ctx.spec,
            ctx.roots,
            ctx.assembly.strip_tool.as_ref(),
            ctx.cancel.clone(),
        );
        let summary = stripper.strip_paths(&files)?;
        done.push((tree.id.as_str().to_string(), summary));
    }
    Ok(done)
}

/// Records what [`strip_trees_job`] stripped: the totals, and for each tree
/// that had files stripped, its state and a message.
pub(super) fn record_strip_trees(run: &mut StepRun<'_>, trees: Vec<(String, StripSummary)>) {
    for (tree, summary) in trees {
        run.strip.add(summary);
        if summary.files == 0 {
            continue;
        }
        run.strip.trees += 1;
        let count = run.strip.trees;
        run.state
            .insert(format!("strip.{count}.tree"), tree.as_str());
        run.state
            .insert(format!("strip.{count}.files"), summary.files);
        run.state
            .insert(format!("strip.{count}.saved_bytes"), summary.saved_bytes);
        run.messages.push(format!(
            "stripped {} file(s) in tree '{}', saved {}",
            summary.files,
            tree,
            format_bytes(summary.saved_bytes)
        ));
    }
}
