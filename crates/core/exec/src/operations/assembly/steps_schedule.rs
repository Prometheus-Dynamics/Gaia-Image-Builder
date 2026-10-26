//! Running the assembly steps concurrently. A step runs once the steps it
//! must follow have been recorded, on a pool of threads; the records are
//! applied in step order, so the state file and the messages are those of a
//! serial run.
//!
//! A step must follow an earlier one when they can interfere:
//! - the writers of a tree run one after another in step order, and a
//!   filesystem, archive or OCI image that packs a tree is one of them (it
//!   strips the tree too);
//! - a step whose paths overlap an earlier step's (a transform reading an
//!   image another step writes, a disk built from filesystems, a file
//!   copied out of a tree another step writes) follows it: any step that is
//!   not a plain tree writer, and any step that reads a path inside a tree,
//!   is compared with every earlier step;
//! - the verity transforms come before the file and archive steps, which
//!   may name their outputs in a token.
//!
//! Steps with no such link may run at once. A failing step stops the steps
//! after it from starting; the steps before it finish, since one of them may
//! fail first, and the error reported is the first failing step's in step
//! order.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use gaia_spec::{AssemblyStep, AssemblyStepPaths, ImageAssemblySpec};

use super::steps::{Record, StepRun};
use super::steps_jobs::{JobCtx, JobInput, run_job};
use super::*;

/// A step that takes at least this long gets a timing message.
const STEP_TIME_MESSAGE_AFTER: Duration = Duration::from_millis(100);

/// One step handed to a worker.
struct Job<'a> {
    index: usize,
    step: AssemblyStep,
    ctx: JobCtx<'a>,
    input: JobInput,
}

/// A worker's answer for one step.
struct Finished<'a> {
    index: usize,
    elapsed: Duration,
    result: Result<Record<'a>, AssemblyError>,
}

/// Runs `order` (the steps, in their serial order) against `paths` (what each
/// reads and writes), applying every step's record to `run` in that order.
pub(super) fn run_steps<'a>(
    run: &mut StepRun<'a>,
    order: &[AssemblyStep],
    paths: &[AssemblyStepPaths],
) -> Result<(), AssemblyError> {
    let n = order.len();
    if n == 0 {
        return Ok(());
    }
    let preds = step_predecessors(run.assembly, run.roots, order, paths);
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut unmet: Vec<usize> = preds.iter().map(Vec::len).collect();
    for (step, list) in preds.iter().enumerate() {
        for &pred in list {
            successors[pred].push(step);
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = (0..n)
        .filter(|&step| unmet[step] == 0)
        .map(Reverse)
        .collect();

    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(n)
        .max(1);
    // The lowest index of a failed step (`usize::MAX` while none failed).
    let failed_at = Arc::new(AtomicUsize::new(usize::MAX));
    let mut failure: Option<AssemblyError> = None;
    let mut finished: Vec<Option<(Record<'a>, Duration)>> = (0..n).map(|_| None).collect();
    let mut recorded = 0usize;
    let mut in_flight = 0usize;
    let base = JobCtx {
        spec: run.spec,
        assembly: run.assembly,
        roots: run.roots,
        operation_id: run.operation_id,
        cancel: None,
    };
    let user_cancel = run.cancel_check.clone();

    let (job_tx, job_rx) = mpsc::channel::<Job<'a>>();
    let job_rx = Mutex::new(job_rx);
    let (done_tx, done_rx) = mpsc::channel::<Finished<'a>>();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let job_rx = &job_rx;
            let done_tx = done_tx.clone();
            scope.spawn(move || {
                loop {
                    let job = match job_rx.lock() {
                        Ok(guard) => guard.recv(),
                        Err(_) => break,
                    };
                    let Ok(job) = job else {
                        break;
                    };
                    let started = Instant::now();
                    let result = run_job(&job.ctx, job.step, job.input);
                    let finished = Finished {
                        index: job.index,
                        elapsed: started.elapsed(),
                        result,
                    };
                    if done_tx.send(finished).is_err() {
                        break;
                    }
                }
            });
        }
        drop(done_tx);

        loop {
            // Start ready steps while a worker is free, lowest index first.
            while in_flight < workers {
                let Some(Reverse(index)) = ready.peek().copied() else {
                    break;
                };
                if index >= failed_at.load(Ordering::SeqCst) {
                    break;
                }
                ready.pop();
                let step = order[index];
                let job = Job {
                    index,
                    step,
                    ctx: JobCtx {
                        cancel: Some(job_cancel(index, &failed_at, user_cancel.clone())),
                        ..base.clone()
                    },
                    input: job_input(&mut *run, step),
                };
                if job_tx.send(job).is_err() {
                    failure.get_or_insert_with(|| {
                        AssemblyError::runtime("the assembly workers stopped unexpectedly")
                    });
                    break;
                }
                in_flight += 1;
            }
            if in_flight == 0 {
                break;
            }
            let Ok(done) = done_rx.recv() else {
                failure.get_or_insert_with(|| {
                    AssemblyError::runtime("an assembly worker stopped unexpectedly")
                });
                break;
            };
            in_flight -= 1;
            match done.result {
                Ok(record) => finished[done.index] = Some((record, done.elapsed)),
                Err(error) => {
                    if done.index < failed_at.load(Ordering::SeqCst) {
                        failed_at.store(done.index, Ordering::SeqCst);
                        failure = Some(error);
                    }
                }
            }
            // Apply the records before the first failure, in step order.
            while recorded < n && recorded < failed_at.load(Ordering::SeqCst) {
                let Some((record, elapsed)) = finished[recorded].take() else {
                    break;
                };
                record(&mut *run);
                if elapsed >= STEP_TIME_MESSAGE_AFTER {
                    run.messages.push(gaia_process::step_time_message(
                        &format!("assembly {}", order[recorded].describe(run.assembly)),
                        elapsed,
                    ));
                }
                for &next in &successors[recorded] {
                    unmet[next] -= 1;
                    if unmet[next] == 0 {
                        ready.push(Reverse(next));
                    }
                }
                recorded += 1;
            }
        }
        drop(job_tx);
    });
    if let Some(error) = failure {
        return Err(error);
    }
    if recorded != n {
        return Err(AssemblyError::runtime("the assembly steps did not all run"));
    }
    Ok(())
}

/// The cancel check of step `index`: the run's cancel, or a step before the
/// first failure having failed (the steps after it are not wanted).
fn job_cancel(
    index: usize,
    failed_at: &Arc<AtomicUsize>,
    user_cancel: Option<gaia_process::ProcessCancelCheck>,
) -> gaia_process::ProcessCancelCheck {
    let failed_at = Arc::clone(failed_at);
    Arc::new(move || {
        failed_at.load(Ordering::SeqCst) < index
            || user_cancel.as_ref().is_some_and(|cancel| cancel())
    })
}

/// What a step's job takes from the run, copied before it starts. Checks are
/// taken here because they belong to the trees the step packs, which every
/// earlier writer of them has recorded by now.
fn job_input(run: &mut StepRun<'_>, step: AssemblyStep) -> JobInput {
    match step {
        AssemblyStep::File(_) => JobInput {
            verity: run.verity_outputs.clone(),
            ..JobInput::default()
        },
        AssemblyStep::Filesystem(index) => {
            let tree = run.assembly.filesystems[index]
                .source_tree
                .as_str()
                .to_string();
            JobInput {
                lib_checks: run.take_lib_checks(&[tree.as_str()]),
                ..JobInput::default()
            }
        }
        AssemblyStep::Archive(index) => {
            let trees: Vec<String> = run.assembly.archives[index]
                .members
                .iter()
                .filter_map(|member| member.tree.as_ref().map(ToString::to_string))
                .collect();
            let trees: Vec<&str> = trees.iter().map(String::as_str).collect();
            JobInput {
                verity: run.verity_outputs.clone(),
                lib_checks: run.take_lib_checks(&trees),
                ..JobInput::default()
            }
        }
        AssemblyStep::Disk(index) => JobInput {
            disk_publish: run.disk_publish.get(index).cloned(),
            ..JobInput::default()
        },
        AssemblyStep::Oci(index) => {
            let trees = oci_tree_layers(&run.assembly.ocis[index]);
            let trees: Vec<&str> = trees.iter().map(String::as_str).collect();
            JobInput {
                lib_checks: run.take_lib_checks(&trees),
                ..JobInput::default()
            }
        }
        _ => JobInput::default(),
    }
}

/// Steps that only fill trees: their work touches nothing but their tree.
fn is_tree_writer(step: AssemblyStep) -> bool {
    matches!(
        step,
        AssemblyStep::Dir(_)
            | AssemblyStep::Symlink(_)
            | AssemblyStep::File(_)
            | AssemblyStep::BusyboxInitramfs(_)
            | AssemblyStep::Binary(_)
            | AssemblyStep::KernelModules(_)
    )
}

/// The trees a step writes (a filesystem or archive strips the trees it packs).
fn tree_writes(assembly: &ImageAssemblySpec, step: AssemblyStep) -> Vec<String> {
    match step {
        AssemblyStep::Dir(index) => vec![assembly.dirs[index].tree.as_str().to_string()],
        AssemblyStep::Symlink(index) => vec![assembly.symlinks[index].tree.as_str().to_string()],
        AssemblyStep::File(index) => vec![assembly.files[index].tree.as_str().to_string()],
        AssemblyStep::BusyboxInitramfs(index) => {
            vec![assembly.busybox_initramfs[index].tree.as_str().to_string()]
        }
        AssemblyStep::Binary(index) => vec![assembly.binaries[index].tree.as_str().to_string()],
        AssemblyStep::KernelModules(index) => {
            vec![assembly.kernel_modules[index].tree.as_str().to_string()]
        }
        AssemblyStep::Filesystem(index) => {
            vec![assembly.filesystems[index].source_tree.as_str().to_string()]
        }
        AssemblyStep::Archive(index) => assembly.archives[index]
            .members
            .iter()
            .filter_map(|member| member.tree.as_ref().map(|tree| tree.as_str().to_string()))
            .collect(),
        AssemblyStep::Oci(index) => oci_tree_layers(&assembly.ocis[index]),
        AssemblyStep::Transform(_) | AssemblyStep::Disk(_) | AssemblyStep::Package(_) => Vec::new(),
    }
}

/// The trees an OCI image packs as layers (each is stripped, and checked for
/// runtime libraries, before it is tarred).
fn oci_tree_layers(oci: &gaia_spec::AssemblyOciSpec) -> Vec<String> {
    oci.layers
        .iter()
        .filter_map(|layer| layer.tree.as_ref().map(|tree| tree.as_str().to_string()))
        .collect()
}

fn is_verity(assembly: &ImageAssemblySpec, step: AssemblyStep) -> bool {
    matches!(step, AssemblyStep::Transform(index)
        if assembly.transforms[index].kind == gaia_spec::AssemblyTransformKindSpec::Verity)
}

/// Whether two steps' paths can interfere: one writes a path the other reads
/// or writes, the paths being equal or one inside the other, or a read glob
/// matching the write. The same test as the step ordering: with a plain
/// `starts_with`, a glob over `tree/overlays/*.dtbo` did not follow the step
/// copying `tree/overlays/x.dtbo` and could run before it.
fn paths_conflict(earlier: &AssemblyStepPaths, later: &AssemblyStepPaths) -> bool {
    let overlap = |left: &Path, right: &Path| {
        gaia_spec::assembly_paths_overlap(left, right)
            || gaia_spec::assembly_paths_overlap(right, left)
    };
    earlier.writes.iter().any(|written| {
        later
            .writes
            .iter()
            .chain(&later.reads)
            .any(|other| overlap(written, other))
    }) || later
        .writes
        .iter()
        .any(|written| earlier.reads.iter().any(|other| overlap(written, other)))
}

/// For each step (by its position in `order`), the earlier positions that
/// must be recorded before it may start.
fn step_predecessors(
    assembly: &ImageAssemblySpec,
    roots: &AssemblyRoots,
    order: &[AssemblyStep],
    paths: &[AssemblyStepPaths],
) -> Vec<Vec<usize>> {
    let n = order.len();
    let by_step: BTreeMap<AssemblyStep, &AssemblyStepPaths> =
        paths.iter().map(|entry| (entry.step, entry)).collect();
    let step_paths: Vec<Option<&AssemblyStepPaths>> = order
        .iter()
        .map(|step| by_step.get(step).copied())
        .collect();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];

    // The writers of a tree, in step order: each follows the one before it.
    let mut last_writer: HashMap<String, usize> = HashMap::new();
    for (index, step) in order.iter().enumerate() {
        for tree in tree_writes(assembly, *step) {
            if let Some(previous) = last_writer.insert(tree, index) {
                preds[index].push(previous);
            }
        }
    }

    // Steps that are not plain tree writers, and steps reading a path inside
    // a tree, are compared with every other step by their paths.
    let tree_roots: Vec<PathBuf> = assembly
        .trees
        .iter()
        .filter_map(|tree| roots.tree_path(&tree.id).ok().map(Path::to_path_buf))
        .collect();
    let reads_a_tree = |paths: Option<&AssemblyStepPaths>| {
        paths.is_some_and(|paths| {
            paths.reads.iter().any(|read| {
                tree_roots
                    .iter()
                    .any(|root| read.starts_with(root.as_path()))
            })
        })
    };
    for later in 0..n {
        let candidate = !is_tree_writer(order[later]) || reads_a_tree(step_paths[later]);
        if !candidate {
            continue;
        }
        for other in 0..n {
            if other == later {
                continue;
            }
            let (earlier, after) = if other < later {
                (other, later)
            } else {
                (later, other)
            };
            if let (Some(first), Some(second)) = (step_paths[earlier], step_paths[after])
                && paths_conflict(first, second)
            {
                preds[after].push(earlier);
            }
        }
    }

    // The verity trees come before the steps that may name their outputs.
    let verity: Vec<usize> = order
        .iter()
        .enumerate()
        .filter(|(_, step)| is_verity(assembly, **step))
        .map(|(index, _)| index)
        .collect();
    for (index, step) in order.iter().enumerate() {
        if matches!(step, AssemblyStep::File(_) | AssemblyStep::Archive(_)) {
            preds[index].extend(verity.iter().copied().filter(|&before| before < index));
        }
    }

    for list in &mut preds {
        list.sort_unstable();
        list.dedup();
    }
    preds
}
