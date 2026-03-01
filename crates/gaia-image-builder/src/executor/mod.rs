use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::log_sanitize::sanitize_log_line;
use crate::planner::Plan;
use crate::workspace::{CleanMode, WorkspaceConfig, WorkspacePaths};

pub type TaskExecFn = fn(&ConfigDoc, &mut ExecCtx) -> Result<()>;

#[derive(Default)]
struct SharedExecState {
    // Stored in shared state so core.init can set it and parallel task threads can read it.
    workspace_paths: Mutex<Option<WorkspacePaths>>,
    // Running child process group ids (on unix these are process groups; on non-unix, it's best-effort).
    // Used to implement "force quit" semantics from the UI.
    child_pgroups: Mutex<BTreeMap<u32, String>>,
}

#[derive(Debug, Clone)]
pub enum ExecEvent {
    TaskSpawned {
        id: String,
    },
    TaskStarted {
        id: String,
    },
    TaskLog {
        id: String,
        line: String,
    },
    TaskFinished {
        id: String,
        ok: bool,
        error: Option<String>,
        elapsed_ms: u128,
    },
    ExecutorDone {
        ok: bool,
        error: Option<String>,
    },
}

pub trait ExecSink: Send + Sync {
    fn emit(&self, ev: ExecEvent);
}

#[derive(Default)]
pub struct StdoutSink {
    state: Mutex<StdoutSinkState>,
}

#[derive(Default)]
struct StdoutSinkState {
    started_at: Option<Instant>,
    tasks_spawned: usize,
    tasks_started: usize,
    tasks_ok: usize,
    tasks_failed: usize,
    log_lines: usize,
    total_task_ms: u128,
    failed_tasks: Vec<String>,
    task_logs: BTreeMap<String, VecDeque<String>>,
    error_logs_dir: Option<PathBuf>,
    error_log_paths: Vec<PathBuf>,
    error_logged_tasks: BTreeSet<String>,
}

impl ExecSink for StdoutSink {
    fn emit(&self, ev: ExecEvent) {
        let mut summary_print = None::<(bool, Option<String>, String)>;
        let mut written_error_log = None::<(String, PathBuf)>;
        match ev {
            ExecEvent::TaskSpawned { id } => {
                if let Ok(mut s) = self.state.lock() {
                    s.tasks_spawned = s.tasks_spawned.saturating_add(1);
                }
                println!("SPAWN: {id}");
            }
            ExecEvent::TaskStarted { id } => {
                if let Ok(mut s) = self.state.lock() {
                    s.tasks_started = s.tasks_started.saturating_add(1);
                    if s.started_at.is_none() {
                        s.started_at = Some(Instant::now());
                    }
                }
                println!("RUN: {id}");
            }
            ExecEvent::TaskLog { id, line } => {
                if let Ok(mut s) = self.state.lock() {
                    s.log_lines = s.log_lines.saturating_add(1);
                    append_task_log_line(&mut s.task_logs, &id, &line);
                }
                println!("[{id}] {line}");
            }
            ExecEvent::TaskFinished {
                id,
                ok,
                error,
                elapsed_ms,
            } => {
                let mut err_text = String::new();
                if let Ok(mut s) = self.state.lock() {
                    if ok {
                        s.tasks_ok = s.tasks_ok.saturating_add(1);
                        s.task_logs.remove(&id);
                    } else {
                        s.tasks_failed = s.tasks_failed.saturating_add(1);
                        s.failed_tasks.push(id.clone());
                        err_text = error.clone().unwrap_or_default();
                        match write_stdout_task_error_log(
                            &mut s,
                            &id,
                            if err_text.is_empty() {
                                None
                            } else {
                                Some(err_text.as_str())
                            },
                            elapsed_ms,
                        ) {
                            Ok(path) => {
                                written_error_log = Some((id.clone(), path));
                            }
                            Err(e) => {
                                println!("WARN: failed to write task error log for {id}: {e}");
                            }
                        }
                    }
                    s.total_task_ms = s.total_task_ms.saturating_add(elapsed_ms);
                }
                if ok {
                    println!("DONE: {id} ({elapsed_ms}ms)");
                } else {
                    if err_text.is_empty() {
                        err_text = error.unwrap_or_default();
                    }
                    println!("FAIL: {id} ({elapsed_ms}ms) {err_text}");
                }
                if let Some((task_id, path)) = written_error_log.take() {
                    println!("ERROR_LOG: {task_id} => {}", path.display());
                }
            }
            ExecEvent::ExecutorDone { ok, error } => {
                if let Ok(mut s) = self.state.lock() {
                    let wall = s.started_at.map(|t| t.elapsed()).unwrap_or_default();
                    let mut summary = String::new();
                    summary.push_str("SUMMARY:\n");
                    summary.push_str(&format!("  status: {}\n", if ok { "ok" } else { "failed" }));
                    summary.push_str(&format!(
                        "  tasks: spawned={} started={} ok={} failed={}\n",
                        s.tasks_spawned, s.tasks_started, s.tasks_ok, s.tasks_failed
                    ));
                    summary.push_str(&format!("  logs: {}\n", s.log_lines));
                    summary.push_str(&format!(
                        "  elapsed: {}\n",
                        format_elapsed_hms(wall.as_secs())
                    ));
                    summary.push_str(&format!(
                        "  summed_task_time: {}\n",
                        format_elapsed_hms((s.total_task_ms / 1000) as u64)
                    ));
                    if !s.failed_tasks.is_empty() {
                        let mut failed = s.failed_tasks.clone();
                        failed.sort();
                        failed.dedup();
                        summary.push_str(&format!("  failed_tasks: {}\n", failed.join(", ")));
                    }
                    if !s.error_log_paths.is_empty() {
                        summary.push_str("  error_logs:\n");
                        for p in &s.error_log_paths {
                            summary.push_str(&format!("    {}\n", p.display()));
                        }
                    }
                    summary_print = Some((ok, error.clone(), summary));
                    *s = StdoutSinkState::default();
                }
                if ok {
                    println!("DONE: ok");
                } else {
                    println!("DONE: failed {}", error.unwrap_or_default());
                }
            }
        }
        if let Some((ok, error, summary)) = summary_print {
            print!("{summary}");
            if !ok {
                if let Some(e) = error {
                    println!("  error: {e}");
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ChannelSink {
    tx: mpsc::Sender<ExecEvent>,
}

impl ChannelSink {
    pub fn new(tx: mpsc::Sender<ExecEvent>) -> Self {
        Self { tx }
    }
}

impl ExecSink for ChannelSink {
    fn emit(&self, ev: ExecEvent) {
        let _ = self.tx.send(ev);
    }
}

#[derive(Clone)]
pub struct ExecCtx {
    pub dry_run: bool,
    pub cancel: Arc<AtomicBool>,
    pub sink: Arc<dyn ExecSink>,
    pub current_task_id: Option<String>,
    shared: Arc<SharedExecState>,
}

impl ExecCtx {
    pub fn new(dry_run: bool, sink: Arc<dyn ExecSink>) -> Self {
        Self {
            dry_run,
            cancel: Arc::new(AtomicBool::new(false)),
            sink,
            current_task_id: None,
            shared: Arc::new(SharedExecState::default()),
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn set_task(&mut self, id: impl Into<String>) {
        self.current_task_id = Some(id.into());
    }

    pub fn set_workspace_paths(&self, paths: WorkspacePaths) {
        if let Ok(mut g) = self.shared.workspace_paths.lock() {
            *g = Some(paths);
        }
    }

    pub fn workspace_paths(&self) -> Option<WorkspacePaths> {
        self.shared
            .workspace_paths
            .lock()
            .ok()
            .and_then(|g| g.clone())
    }

    // Make workspace paths available to tasks. Cleaning is only applied by core.init.
    pub fn workspace_paths_or_init(&self, doc: &ConfigDoc) -> Result<WorkspacePaths> {
        if let Some(p) = self.workspace_paths() {
            return Ok(p);
        }

        let mut ws: WorkspaceConfig = doc.deserialize_path("workspace")?.unwrap_or_default();
        if !ws.enabled {
            return crate::workspace::load_paths(&WorkspaceConfig::default());
        }

        ws.clean = CleanMode::None;
        let paths = crate::workspace::init_dirs(&ws)?;
        self.set_workspace_paths(paths.clone());
        Ok(paths)
    }

    fn register_child_pgroup(&self, pgid: u32) {
        if let Ok(mut g) = self.shared.child_pgroups.lock() {
            let owner = self
                .current_task_id
                .clone()
                .unwrap_or_else(|| "<none>".into());
            g.insert(pgid, owner);
        }
    }

    fn unregister_child_pgroup(&self, pgid: u32) {
        if let Ok(mut g) = self.shared.child_pgroups.lock() {
            g.remove(&pgid);
        }
    }

    pub fn kill_running_children_force(&self) {
        let pgids: Vec<u32> = self
            .shared
            .child_pgroups
            .lock()
            .ok()
            .map(|g| g.keys().copied().collect())
            .unwrap_or_default();
        for pgid in pgids {
            kill_pgroup(pgid, true);
        }
    }

    pub fn log(&self, msg: &str) {
        let id = self
            .current_task_id
            .clone()
            .unwrap_or_else(|| "<none>".into());
        self.sink.emit(ExecEvent::TaskLog {
            id,
            line: msg.to_string(),
        });
    }

    // Minimal helper for tasks that want to run subprocesses with line-buffered output.
    // Output is sanitized for terminal safety (control chars stripped).
    pub fn run_cmd(&self, mut cmd: Command) -> Result<()> {
        if self.cancelled() {
            return Err(Error::msg("cancelled"));
        }
        if self.dry_run {
            self.log(&format!("DRY-RUN: {:?}", cmd));
            return Ok(());
        }

        // On unix: put the child into its own process group so we can kill the whole subtree.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let mut child = cmd
            // Child tasks run in their own process group. If stdin remains attached to the
            // controlling TTY, any read can trigger SIGTTIN and suspend the task.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::msg(format!("spawn failed: {e}")))?;
        let pgid = child.id();
        self.register_child_pgroup(pgid);

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let (tx, rx) = mpsc::channel::<String>();
        if let Some(out) = stdout {
            let tx = tx.clone();
            std::thread::spawn(move || read_output_stream(out, tx));
        }
        if let Some(err) = stderr {
            let tx = tx.clone();
            std::thread::spawn(move || read_output_stream(err, tx));
        }
        drop(tx);

        for line in rx {
            let line = sanitize_log_line(&line);
            if line.is_empty() {
                continue;
            }
            self.log(&line);
            if self.cancelled() {
                // Stop the command; this is best-effort (and significantly improves "cancel" UX).
                kill_pgroup(pgid, false);
                kill_pgroup(pgid, true);
                break;
            }
        }

        let status = child
            .wait()
            .map_err(|e| Error::msg(format!("wait failed: {e}")))?;
        self.unregister_child_pgroup(pgid);
        if !status.success() {
            return Err(Error::msg(format!("command failed: {status}")));
        }
        Ok(())
    }
}

fn kill_pgroup(pgid: u32, force: bool) {
    #[cfg(unix)]
    {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        // Negative PID targets the whole process group.
        let _ = unsafe { libc::kill(-(pgid as i32), sig) };
    }
    #[cfg(not(unix))]
    {
        let _ = (pgid, force);
    }
}

#[derive(Default)]
pub struct TaskRegistry {
    exec: BTreeMap<&'static str, TaskExecFn>,
}

impl TaskRegistry {
    pub fn add(&mut self, id: &'static str, f: TaskExecFn) -> Result<()> {
        if self.exec.contains_key(id) {
            return Err(Error::msg(format!("duplicate task executor for '{id}'")));
        }
        self.exec.insert(id, f);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<TaskExecFn> {
        self.exec.get(id).copied()
    }
}

pub trait ModuleExec {
    fn register_tasks(reg: &mut TaskRegistry) -> Result<()>;
}

pub fn execute_plan(
    doc: &ConfigDoc,
    plan: &Plan,
    reg: &TaskRegistry,
    ctx: &mut ExecCtx,
) -> Result<()> {
    for task in plan.ordered()? {
        let Some(exec) = reg.get(&task.id) else {
            return Err(Error::msg(format!(
                "no executor registered for task '{}'",
                task.id
            )));
        };
        if ctx.dry_run {
            ctx.sink.emit(ExecEvent::TaskSpawned {
                id: task.id.clone(),
            });
            ctx.sink.emit(ExecEvent::TaskStarted {
                id: task.id.clone(),
            });
            ctx.set_task(task.id.clone());
            ctx.log(&format!(
                "DRY-RUN: {} ({}/{})",
                task.id, task.module, task.phase
            ));
            ctx.sink.emit(ExecEvent::TaskFinished {
                id: task.id.clone(),
                ok: true,
                error: None,
                elapsed_ms: 0,
            });
            continue;
        }
        ctx.sink.emit(ExecEvent::TaskSpawned {
            id: task.id.clone(),
        });
        ctx.sink.emit(ExecEvent::TaskStarted {
            id: task.id.clone(),
        });
        ctx.set_task(task.id.clone());
        let start = Instant::now();
        let res = exec(doc, ctx);
        let elapsed_ms = start.elapsed().as_millis();
        match res {
            Ok(()) => ctx.sink.emit(ExecEvent::TaskFinished {
                id: task.id.clone(),
                ok: true,
                error: None,
                elapsed_ms,
            }),
            Err(e) => {
                ctx.sink.emit(ExecEvent::TaskFinished {
                    id: task.id.clone(),
                    ok: false,
                    error: Some(e.to_string()),
                    elapsed_ms,
                });
                ctx.sink.emit(ExecEvent::ExecutorDone {
                    ok: false,
                    error: Some(format!("task '{}' failed: {e}", task.id)),
                });
                return Err(Error::msg(format!("task '{}' failed: {e}", task.id)));
            }
        }
    }
    ctx.sink.emit(ExecEvent::ExecutorDone {
        ok: true,
        error: None,
    });
    Ok(())
}

pub fn execute_plan_parallel(
    doc: &ConfigDoc,
    plan: &Plan,
    reg: &TaskRegistry,
    ctx_template: &ExecCtx,
    max_parallel: usize,
) -> Result<()> {
    if max_parallel <= 1 || ctx_template.dry_run {
        let mut ctx = ctx_template.clone();
        return execute_plan(doc, plan, reg, &mut ctx);
    }

    // Clone the doc into an Arc so we can move it into worker threads.
    // (spawn requires 'static captures.)
    let doc = Arc::new(doc.clone());

    // Build provides index: token -> task id.
    let mut provides: BTreeMap<String, String> = BTreeMap::new();
    for t in plan.tasks() {
        for p in &t.provides {
            if let Some(existing) = provides.insert(p.clone(), t.id.clone()) {
                return Err(Error::msg(format!(
                    "provide token '{}' is produced by both '{}' and '{}'",
                    p, existing, t.id
                )));
            }
        }
    }

    // Build dependency graph (deps resolved to concrete task ids).
    let mut incoming: BTreeMap<String, usize> = BTreeMap::new();
    let mut outgoing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for task in plan.tasks() {
        incoming.insert(task.id.clone(), 0);
        outgoing.entry(task.id.clone()).or_default();
    }

    for task in plan.tasks() {
        for dep in &task.after {
            let (dep, optional) = dep
                .strip_suffix('?')
                .map(|d| (d, true))
                .unwrap_or((dep.as_str(), false));

            let dep_id = if incoming.contains_key(dep) {
                dep.to_string()
            } else if let Some(provider) = provides.get(dep) {
                provider.clone()
            } else if optional {
                continue;
            } else {
                return Err(Error::msg(format!(
                    "task '{}' has invalid dependency '{}'",
                    task.id, dep
                )));
            };

            outgoing.entry(dep_id).or_default().insert(task.id.clone());
            *incoming.get_mut(&task.id).unwrap() += 1;
        }
    }

    let total = incoming.len();
    let mut completed: BTreeSet<String> = BTreeSet::new();

    // Deterministic ready queue.
    let mut ready: VecDeque<String> = incoming
        .iter()
        .filter_map(|(k, v)| (*v == 0).then_some(k.clone()))
        .collect();

    let (tx, rx) = mpsc::channel::<(String, Result<()>, u128)>();
    let mut running: HashMap<String, std::thread::JoinHandle<()>> = HashMap::new();
    let mut first_err: Option<Error> = None;

    while completed.len() < total {
        if ctx_template.cancelled() {
            break;
        }
        if first_err.is_some() && running.is_empty() {
            break;
        }
        // Fill worker slots.
        while first_err.is_none() && running.len() < max_parallel {
            if ctx_template.cancelled() {
                break;
            }
            let Some(task_id) = ready.pop_front() else {
                break;
            };
            if completed.contains(&task_id) || running.contains_key(&task_id) {
                continue;
            }

            let Some(exec) = reg.get(&task_id) else {
                return Err(Error::msg(format!(
                    "no executor registered for task '{}'",
                    task_id
                )));
            };
            let tx = tx.clone();
            let doc = Arc::clone(&doc);
            let ctx = ctx_template.clone();
            ctx.sink.emit(ExecEvent::TaskSpawned {
                id: task_id.clone(),
            });

            let task_id_for_thread = task_id.clone();
            let handle = std::thread::spawn(move || {
                let mut local_ctx = ctx;
                local_ctx.sink.emit(ExecEvent::TaskStarted {
                    id: task_id_for_thread.clone(),
                });
                local_ctx.set_task(task_id_for_thread.clone());
                let start = Instant::now();
                let r = exec(&doc, &mut local_ctx);
                let elapsed_ms = start.elapsed().as_millis();
                let _ = tx.send((task_id_for_thread, r, elapsed_ms));
            });

            running.insert(task_id, handle);
        }

        if completed.len() == total {
            break;
        }

        if running.is_empty() {
            if first_err.is_some() {
                break;
            }
            // Nothing running, nothing ready, but not complete => cycle / deadlock.
            if ready.is_empty() {
                let remaining: Vec<String> = incoming
                    .iter()
                    .filter_map(|(k, v)| (*v > 0).then_some(k.clone()))
                    .collect();
                return Err(Error::msg(format!(
                    "cannot make progress (cycle or unresolved deps); remaining: {}",
                    remaining.join(", ")
                )));
            }
            continue;
        }

        let (done_id, res, elapsed_ms) = rx
            .recv()
            .map_err(|e| Error::msg(format!("executor recv failed: {e}")))?;

        // Join the worker so panics are surfaced.
        if let Some(h) = running.remove(&done_id) {
            if let Err(panic) = h.join() {
                first_err = Some(Error::msg(format!(
                    "task '{done_id}' panicked: {:?}",
                    panic
                )));
            }
        }

        match res {
            Ok(()) => {
                ctx_template.sink.emit(ExecEvent::TaskFinished {
                    id: done_id.clone(),
                    ok: true,
                    error: None,
                    elapsed_ms,
                });
                completed.insert(done_id.clone());
                // Release dependents.
                if let Some(children) = outgoing.get(&done_id) {
                    for child in children {
                        let slot = incoming.get_mut(child).unwrap();
                        *slot -= 1;
                        if *slot == 0 {
                            ready.push_back(child.clone());
                        }
                    }
                }
            }
            Err(e) => {
                ctx_template.sink.emit(ExecEvent::TaskFinished {
                    id: done_id.clone(),
                    ok: false,
                    error: Some(e.to_string()),
                    elapsed_ms,
                });
                if first_err.is_none() {
                    first_err = Some(Error::msg(format!("task '{done_id}' failed: {e}")));
                }
            }
        }
    }

    // Drain/stop: if there was an error, wait for running tasks to finish and return it.
    while let Ok((done_id, _, _)) = rx.try_recv() {
        if let Some(h) = running.remove(&done_id) {
            let _ = h.join();
        }
    }
    for (_, h) in running.drain() {
        let _ = h.join();
    }

    if let Some(e) = first_err {
        ctx_template.sink.emit(ExecEvent::ExecutorDone {
            ok: false,
            error: Some(e.to_string()),
        });
        return Err(e);
    }
    if ctx_template.cancelled() {
        let e = Error::msg("cancelled");
        ctx_template.sink.emit(ExecEvent::ExecutorDone {
            ok: false,
            error: Some("cancelled".into()),
        });
        return Err(e);
    }
    ctx_template.sink.emit(ExecEvent::ExecutorDone {
        ok: true,
        error: None,
    });
    Ok(())
}

pub fn builtin_registry() -> Result<TaskRegistry> {
    let mut reg = TaskRegistry::default();
    // Keep this list explicit for now (compiled-in modules).
    reg.add("core.init", core_init)?;
    reg.add("core.barrier.stage", core_barrier_stage)?;
    crate::modules::program::lint::ProgramLintModule::register_tasks(&mut reg)?;
    crate::modules::program::rust::ProgramRustModule::register_tasks(&mut reg)?;
    crate::modules::program::java::ProgramJavaModule::register_tasks(&mut reg)?;
    crate::modules::program::custom::ProgramCustomModule::register_tasks(&mut reg)?;
    crate::modules::program::install::ProgramInstallModule::register_tasks(&mut reg)?;
    crate::modules::stage::StageModule::register_tasks(&mut reg)?;
    crate::modules::buildroot_rpi::BuildrootRpiModule::register_tasks(&mut reg)?;
    crate::modules::checkpoints::CheckpointsModule::register_tasks(&mut reg)?;
    crate::modules::buildroot::BuildrootModule::register_tasks(&mut reg)?;
    Ok(reg)
}

fn core_init(_doc: &ConfigDoc, _ctx: &mut ExecCtx) -> Result<()> {
    let ws: WorkspaceConfig = _doc.deserialize_path("workspace")?.unwrap_or_default();
    if !ws.enabled {
        return Ok(());
    }

    _ctx.set_task("core.init");
    _ctx.log(&format!("workspace.root_dir = {}", ws.root_dir));
    _ctx.log(&format!("workspace.build_dir = {}", ws.build_dir));
    _ctx.log(&format!("workspace.out_dir = {}", ws.out_dir));
    _ctx.log(&format!(
        "workspace.clean = {}",
        match ws.clean {
            CleanMode::None => "none",
            CleanMode::Build => "build",
            CleanMode::Out => "out",
            CleanMode::All => "all",
        }
    ));

    let paths = crate::workspace::init_dirs(&ws)?;
    _ctx.set_workspace_paths(paths.clone());
    _ctx.log(&format!("workspace.root = {}", paths.root.display()));
    _ctx.log(&format!(
        "workspace.build_dir(abs) = {}",
        paths.build_dir.display()
    ));
    _ctx.log(&format!(
        "workspace.out_dir(abs) = {}",
        paths.out_dir.display()
    ));
    for (name, path) in &paths.named_dirs {
        _ctx.log(&format!("workspace.paths.{} = {}", name, path.display()));
    }
    Ok(())
}

fn core_barrier_stage(_doc: &ConfigDoc, _ctx: &mut ExecCtx) -> Result<()> {
    // A pure ordering barrier; all work is done by dependencies.
    _ctx.set_task("core.barrier.stage");
    _ctx.log("stage barrier reached");
    Ok(())
}

fn read_output_stream<R: Read>(reader: R, tx: mpsc::Sender<String>) {
    const MAX_PENDING_BYTES: usize = 16 * 1024;
    let mut r = BufReader::new(reader);
    let mut buf = [0u8; 8192];
    let mut pending = Vec::with_capacity(1024);

    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for b in &buf[..n] {
            if *b == b'\n' || *b == b'\r' {
                if pending.is_empty() {
                    continue;
                }
                let line = String::from_utf8_lossy(&pending).into_owned();
                pending.clear();
                let _ = tx.send(line);
            } else {
                pending.push(*b);
                if pending.len() >= MAX_PENDING_BYTES {
                    let line = String::from_utf8_lossy(&pending).into_owned();
                    pending.clear();
                    let _ = tx.send(line);
                }
            }
        }
    }

    if !pending.is_empty() {
        let line = String::from_utf8_lossy(&pending).into_owned();
        let _ = tx.send(line);
    }
}

fn append_task_log_line(
    task_logs: &mut BTreeMap<String, VecDeque<String>>,
    task_id: &str,
    line: &str,
) {
    const MAX_LINES: usize = 4000;
    let sanitized = sanitize_log_line(line);

    let mut write_line = |id: &str| {
        let q = task_logs.entry(id.to_string()).or_default();
        while q.len() >= MAX_LINES {
            q.pop_front();
        }
        q.push_back(sanitized.clone());
    };

    write_line(task_id);
    if let Some((parent, _)) = task_id.split_once(':')
        && !parent.trim().is_empty()
    {
        write_line(parent);
    }
}

fn write_stdout_task_error_log(
    state: &mut StdoutSinkState,
    task_id: &str,
    error: Option<&str>,
    elapsed_ms: u128,
) -> Result<PathBuf> {
    if state.error_logged_tasks.contains(task_id) {
        if let Some(existing) = state.error_log_paths.iter().find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s == sanitize_filename_component(task_id))
                .unwrap_or(false)
        }) {
            return Ok(existing.clone());
        }
    }

    let dir = ensure_stdout_error_logs_dir(state)?;
    let path = dir.join(format!("{}.log", sanitize_filename_component(task_id)));

    let mut body = String::new();
    body.push_str(&format!("task: {task_id}\n"));
    body.push_str("status: failed\n");
    body.push_str(&format!("elapsed_ms: {elapsed_ms}\n"));
    if let Some(e) = error {
        if !e.trim().is_empty() {
            body.push_str(&format!("error: {e}\n"));
        }
    }
    body.push('\n');
    body.push_str("logs:\n");
    if let Some(lines) = state.task_logs.get(task_id) {
        for line in lines {
            body.push_str(line);
            body.push('\n');
        }
    }

    fs::write(&path, body).map_err(|e| {
        Error::msg(format!(
            "failed to write task error log {}: {e}",
            path.display()
        ))
    })?;

    state.error_logged_tasks.insert(task_id.to_string());
    state.error_log_paths.push(path.clone());
    Ok(path)
}

fn ensure_stdout_error_logs_dir(state: &mut StdoutSinkState) -> Result<PathBuf> {
    if let Some(existing) = state.error_logs_dir.as_ref() {
        return Ok(existing.clone());
    }

    let root = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("build")
        .join("error-logs");
    let dir = root.join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    fs::create_dir_all(&dir).map_err(|e| {
        Error::msg(format!(
            "failed to create error logs dir {}: {e}",
            dir.display()
        ))
    })?;
    state.error_logs_dir = Some(dir.clone());
    Ok(dir)
}

fn sanitize_filename_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "task".into() } else { out }
}

fn format_elapsed_hms(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}
