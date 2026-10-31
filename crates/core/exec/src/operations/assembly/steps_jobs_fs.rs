//! The work of the assembly steps that make outputs: transforms, verity
//! trees, filesystem images, disks and archives. Like `steps_jobs.rs`, each
//! job returns the [`Record`] of its bookkeeping.

use super::steps::{PendingLibCheck, Record, StepRun};
use super::steps_jobs::JobCtx;
use super::steps_strip::{record_strip_trees, strip_trees_job};
use super::verity::VerityOutputs;
use super::*;

/// Whether `path` is under the image output dir, where published outputs
/// go.
fn under_out_dir(ctx: &JobCtx<'_>, path: &Path) -> bool {
    path.starts_with(&ctx.roots.assembly_out)
}

pub(super) fn job_transform<'a>(
    ctx: &JobCtx<'a>,
    index: usize,
) -> Result<Record<'a>, AssemblyError> {
    let transform = &ctx.assembly.transforms[index];
    if transform.kind == gaia_spec::AssemblyTransformKindSpec::Verity {
        return job_verity(ctx, index);
    }
    let span = tracing::info_span!(
        "assembly_transform",
        operation_id = %ctx.operation_id.as_str(),
        kind = transform.kind.as_str(),
        dest = %transform.dest,
        output_path = tracing::field::Empty,
        tool_path = tracing::field::Empty
    );
    let _span_guard = span.enter();
    let clock = gaia_process::ActiveClock::start();
    let summary = execute_assembly_transform(ctx.spec, ctx.roots, transform, ctx.cancel.clone())?;
    let mut messages = Vec::new();
    if matches!(
        transform.kind,
        gaia_spec::AssemblyTransformKindSpec::Gzip
            | gaia_spec::AssemblyTransformKindSpec::Zstd
            | gaia_spec::AssemblyTransformKindSpec::Xz
    ) {
        let input_bytes = std_fs::metadata(&summary.src)
            .ok()
            .map(|metadata| metadata.len());
        messages.extend(archive_log_messages(
            &summary.dest,
            transform.kind.as_str(),
            input_bytes,
            clock.elapsed(),
        ));
    }
    tracing::Span::current().record("output_path", summary.dest.display().to_string());
    let published = under_out_dir(ctx, &summary.dest);
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        run.messages.extend(messages);
        if published {
            run.published_outputs.push(summary.dest.clone());
        }
        run.produced.record_transform(index, summary.bytes);
        run.transform_count += 1;
        let key = AssemblyStateKey::new("transform", run.transform_count);
        let state = &mut run.state;
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
        run.messages.push(format!(
            "ran assembly transform '{}' to '{}'",
            transform.kind.as_str(),
            summary.dest.display()
        ));
    }))
}

fn job_verity<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let transform = &ctx.assembly.transforms[index];
    let span = tracing::info_span!(
        "assembly_verity",
        operation_id = %ctx.operation_id.as_str(),
        dest = %transform.dest,
        output_path = tracing::field::Empty,
        tool_path = tracing::field::Empty
    );
    let _span_guard = span.enter();
    let output = execute_assembly_verity(ctx.spec, ctx.roots, transform, ctx.cancel.clone())?;
    tracing::Span::current().record("output_path", output.data.display().to_string());
    let published = under_out_dir(ctx, &output.data);
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        if published {
            run.published_outputs.push(output.data.clone());
        }
        run.produced.record_verity(index, &output);
        run.transform_count += 1;
        let key = AssemblyStateKey::new("transform", run.transform_count);
        let verity_key = AssemblyStateKey::new("verity", run.transform_count);
        let state = &mut run.state;
        state.insert(key.field("kind"), transform.kind.as_str());
        state.insert(key.field("src"), output.src.display().to_string());
        state.insert(key.field("dest"), output.data.display().to_string());
        state.insert(key.field("deterministic"), transform.deterministic);
        state.insert(key.field("bytes"), output.bytes);
        state.insert(key.field("sha256"), &output.sha256);
        state.insert(key.field("tool"), &output.tool_path);
        if let Some(tool_version) = &output.tool_version {
            state.insert(key.field("tool_version"), tool_version);
        }
        state.insert(verity_key.field("hash"), output.hash.display().to_string());
        state.insert(verity_key.field("root_hash"), &output.root_hash);
        state.insert(verity_key.field("salt"), &output.salt);
        state.insert(verity_key.field("uuid"), &output.uuid);
        state.insert(verity_key.field("hash_offset"), output.hash_offset);
        state.insert(verity_key.field("data_blocks"), output.data_blocks);
        state.insert(verity_key.field("data_block_size"), output.data_block_size);
        state.insert(verity_key.field("hash_block_size"), output.hash_block_size);
        run.messages.push(format!(
            "built assembly verity tree for '{}' (root hash {})",
            output.data.display(),
            output.root_hash
        ));
        run.verity_outputs.insert(output.data.clone(), output);
    }))
}

pub(super) fn job_filesystem<'a>(
    ctx: &JobCtx<'a>,
    index: usize,
    lib_checks: Vec<PendingLibCheck>,
) -> Result<Record<'a>, AssemblyError> {
    let filesystem = &ctx.assembly.filesystems[index];
    let span = tracing::info_span!(
        "assembly_filesystem",
        operation_id = %ctx.operation_id.as_str(),
        filesystem_id = %filesystem.id,
        kind = filesystem.kind.as_str(),
        source_tree = %filesystem.source_tree,
        output = %filesystem.output,
        output_path = tracing::field::Empty,
        tool_path = tracing::field::Empty
    );
    let _span_guard = span.enter();
    // The tree is checked and stripped before it is packed.
    let warnings = lib_check_messages(&lib_checks);
    let strips = strip_trees_job(ctx, &[filesystem.source_tree.as_str()])?;
    let summary = execute_assembly_filesystem(ctx.spec, ctx.roots, filesystem, ctx.cancel.clone())?;
    // A published copy goes to the image output dir under the output's
    // file name, as a raw disk does; the bytes are the same.
    let published = match filesystem.publish_template() {
        Some(template) => {
            let target = ctx.roots.resolve_path(ctx.spec, template.as_str())?;
            if target != summary.output {
                publish_copy_to_disk(&summary.output, &target)?;
            }
            Some(target)
        }
        None => None,
    };
    tracing::Span::current().record("output_path", summary.output.display().to_string());
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        record_lib_warnings(run, warnings);
        record_strip_trees(run, strips);
        run.produced.record_filesystem(index, summary.bytes);
        if let Some(target) = &published {
            run.filesystem_published.push(target.clone());
        }
        run.filesystem_count += 1;
        let key = AssemblyStateKey::new("filesystem", run.filesystem_count);
        let state = &mut run.state;
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
        run.messages.push(format!(
            "built assembly filesystem '{}' at '{}'",
            filesystem.id,
            summary.output.display()
        ));
        if let Some(target) = published {
            run.messages.push(format!(
                "published assembly filesystem '{}' as '{}'",
                filesystem.id,
                target.display()
            ));
        }
    }))
}

pub(super) fn job_disk<'a>(
    ctx: &JobCtx<'a>,
    index: usize,
    disk_publish: Option<PathBuf>,
) -> Result<Record<'a>, AssemblyError> {
    let disk = &ctx.assembly.disks[index];
    let span = tracing::info_span!(
        "assembly_disk",
        operation_id = %ctx.operation_id.as_str(),
        disk_id = %disk.id,
        partition_table = disk.partition_table.as_str(),
        output = %disk.output,
        output_path = tracing::field::Empty
    );
    let _span_guard = span.enter();
    let mut summary = execute_assembly_disk(ctx.spec, ctx.roots, disk)?;
    let source = summary.output.clone();
    // A raw disk built in RAM is copied to its published place now; the
    // copy is the same bytes, so the summary's digest stands.
    if let Some(target) = disk_publish {
        publish_copy_to_disk(&source, &target)?;
        summary.output = target;
    }
    tracing::Span::current().record("output_path", summary.output.display().to_string());
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        run.disk_count += 1;
        run.produced.record_disk(index, summary.bytes);
        run.disk_outputs.push(source);
        run.disk_ids.push(disk.id.clone());
        run.disk_published.push(summary.output.clone());
        record_disk_state(&mut run.state, run.disk_count, disk, &summary);
        run.messages.push(format!(
            "built assembly disk '{}' at '{}'",
            disk.id,
            summary.output.display()
        ));
    }))
}

pub(super) fn job_archive<'a>(
    ctx: &JobCtx<'a>,
    index: usize,
    lib_checks: Vec<PendingLibCheck>,
    verity: &VerityOutputs,
) -> Result<Record<'a>, AssemblyError> {
    let archive = &ctx.assembly.archives[index];
    let span = tracing::info_span!(
        "assembly_archive",
        operation_id = %ctx.operation_id.as_str(),
        archive_id = %archive.id
    );
    let _span_guard = span.enter();
    // Each member tree is complete: every step that fills it ran before
    // this one (the order reads its root).
    let warnings = lib_check_messages(&lib_checks);
    let trees: Vec<String> = archive
        .members
        .iter()
        .filter_map(|member| member.tree.as_ref().map(ToString::to_string))
        .collect();
    let trees: Vec<&str> = trees.iter().map(String::as_str).collect();
    let strips = strip_trees_job(ctx, &trees)?;
    let clock = gaia_process::ActiveClock::start();
    let summary = execute_assembly_archive(ctx.spec, ctx.roots, archive, verity)?;
    let member_bytes = summary.members.iter().map(|member| member.bytes).sum();
    let messages =
        archive_log_messages(&summary.output, "tar", Some(member_bytes), clock.elapsed());
    let published = under_out_dir(ctx, &summary.output);
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        record_lib_warnings(run, warnings);
        record_strip_trees(run, strips);
        run.messages.extend(messages);
        if published {
            run.published_outputs.push(summary.output.clone());
        }
        run.produced.record_archive(index, &summary.output);
        run.archive_count += 1;
        record_archive_state(&mut run.state, run.archive_count, archive, &summary);
        run.messages.push(format!(
            "built assembly archive '{}' at '{}'",
            archive.id,
            summary.output.display()
        ));
    }))
}

pub(super) fn job_oci<'a>(
    ctx: &JobCtx<'a>,
    index: usize,
    lib_checks: Vec<PendingLibCheck>,
) -> Result<Record<'a>, AssemblyError> {
    let oci = &ctx.assembly.ocis[index];
    let span = tracing::info_span!(
        "assembly_oci",
        operation_id = %ctx.operation_id.as_str(),
        oci_id = %oci.id
    );
    let _span_guard = span.enter();
    // Each tree layer is complete: every step that fills it ran before this
    // one (the order reads its root).
    let warnings = lib_check_messages(&lib_checks);
    let trees: Vec<String> = oci
        .layers
        .iter()
        .filter_map(|layer| layer.tree.as_ref().map(ToString::to_string))
        .collect();
    let trees: Vec<&str> = trees.iter().map(String::as_str).collect();
    let strips = strip_trees_job(ctx, &trees)?;
    let clock = gaia_process::ActiveClock::start();
    let summary = execute_assembly_oci(ctx.spec, ctx.roots, oci, ctx.cancel.clone())?;
    let elapsed = clock.elapsed();
    let reused = summary.layers.iter().filter(|layer| layer.reused).count();
    let compressed = if oci.compression == "none" {
        0
    } else {
        summary.layers.len() - reused
    };
    let message = format!(
        "built assembly oci image '{}' at '{}' ({} layer(s): {compressed} compressed, {reused} from cache) in {:.2}s",
        oci.id,
        summary.output.display(),
        summary.layers.len(),
        elapsed.as_secs_f64()
    );
    let published = under_out_dir(ctx, &summary.output);
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        record_lib_warnings(run, warnings);
        record_strip_trees(run, strips);
        if published {
            run.published_outputs.push(summary.output.clone());
        }
        run.produced.record_oci(index, summary.bytes);
        run.oci_count += 1;
        record_oci_state(&mut run.state, run.oci_count, oci, &summary);
        run.messages.push(message);
    }))
}

pub(super) fn job_ubi<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let ubi = &ctx.assembly.ubis[index];
    let span = tracing::info_span!(
        "assembly_ubi",
        operation_id = %ctx.operation_id.as_str(),
        ubi_id = %ubi.id
    );
    let _span_guard = span.enter();
    let summary = super::ubi::execute_assembly_ubi(ctx.spec, ctx.roots, ubi)?;
    let message = format!(
        "built ubi image '{}' at '{}': {} PEBs written, {} reserved; needs a flash of at least {} bytes",
        ubi.id,
        summary.output.display(),
        summary.image.pebs_written,
        summary.image.reserved_pebs(),
        summary.minimum_flash_bytes
    );
    let published = under_out_dir(ctx, &summary.output);
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        if published {
            run.published_outputs.push(summary.output.clone());
        }
        run.produced.record_ubi(index, summary.bytes);
        run.ubi_count += 1;
        super::ubi::record_ubi_state(&mut run.state, run.ubi_count, ubi, &summary);
        run.messages.push(message);
    }))
}

pub(super) fn job_package<'a>(ctx: &JobCtx<'a>, index: usize) -> Result<Record<'a>, AssemblyError> {
    let package = &ctx.assembly.packages[index];
    let span = tracing::info_span!(
        "assembly_package",
        operation_id = %ctx.operation_id.as_str(),
        package_id = %package.id
    );
    let _span_guard = span.enter();
    let summary =
        super::package::execute_assembly_package(ctx.spec, ctx.roots, package, ctx.cancel.clone())?;
    let message = format!(
        "package '{}': {} ({} bytes), {} files",
        package.id,
        summary.output.display(),
        summary.bytes,
        summary.files
    );
    let published = under_out_dir(ctx, &summary.output);
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        if published {
            run.published_outputs.push(summary.output.clone());
        }
        run.produced.record_package(index, summary.bytes);
        run.package_count += 1;
        super::package::record_package_state(&mut run.state, run.package_count, package, &summary);
        run.messages.push(message);
    }))
}

/// The warning text of a copied dynamic file whose runtime libraries its
/// tree lacks, or `None` when they are all there. A tree may overlay a full
/// rootfs, so this is a warning, not an error.
pub(super) fn runtime_lib_message(check: &PendingLibCheck) -> Option<String> {
    let guest = match check.file.strip_prefix(&check.tree_path) {
        Ok(relative) => format!("/{}", relative.display()),
        Err(_) => check.file.display().to_string(),
    };
    match gaia_plan::missing_runtime_libraries(&check.file, &check.tree_path) {
        Ok(missing) if missing.is_empty() => None,
        Ok(missing) => Some(format!(
            "assembly file '{guest}' in tree '{}' is dynamically linked, but the tree lacks {}: copy it with [[image.assembly.binaries]] to bring its libraries",
            check.tree,
            missing.join(", ")
        )),
        Err(error) => Some(format!(
            "could not check the runtime libraries of assembly file '{guest}' in tree '{}': {error}",
            check.tree
        )),
    }
}

fn lib_check_messages(checks: &[PendingLibCheck]) -> Vec<String> {
    checks.iter().filter_map(runtime_lib_message).collect()
}

fn record_lib_warnings(run: &mut StepRun<'_>, messages: Vec<String>) {
    for message in messages {
        run.record_lib_warning(message);
    }
}
