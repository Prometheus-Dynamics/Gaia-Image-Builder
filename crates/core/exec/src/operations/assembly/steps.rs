//! Running the assembly steps (dirs, symlinks, files, busybox initramfs,
//! kernel modules, transforms, filesystems, disks, archives) in dependency order: a step
//! that reads another step's output runs after it
//! ([`gaia_spec::order_assembly_steps`]).

use super::*;
use gaia_spec::{AssemblyStep, ImageAssemblySpec};

/// What the steps of one assembly run produced, recorded into its state.
pub(super) struct StepRun<'a> {
    spec: &'a ResolvedBuildSpec,
    assembly: &'a ImageAssemblySpec,
    roots: &'a AssemblyRoots,
    operation_id: &'a OperationId,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
    pub(super) state: KeyValueState,
    pub(super) messages: Vec<String>,
    dir_count: usize,
    symlink_count: usize,
    staged_count: usize,
    skipped_count: usize,
    busybox_count: usize,
    transform_count: usize,
    filesystem_count: usize,
    disk_count: usize,
    archive_count: usize,
    kernel_modules_count: usize,
    /// Where each published filesystem image is published, in build order.
    pub(super) filesystem_published: Vec<PathBuf>,
    /// Where each built disk's bytes are (RAM in a RAM run).
    pub(super) disk_outputs: Vec<PathBuf>,
    /// Where each built disk is published, in the same order.
    pub(super) disk_published: Vec<PathBuf>,
    /// Per disk: the on-disk path it is copied to when built in RAM.
    pub(super) disk_publish: Vec<PathBuf>,
}

/// The steps in the order they must run, with every step's resolved paths.
pub(super) fn ordered_assembly_steps(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    roots: &AssemblyRoots,
) -> Result<(Vec<AssemblyStep>, Vec<gaia_spec::AssemblyStepPaths>), AssemblyError> {
    let paths = gaia_spec::assembly_step_paths(
        assembly,
        &|template| roots.resolve_path(spec, template.as_str()).ok(),
        &|tree| roots.tree_path(tree).ok().map(Path::to_path_buf),
    );
    let order = gaia_spec::order_assembly_steps(&paths)
        .map_err(|cycle| AssemblyError::runtime(cycle.describe(assembly)))?;
    Ok((order, paths))
}

/// Removes what the steps that create their outputs from scratch produced
/// in an earlier run, so a step reading one of those paths before it is
/// produced again fails instead of using a stale file.
pub(super) fn remove_stale_step_outputs(
    paths: &[gaia_spec::AssemblyStepPaths],
) -> Result<(), AssemblyError> {
    for step in paths.iter().filter(|step| step.step.replaces_outputs()) {
        for output in &step.writes {
            match std_fs::symlink_metadata(output) {
                Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                    std_fs::remove_file(output).map_err(|error| {
                        format!(
                            "failed to remove stale assembly output '{}': {error}",
                            output.display()
                        )
                    })?;
                }
                _ => {}
            }
        }
    }
    Ok(())
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
            state,
            messages,
            dir_count: 0,
            symlink_count: 0,
            staged_count: 0,
            skipped_count: 0,
            busybox_count: 0,
            transform_count: 0,
            filesystem_count: 0,
            disk_count: 0,
            archive_count: 0,
            kernel_modules_count: 0,
            filesystem_published: Vec::new(),
            disk_outputs: Vec::new(),
            disk_published: Vec::new(),
            disk_publish: Vec::new(),
        }
    }

    pub(super) fn run(&mut self, step: AssemblyStep) -> Result<(), AssemblyError> {
        let started = std::time::Instant::now();
        self.run_step(step)?;
        // Only steps that take a noticeable time are worth a line.
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_millis(100) {
            self.messages.push(gaia_process::step_time_message(
                &format!("assembly {}", step.describe(self.assembly)),
                elapsed,
            ));
        }
        Ok(())
    }

    fn run_step(&mut self, step: AssemblyStep) -> Result<(), AssemblyError> {
        match step {
            AssemblyStep::Dir(index) => self.dir(index),
            AssemblyStep::Symlink(index) => self.symlink(index),
            AssemblyStep::File(index) => self.file(index),
            AssemblyStep::BusyboxInitramfs(index) => self.busybox(index),
            AssemblyStep::KernelModules(index) => self.kernel_modules(index),
            AssemblyStep::Transform(index) => self.transform(index),
            AssemblyStep::Filesystem(index) => self.filesystem(index),
            AssemblyStep::Disk(index) => self.disk(index),
            AssemblyStep::Archive(index) => self.archive(index),
        }
    }

    /// Records the totals once every step ran.
    pub(super) fn finish(&mut self) {
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
        state.insert("completed_kernel_modules_count", self.kernel_modules_count);
        state.insert("completed_transform_count", self.transform_count);
        state.insert("completed_filesystem_count", self.filesystem_count);
        state.insert("completed_disk_count", self.disk_count);
        if !self.assembly.archives.is_empty() {
            state.insert("completed_archive_count", self.archive_count);
        }
    }

    fn dir(&mut self, index: usize) -> Result<(), AssemblyError> {
        let dir = &self.assembly.dirs[index];
        let tree_path = self.roots.tree_path(&dir.tree)?;
        let dest = create_assembly_dir(tree_path, dir)?;
        self.dir_count += 1;
        let count = self.dir_count;
        self.state
            .insert(format!("dir.{count}.tree"), dir.tree.as_str());
        self.state
            .insert(format!("dir.{count}.path"), dest.display().to_string());
        if let Some(mode) = &dir.mode {
            self.state.insert(format!("dir.{count}.mode"), mode);
        }
        Ok(())
    }

    fn symlink(&mut self, index: usize) -> Result<(), AssemblyError> {
        let symlink = &self.assembly.symlinks[index];
        let tree_path = self.roots.tree_path(&symlink.tree)?;
        let dest = create_assembly_symlink(tree_path, symlink)?;
        self.symlink_count += 1;
        let count = self.symlink_count;
        self.state
            .insert(format!("symlink.{count}.tree"), symlink.tree.as_str());
        self.state
            .insert(format!("symlink.{count}.path"), dest.display().to_string());
        self.state
            .insert(format!("symlink.{count}.target"), &symlink.target);
        Ok(())
    }

    fn file(&mut self, entry_index: usize) -> Result<(), AssemblyError> {
        let file = &self.assembly.files[entry_index];
        let span = tracing::info_span!(
            "assembly_file_stage",
            operation_id = %self.operation_id.as_str(),
            entry_index,
            tree_id = %file.tree,
            dest = %file.dest,
            output_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let tree_path = self.roots.tree_path(&file.tree)?;
        let sources = assembly_file_sources(self.spec, self.roots, file)?;
        if sources.is_empty() && file.optional {
            self.skipped_count += 1;
            self.state
                .insert(format!("file.{entry_index}.skipped"), "true");
            self.state
                .insert(format!("file.{entry_index}.tree"), &file.tree);
            return Ok(());
        }
        if sources.is_empty() {
            return Err(format!(
                "assembly file entry for tree '{}' matched no sources",
                file.tree
            )
            .into());
        }
        if sources.len() > 1 && !assembly_dest_is_directory(&file.dest) {
            return Err(format!(
                "assembly file entry for tree '{}' matches {} files but its dest '{}' names one file: end it with '/' to copy them into that directory",
                file.tree,
                sources.len(),
                file.dest
            )
            .into());
        }
        for source in sources {
            if !source.exists() {
                if file.optional {
                    self.skipped_count += 1;
                    self.state
                        .insert(format!("file.{entry_index}.skipped"), source.display());
                    continue;
                }
                return Err(
                    format!("assembly source '{}' does not exist", source.display()).into(),
                );
            }
            let dest = assembly_file_dest(tree_path, &source, &file.dest)?;
            tracing::Span::current().record("output_path", dest.display().to_string());
            copy_assembly_file(&source, &dest, file)?;
            self.staged_count += 1;
            let count = self.staged_count;
            self.state
                .insert(format!("file.{count}.src"), source.display().to_string());
            self.state
                .insert(format!("file.{count}.dest"), dest.display().to_string());
            self.state
                .insert(format!("file.{count}.bytes"), file_len(&dest)?);
            self.state
                .insert(format!("file.{count}.sha256"), file_sha256(&dest)?);
            if let Some(mode) = &file.mode {
                self.state.insert(format!("file.{count}.mode"), mode);
            }
        }
        Ok(())
    }

    fn busybox(&mut self, index: usize) -> Result<(), AssemblyError> {
        let initramfs = &self.assembly.busybox_initramfs[index];
        let span = tracing::info_span!(
            "assembly_busybox_initramfs",
            operation_id = %self.operation_id.as_str(),
            tree_id = %initramfs.tree,
            busybox = %initramfs.busybox,
            output_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let summary = execute_busybox_initramfs(self.spec, self.roots, initramfs)?;
        tracing::Span::current().record("output_path", summary.dest.display().to_string());
        self.busybox_count += 1;
        let prefix = format!("busybox.{}", self.busybox_count);
        let state = &mut self.state;
        state.insert(format!("{prefix}.tree"), initramfs.tree.as_str());
        state.insert(format!("{prefix}.src"), summary.src.display().to_string());
        state.insert(format!("{prefix}.dest"), summary.dest.display().to_string());
        state.insert(format!("{prefix}.bytes"), summary.bytes);
        state.insert(format!("{prefix}.sha256"), summary.sha256);
        state.insert(format!("{prefix}.applet_count"), summary.applets.len());
        for (applet_index, applet) in summary.applets.iter().enumerate() {
            state.insert(format!("{prefix}.applet.{}", applet_index + 1), applet);
        }
        state.insert(
            format!("{prefix}.runtime_linkage"),
            summary.runtime_linkage.as_str(),
        );
        state.insert(
            format!("{prefix}.runtime_library_count"),
            summary.runtime_libraries.len(),
        );
        for (library_index, library) in summary.runtime_libraries.iter().enumerate() {
            state.insert(
                format!("{prefix}.runtime_library.{}", library_index + 1),
                library.display().to_string(),
            );
        }
        self.messages.push(format!(
            "prepared busybox initramfs tree '{}' with {} applet(s)",
            initramfs.tree,
            summary.applets.len()
        ));
        Ok(())
    }

    fn kernel_modules(&mut self, index: usize) -> Result<(), AssemblyError> {
        let modules = &self.assembly.kernel_modules[index];
        let span = tracing::info_span!(
            "assembly_kernel_modules",
            operation_id = %self.operation_id.as_str(),
            tree_id = %modules.tree
        );
        let _span_guard = span.enter();
        let summary =
            execute_kernel_modules(self.spec, self.roots, modules, self.cancel_check.clone())?;
        self.kernel_modules_count += 1;
        let count = self.kernel_modules_count;
        let state = &mut self.state;
        state.insert(
            format!("kernel_modules.{count}.tree"),
            modules.tree.as_str(),
        );
        state.insert(
            format!("kernel_modules.{count}.kernel_version"),
            &summary.kernel_version,
        );
        state.insert(
            format!("kernel_modules.{count}.copied_count"),
            summary.copied.len(),
        );
        state.insert(
            format!("kernel_modules.{count}.builtin_count"),
            summary.builtin.len(),
        );
        state.insert(format!("kernel_modules.{count}.depmod"), &summary.depmod);
        if let Some(version) = &summary.depmod_version {
            state.insert(format!("kernel_modules.{count}.depmod_version"), version);
        }
        for (module_index, module) in summary.copied.iter().enumerate() {
            state.insert(
                format!("kernel_modules.{count}.module.{}", module_index + 1),
                module,
            );
        }
        self.messages.push(format!(
            "installed {} kernel module file(s) for tree '{}' (kernel {}) and ran depmod",
            summary.copied.len(),
            modules.tree,
            summary.kernel_version
        ));
        for name in &summary.builtin {
            self.messages.push(format!(
                "kernel module '{name}' is built into the kernel; nothing copied"
            ));
        }
        Ok(())
    }

    fn transform(&mut self, index: usize) -> Result<(), AssemblyError> {
        let transform = &self.assembly.transforms[index];
        let span = tracing::info_span!(
            "assembly_transform",
            operation_id = %self.operation_id.as_str(),
            kind = transform.kind.as_str(),
            dest = %transform.dest,
            output_path = tracing::field::Empty,
            tool_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let clock = gaia_process::ActiveClock::start();
        let summary = execute_assembly_transform(
            self.spec,
            self.roots,
            transform,
            self.cancel_check.clone(),
        )?;
        if matches!(
            transform.kind,
            gaia_spec::AssemblyTransformKindSpec::Gzip | gaia_spec::AssemblyTransformKindSpec::Zstd
        ) {
            let input_bytes = std_fs::metadata(&summary.src)
                .ok()
                .map(|metadata| metadata.len());
            self.messages.extend(archive_log_messages(
                &summary.dest,
                transform.kind.as_str(),
                input_bytes,
                clock.elapsed(),
            ));
        }
        tracing::Span::current().record("output_path", summary.dest.display().to_string());
        self.transform_count += 1;
        let key = AssemblyStateKey::new("transform", self.transform_count);
        let state = &mut self.state;
        state.insert(key.field("kind"), transform.kind.as_str());
        state.insert(key.field("src"), summary.src.display().to_string());
        state.insert(key.field("dest"), summary.dest.display().to_string());
        state.insert(key.field("deterministic"), transform.deterministic);
        state.insert(key.field("bytes"), summary.bytes);
        state.insert(key.field("sha256"), summary.sha256);
        if let Some(tool) = summary.tool_path {
            state.insert(key.field("tool"), tool);
        }
        if let Some(tool_version) = summary.tool_version {
            state.insert(key.field("tool_version"), tool_version);
        }
        self.messages.push(format!(
            "ran assembly transform '{}' to '{}'",
            transform.kind.as_str(),
            summary.dest.display()
        ));
        Ok(())
    }

    fn filesystem(&mut self, index: usize) -> Result<(), AssemblyError> {
        let filesystem = &self.assembly.filesystems[index];
        let span = tracing::info_span!(
            "assembly_filesystem",
            operation_id = %self.operation_id.as_str(),
            filesystem_id = %filesystem.id,
            kind = filesystem.kind.as_str(),
            source_tree = %filesystem.source_tree,
            output = %filesystem.output,
            output_path = tracing::field::Empty,
            tool_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let summary = execute_assembly_filesystem(
            self.spec,
            self.roots,
            filesystem,
            self.cancel_check.clone(),
        )?;
        // A published copy goes to the image output dir under the output's
        // file name, as a raw disk does; the bytes are the same.
        let published = match filesystem.publish_template() {
            Some(template) => {
                let target = self.roots.resolve_path(self.spec, template.as_str())?;
                if target != summary.output {
                    publish_copy_to_disk(&summary.output, &target)?;
                }
                self.filesystem_published.push(target.clone());
                Some(target)
            }
            None => None,
        };
        tracing::Span::current().record("output_path", summary.output.display().to_string());
        self.filesystem_count += 1;
        let key = AssemblyStateKey::new("filesystem", self.filesystem_count);
        let state = &mut self.state;
        if let Some(target) = &published {
            state.insert(key.field("published"), target.display().to_string());
        }
        state.insert(key.field("id"), filesystem.id.as_str());
        state.insert(key.field("kind"), filesystem.kind.as_str());
        state.insert(key.field("source_tree"), filesystem.source_tree.as_str());
        state.insert(key.field("output"), summary.output.display().to_string());
        state.insert(key.field("deterministic"), filesystem.deterministic);
        state.insert(key.field("bytes"), summary.bytes);
        state.insert(key.field("sha256"), summary.sha256);
        if let Some(tool) = summary.tool_path {
            state.insert(key.field("tool"), tool);
        }
        if let Some(tool_version) = summary.tool_version {
            state.insert(key.field("tool_version"), tool_version);
        }
        self.messages.push(format!(
            "built assembly filesystem '{}' at '{}'",
            filesystem.id,
            summary.output.display()
        ));
        if let Some(target) = published {
            self.messages.push(format!(
                "published assembly filesystem '{}' as '{}'",
                filesystem.id,
                target.display()
            ));
        }
        Ok(())
    }

    fn disk(&mut self, index: usize) -> Result<(), AssemblyError> {
        let disk = &self.assembly.disks[index];
        let span = tracing::info_span!(
            "assembly_disk",
            operation_id = %self.operation_id.as_str(),
            disk_id = %disk.id,
            partition_table = disk.partition_table.as_str(),
            output = %disk.output,
            output_path = tracing::field::Empty
        );
        let _span_guard = span.enter();
        let mut summary = execute_assembly_disk(self.spec, self.roots, disk)?;
        let source = summary.output.clone();
        // A raw disk built in RAM is copied to its published place now; the
        // copy is the same bytes, so the summary's digest stands.
        if let Some(target) = self.disk_publish.get(index).cloned() {
            publish_copy_to_disk(&source, &target)?;
            summary.output = target;
        }
        tracing::Span::current().record("output_path", summary.output.display().to_string());
        self.disk_count += 1;
        self.disk_outputs.push(source);
        self.disk_published.push(summary.output.clone());
        record_disk_state(&mut self.state, self.disk_count, disk, &summary);
        self.messages.push(format!(
            "built assembly disk '{}' at '{}'",
            disk.id,
            summary.output.display()
        ));
        Ok(())
    }

    fn archive(&mut self, index: usize) -> Result<(), AssemblyError> {
        let archive = &self.assembly.archives[index];
        let span = tracing::info_span!(
            "assembly_archive",
            operation_id = %self.operation_id.as_str(),
            archive_id = %archive.id
        );
        let _span_guard = span.enter();
        let clock = gaia_process::ActiveClock::start();
        let summary = execute_assembly_archive(self.spec, self.roots, archive)?;
        let member_bytes = summary.members.iter().map(|member| member.bytes).sum();
        self.messages.extend(archive_log_messages(
            &summary.output,
            "tar",
            Some(member_bytes),
            clock.elapsed(),
        ));
        self.archive_count += 1;
        record_archive_state(&mut self.state, self.archive_count, archive, &summary);
        self.messages.push(format!(
            "built assembly archive '{}' at '{}'",
            archive.id,
            summary.output.display()
        ));
        Ok(())
    }
}
