//! The work of the assembly steps that fill trees (dirs, symlinks, files,
//! busybox initramfs, binaries, kernel modules), and the dispatch of every
//! step's work. A job reads only what [`JobCtx`] and [`JobInput`] carry; it
//! returns the [`Record`] of its bookkeeping, which the run applies in step
//! order.

use super::steps::{PendingLibCheck, Record, StepRun};
use super::verity::VerityOutputs;
use super::*;
use gaia_spec::{AssemblyStep, ImageAssemblySpec};

/// What every job shares: read-only references and the cancel check the
/// job's steps consult.
#[derive(Clone)]
pub(super) struct JobCtx<'a> {
    pub(super) spec: &'a ResolvedBuildSpec,
    pub(super) assembly: &'a ImageAssemblySpec,
    pub(super) roots: &'a AssemblyRoots,
    pub(super) operation_id: &'a OperationId,
    pub(super) cancel: Option<gaia_process::ProcessCancelCheck>,
}

/// What a job takes from the run before it starts, copied on the run's
/// thread: a job never shares the run.
#[derive(Default)]
pub(super) struct JobInput {
    /// The verity outputs recorded so far (generated files and archives
    /// name them).
    pub(super) verity: VerityOutputs,
    /// Where a disk built in RAM is published (disks only).
    pub(super) disk_publish: Option<PathBuf>,
    /// The runtime-library checks of the trees a filesystem or archive packs.
    pub(super) lib_checks: Vec<PendingLibCheck>,
}

/// The work of one step, and the record of its bookkeeping.
pub(super) fn run_job<'a>(
    ctx: &JobCtx<'a>,
    step: AssemblyStep,
    input: JobInput,
) -> Result<Record<'a>, AssemblyError> {
    match step {
        AssemblyStep::Dir(index) => job_dir(ctx, index),
        AssemblyStep::Symlink(index) => job_symlink(ctx, index),
        AssemblyStep::File(index) => job_file(ctx, index, &input.verity),
        AssemblyStep::BusyboxInitramfs(index) => job_busybox(ctx, index),
        AssemblyStep::Binary(index) => job_binary(ctx, index),
        AssemblyStep::KernelModules(index) => job_kernel_modules(ctx, index),
        AssemblyStep::Transform(index) => super::steps_jobs_fs::job_transform(ctx, index),
        AssemblyStep::Filesystem(index) => {
            super::steps_jobs_fs::job_filesystem(ctx, index, input.lib_checks)
        }
        AssemblyStep::Disk(index) => super::steps_jobs_fs::job_disk(ctx, index, input.disk_publish),
        AssemblyStep::Archive(index) => {
            super::steps_jobs_fs::job_archive(ctx, index, input.lib_checks, &input.verity)
        }
        AssemblyStep::Oci(index) => super::steps_jobs_fs::job_oci(ctx, index, input.lib_checks),
        AssemblyStep::Package(index) => super::steps_jobs_fs::job_package(ctx, index),
    }
}

fn job_dir<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let assembly = ctx.assembly;
    let dir = &assembly.dirs[index];
    let tree_path = ctx.roots.tree_path(&dir.tree)?;
    let dest = create_assembly_dir(tree_path, dir)?;
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        run.dir_count += 1;
        let count = run.dir_count;
        run.state
            .insert(format!("dir.{count}.tree"), dir.tree.as_str());
        run.state
            .insert(format!("dir.{count}.path"), dest.display().to_string());
        if let Some(mode) = &dir.mode {
            run.state.insert(format!("dir.{count}.mode"), mode);
        }
    }))
}

fn job_symlink<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let assembly = ctx.assembly;
    let symlink = &assembly.symlinks[index];
    let tree_path = ctx.roots.tree_path(&symlink.tree)?;
    let dest = create_assembly_symlink(tree_path, symlink)?;
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        run.symlink_count += 1;
        let count = run.symlink_count;
        run.state
            .insert(format!("symlink.{count}.tree"), symlink.tree.as_str());
        run.state
            .insert(format!("symlink.{count}.path"), dest.display().to_string());
        run.state
            .insert(format!("symlink.{count}.target"), &symlink.target);
    }))
}

/// The copies one `file` entry makes, each as its record needs them.
enum StagedFile {
    /// An optional source that does not exist.
    Missing { source: PathBuf },
    Copied {
        source: PathBuf,
        dest: PathBuf,
        bytes: u64,
        sha256: String,
        strip: Option<StripSummary>,
        pending: Option<PendingLibCheck>,
    },
}

fn job_file<'a>(
    ctx: &JobCtx<'a>,
    entry_index: usize,
    verity: &VerityOutputs,
) -> Result<Record<'a>, AssemblyError> {
    let assembly = ctx.assembly;
    let file = &assembly.files[entry_index];
    let span = tracing::info_span!(
        "assembly_file_stage",
        operation_id = %ctx.operation_id.as_str(),
        entry_index,
        tree_id = %file.tree,
        dest = %file.dest,
        output_path = tracing::field::Empty
    );
    let _span_guard = span.enter();
    if file.src_dir.is_some() {
        return super::files_tree::job_file_tree(ctx, entry_index, file);
    }
    let tree_path = ctx.roots.tree_path(&file.tree)?;
    let mode = file.mode.clone();
    if file.content.is_some() || file.json.is_some() {
        let generated = write_generated_file(ctx.spec, ctx.roots, verity, tree_path, file)?;
        tracing::Span::current().record("output_path", generated.dest.display().to_string());
        return Ok(Box::new(move |run: &mut StepRun<'a>| {
            run.staged_count += 1;
            let count = run.staged_count;
            run.state.insert(format!("file.{count}.generated"), "true");
            run.state.insert(
                format!("file.{count}.dest"),
                generated.dest.display().to_string(),
            );
            run.state
                .insert(format!("file.{count}.bytes"), generated.bytes);
            run.state
                .insert(format!("file.{count}.sha256"), generated.sha256);
            if let Some(mode) = &mode {
                run.state.insert(format!("file.{count}.mode"), mode);
            }
        }));
    }
    let sources = assembly_file_sources(ctx.spec, ctx.roots, file)?;
    if sources.is_empty() && file.optional {
        return Ok(Box::new(move |run: &mut StepRun<'a>| {
            run.skipped_count += 1;
            // Skipped entries live under `file.skipped.<entry>`: staged files
            // use `file.<count>.*`, so the two numberings never share a key.
            run.state
                .insert(format!("file.skipped.{entry_index}.empty"), "true");
            run.state
                .insert(format!("file.skipped.{entry_index}.tree"), &file.tree);
        }));
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
    let mut staged = Vec::new();
    for source in sources {
        if !source.exists() {
            if file.optional {
                staged.push(StagedFile::Missing { source });
                continue;
            }
            return Err(format!("assembly source '{}' does not exist", source.display()).into());
        }
        let dest = assembly_file_dest(tree_path, &source, &file.dest)?;
        tracing::Span::current().record("output_path", dest.display().to_string());
        copy_assembly_file(&source, &dest, file)?;
        let strip = if file.strip {
            let mut stripper = Stripper::new(
                ctx.spec,
                ctx.roots,
                assembly.strip_tool.as_ref(),
                ctx.cancel.clone(),
            );
            Some(stripper.strip_paths(std::slice::from_ref(&dest))?)
        } else {
            None
        };
        let regular = std_fs::symlink_metadata(&dest).is_ok_and(|metadata| metadata.is_file());
        // A tree a package with device checks is made from relies on the
        // device's own libraries; the package step checks them against the
        // device sysroot instead.
        let device_checked = assembly
            .packages
            .iter()
            .any(|package| package.tree == file.tree && package.device.is_some());
        let pending = (regular
            && !file.allow_missing_libs
            && !device_checked
            && gaia_plan::is_dynamic_elf(&dest))
        .then(|| PendingLibCheck {
            tree: file.tree.as_str().to_string(),
            tree_path: tree_path.to_path_buf(),
            file: dest.clone(),
        });
        let bytes = file_len(&dest)?;
        let sha256 = file_sha256(&dest)?;
        staged.push(StagedFile::Copied {
            source,
            dest,
            bytes,
            sha256,
            strip,
            pending,
        });
    }
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        for item in staged {
            match item {
                StagedFile::Missing { source } => {
                    run.skipped_count += 1;
                    run.state.insert(
                        format!("file.skipped.{entry_index}.source"),
                        source.display(),
                    );
                }
                StagedFile::Copied {
                    source,
                    dest,
                    bytes,
                    sha256,
                    strip,
                    pending,
                } => {
                    if let Some(strip) = strip {
                        run.strip.add(strip);
                    }
                    if let Some(check) = pending {
                        run.pending_lib_checks.push(check);
                    }
                    run.staged_count += 1;
                    let count = run.staged_count;
                    run.state
                        .insert(format!("file.{count}.src"), source.display().to_string());
                    run.state
                        .insert(format!("file.{count}.dest"), dest.display().to_string());
                    run.state.insert(format!("file.{count}.bytes"), bytes);
                    run.state.insert(format!("file.{count}.sha256"), sha256);
                    if let Some(mode) = &mode {
                        run.state.insert(format!("file.{count}.mode"), mode);
                    }
                }
            }
        }
    }))
}

fn job_busybox<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let initramfs = &ctx.assembly.busybox_initramfs[index];
    let span = tracing::info_span!(
        "assembly_busybox_initramfs",
        operation_id = %ctx.operation_id.as_str(),
        tree_id = %initramfs.tree,
        busybox = %initramfs.busybox,
        output_path = tracing::field::Empty
    );
    let _span_guard = span.enter();
    let summary = execute_busybox_initramfs(ctx.spec, ctx.roots, initramfs)?;
    tracing::Span::current().record("output_path", summary.dest.display().to_string());
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        run.busybox_count += 1;
        let prefix = format!("busybox.{}", run.busybox_count);
        let state = &mut run.state;
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
        run.messages.push(format!(
            "prepared busybox initramfs tree '{}' with {} applet(s)",
            initramfs.tree,
            summary.applets.len()
        ));
    }))
}

fn job_binary<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let binary = &ctx.assembly.binaries[index];
    let span = tracing::info_span!(
        "assembly_binary",
        operation_id = %ctx.operation_id.as_str(),
        tree_id = %binary.tree,
        dest = %binary.dest,
        output_path = tracing::field::Empty
    );
    let _span_guard = span.enter();
    let mut stripper = Stripper::new(
        ctx.spec,
        ctx.roots,
        ctx.assembly.strip_tool.as_ref(),
        ctx.cancel.clone(),
    );
    let summary = execute_binary(ctx.spec, ctx.roots, binary, &mut stripper)?;
    tracing::Span::current().record("output_path", summary.dest.display().to_string());
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        run.strip.add(summary.stripped);
        run.binary_count += 1;
        let prefix = format!("binary.{}", run.binary_count);
        let state = &mut run.state;
        state.insert(format!("{prefix}.tree"), binary.tree.as_str());
        state.insert(format!("{prefix}.src"), summary.src.display().to_string());
        state.insert(format!("{prefix}.dest"), summary.dest.display().to_string());
        state.insert(format!("{prefix}.bytes"), summary.bytes);
        state.insert(format!("{prefix}.sha256"), summary.sha256);
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
        run.messages.push(format!(
            "installed binary '{}' into tree '{}' ({} runtime librar{})",
            binary.dest,
            binary.tree,
            summary.runtime_libraries.len(),
            if summary.runtime_libraries.len() == 1 {
                "y"
            } else {
                "ies"
            }
        ));
    }))
}

fn job_kernel_modules<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let modules = &ctx.assembly.kernel_modules[index];
    let span = tracing::info_span!(
        "assembly_kernel_modules",
        operation_id = %ctx.operation_id.as_str(),
        tree_id = %modules.tree
    );
    let _span_guard = span.enter();
    let summary = execute_kernel_modules(ctx.spec, ctx.roots, modules, ctx.cancel.clone())?;
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        run.kernel_modules_count += 1;
        let count = run.kernel_modules_count;
        let state = &mut run.state;
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
        run.messages.push(format!(
            "installed {} kernel module file(s) for tree '{}' (kernel {}) and ran depmod",
            summary.copied.len(),
            modules.tree,
            summary.kernel_version
        ));
        for name in &summary.builtin {
            run.messages.push(format!(
                "kernel module '{name}' is built into the kernel; nothing copied"
            ));
        }
    }))
}
