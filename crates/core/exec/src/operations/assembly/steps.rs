//! The bookkeeping of one assembly run: the state file, the messages, the
//! produced outputs and the published paths. A step's work runs in
//! `steps_jobs*.rs`, possibly concurrently with other steps (see
//! `steps_schedule.rs`); the work hands back a [`Record`], and the run applies
//! the records in step order, so the state and messages of a concurrent run
//! are the ones a serial run writes.

use super::produced::ProducedRecorder;
use super::strip::StripTotals;
use super::verity::VerityOutputs;
use super::*;
use gaia_spec::ImageAssemblySpec;

/// One step's bookkeeping, applied to the run once the steps before it are
/// recorded.
pub(super) type Record<'a> = Box<dyn FnOnce(&mut StepRun<'a>) + Send + 'a>;

/// A copied dynamically linked file, checked against its tree's libraries
/// once the tree is complete.
pub(super) struct PendingLibCheck {
    pub(super) tree: String,
    pub(super) tree_path: PathBuf,
    /// The copied file, under `tree_path`.
    pub(super) file: PathBuf,
}

/// What the steps of one assembly run produced, recorded into its state.
pub(super) struct StepRun<'a> {
    pub(super) spec: &'a ResolvedBuildSpec,
    pub(super) assembly: &'a ImageAssemblySpec,
    pub(super) roots: &'a AssemblyRoots,
    pub(super) operation_id: &'a OperationId,
    pub(super) cancel_check: Option<gaia_process::ProcessCancelCheck>,
    /// The strip totals of the run.
    pub(super) strip: StripTotals,
    pub(super) state: KeyValueState,
    pub(super) messages: Vec<String>,
    pub(super) dir_count: usize,
    pub(super) symlink_count: usize,
    pub(super) staged_count: usize,
    pub(super) skipped_count: usize,
    pub(super) busybox_count: usize,
    pub(super) binary_count: usize,
    pub(super) transform_count: usize,
    pub(super) filesystem_count: usize,
    pub(super) disk_count: usize,
    pub(super) archive_count: usize,
    pub(super) kernel_modules_count: usize,
    /// The verity transforms' outputs, for archive entries that name them.
    pub(super) verity_outputs: VerityOutputs,
    /// Where each published filesystem image is published, in build order.
    pub(super) filesystem_published: Vec<PathBuf>,
    /// Where each built disk's bytes are (RAM in a RAM run).
    pub(super) disk_outputs: Vec<PathBuf>,
    /// Where each built disk is published, in the same order.
    pub(super) disk_published: Vec<PathBuf>,
    /// The id of each built disk, in the same order as `disk_outputs`.
    pub(super) disk_ids: Vec<String>,
    /// Per disk: the on-disk path it is copied to when built in RAM.
    pub(super) disk_publish: Vec<PathBuf>,
    /// The transform, verity and archive outputs written under the image
    /// output dir (`$assembly.out`): final outputs of the run.
    pub(super) published_outputs: Vec<PathBuf>,
    /// The assembly outputs this run wrote, with their sizes as written.
    pub(super) produced: ProducedRecorder<'a>,
    /// Copied dynamically linked files whose runtime libraries are checked
    /// once their tree is complete, before a filesystem or archive packs it.
    pub(super) pending_lib_checks: Vec<PendingLibCheck>,
    /// The runtime-library warnings this run raised, for the run summary.
    pub(super) warnings: Vec<String>,
}

impl<'a> StepRun<'a> {
    pub(super) fn new(
        spec: &'a ResolvedBuildSpec,
        assembly: &'a ImageAssemblySpec,
        roots: &'a AssemblyRoots,
        operation_id: &'a OperationId,
        cancel_check: Option<gaia_process::ProcessCancelCheck>,
        state: KeyValueState,
        messages: Vec<String>,
    ) -> Self {
        Self {
            spec,
            assembly,
            roots,
            operation_id,
            cancel_check,
            strip: StripTotals::default(),
            state,
            messages,
            dir_count: 0,
            symlink_count: 0,
            staged_count: 0,
            skipped_count: 0,
            busybox_count: 0,
            binary_count: 0,
            transform_count: 0,
            filesystem_count: 0,
            disk_count: 0,
            archive_count: 0,
            kernel_modules_count: 0,
            verity_outputs: VerityOutputs::new(),
            filesystem_published: Vec::new(),
            disk_outputs: Vec::new(),
            disk_published: Vec::new(),
            disk_ids: Vec::new(),
            disk_publish: Vec::new(),
            published_outputs: Vec::new(),
            produced: ProducedRecorder::new(spec, assembly, roots),
            pending_lib_checks: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Records a runtime-library warning: the state, the messages and the
    /// warnings of the run.
    pub(super) fn record_lib_warning(&mut self, message: String) {
        let key = self.warnings.len() + 1;
        self.state
            .insert(format!("runtime_lib_warning.{key}"), &message);
        self.messages.push(format!("warning: {message}"));
        self.warnings.push(message);
    }

    /// Takes the pending checks of `trees`, the trees a filesystem or archive
    /// is about to pack, in the order they were recorded.
    pub(super) fn take_lib_checks(&mut self, trees: &[&str]) -> Vec<PendingLibCheck> {
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_lib_checks)
            .into_iter()
            .partition(|check| trees.contains(&check.tree.as_str()));
        self.pending_lib_checks = waiting;
        ready
    }

    /// Records the totals once every step ran.
    pub(super) fn finish(&mut self) {
        // Trees no filesystem or archive packed are complete now.
        for check in std::mem::take(&mut self.pending_lib_checks) {
            if let Some(message) = super::steps_jobs_fs::runtime_lib_message(&check) {
                self.record_lib_warning(message);
            }
        }
        let state = &mut self.state;
        state.insert("created_dir_count", self.dir_count);
        if self.dir_count > 0 {
            self.messages
                .push(format!("created {} assembly dir(s)", self.dir_count));
        }
        state.insert("created_symlink_count", self.symlink_count);
        if self.symlink_count > 0 {
            self.messages.push(format!(
                "created {} assembly symlink(s)",
                self.symlink_count
            ));
        }
        state.insert("staged_file_count", self.staged_count);
        state.insert("skipped_file_count", self.skipped_count);
        self.messages.push(format!(
            "staged {} assembly file(s), skipped {}",
            self.staged_count, self.skipped_count
        ));
        state.insert("completed_busybox_initramfs_count", self.busybox_count);
        state.insert("completed_binary_count", self.binary_count);
        state.insert("completed_kernel_modules_count", self.kernel_modules_count);
        state.insert("completed_transform_count", self.transform_count);
        state.insert("completed_filesystem_count", self.filesystem_count);
        state.insert("completed_disk_count", self.disk_count);
        if self.strip.files > 0 {
            state.insert("stripped_file_count", self.strip.files);
            state.insert("stripped_saved_bytes", self.strip.saved_bytes);
        }
        if !self.assembly.archives.is_empty() {
            state.insert("completed_archive_count", self.archive_count);
        }
    }
}
