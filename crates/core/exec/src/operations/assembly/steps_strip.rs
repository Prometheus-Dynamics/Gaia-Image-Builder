//! The tree strip of an assembly run: a tree declared with `strip = true` has
//! its ELF files stripped right before a filesystem or archive packs it, so
//! every step that fills the tree has run (the step order reads its root).

use super::steps::StepRun;
use super::*;

impl StepRun<'_> {
    /// Strips the ELF files of each tree in `trees` whose `strip` is set.
    pub(super) fn strip_trees_of(&mut self, trees: &[&str]) -> Result<(), AssemblyError> {
        for tree in self
            .assembly
            .trees
            .iter()
            .filter(|tree| tree.strip && trees.contains(&tree.id.as_str()))
        {
            let tree_path = self.roots.tree_path(&tree.id)?;
            let files = tree_files(tree_path)?;
            let summary = self.stripper.strip_paths(&files)?;
            self.strip.add(summary);
            if summary.files == 0 {
                continue;
            }
            self.strip.trees += 1;
            let count = self.strip.trees;
            self.state
                .insert(format!("strip.{count}.tree"), tree.id.as_str());
            self.state
                .insert(format!("strip.{count}.files"), summary.files);
            self.state
                .insert(format!("strip.{count}.saved_bytes"), summary.saved_bytes);
            self.messages.push(format!(
                "stripped {} file(s) in tree '{}', saved {}",
                summary.files,
                tree.id,
                format_bytes(summary.saved_bytes)
            ));
        }
        Ok(())
    }
}
