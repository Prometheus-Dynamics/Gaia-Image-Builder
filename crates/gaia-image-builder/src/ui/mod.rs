use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::{self, Stdout};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Axis, Block, BorderType, Borders, Chart, Clear, Dataset, Gauge, GraphType, List, ListItem,
    ListState, Paragraph, Tabs, Widget, Wrap,
};

use crate::config::ConfigDoc;
use crate::error::{Error, Result};
use crate::executor::{ChannelSink, ExecCtx, ExecEvent, StdoutSink};
use crate::log_sanitize::sanitize_log_line;
use crate::workspace::{CleanMode, WorkspaceConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Picker,
    Quick,
    Modules,
    Keys,
    Inputs,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunTab {
    Overview,
    Tasks,
    Logs,
    TaskLog,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickItem {
    Start,
    DryRun,
    MaxParallel,
    BuildrootPerfProfile,
    TopLevelLoad,
    Inputs,
    RefreshCheckpointStatus,
    RetryCheckpointUploads,
    CheckpointUsePolicy,
    CheckpointUploadPolicy,
    Clean,
    Modules,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStatus {
    Pending,
    Running,
    Ok,
    Failed,
}

#[derive(Debug, Clone)]
struct TaskViewState {
    status: TaskStatus,
    last_line: Option<String>,
}

impl Default for TaskViewState {
    fn default() -> Self {
        Self {
            status: TaskStatus::Pending,
            last_line: None,
        }
    }
}

#[derive(Debug, Clone)]
struct SystemSnapshot {
    cpu_cores: usize,
    load_1m: Option<f64>,
    load_pct: Option<f64>,
    mem_total_kib: Option<u64>,
    mem_used_kib: Option<u64>,
    mem_pct: Option<f64>,
    disk_total_bytes: Option<u64>,
    disk_used_bytes: Option<u64>,
    disk_pct: Option<f64>,
}

impl Default for SystemSnapshot {
    fn default() -> Self {
        Self {
            cpu_cores: num_cpus::get().max(1),
            load_1m: None,
            load_pct: None,
            mem_total_kib: None,
            mem_used_kib: None,
            mem_pct: None,
            disk_total_bytes: None,
            disk_used_bytes: None,
            disk_pct: None,
        }
    }
}

struct App {
    builds_dir: PathBuf,
    builds: Vec<PathBuf>,
    build_list: ListState,

    screen: Screen,

    selected_build: Option<PathBuf>,
    doc_base: Option<ConfigDoc>,
    doc_effective: Option<ConfigDoc>,

    // Interactive overrides applied on top of loaded config (modules, inputs, and quick edits).
    overrides: toml::Value,
    overrides_file: Option<PathBuf>,

    enabled_modules: Vec<String>,
    module_list: ListState,
    config_keys: Vec<String>,
    config_key_list: ListState,
    input_keys: Vec<String>,
    input_key_list: ListState,
    config_parts: Vec<String>,
    config_part_query: String,
    config_part_matches: Vec<usize>,
    config_part_list: ListState,

    quick_items: Vec<QuickItem>,
    quick_list: ListState,
    dry_run: bool,
    auto_exit_on_done: bool,
    exit_at: Option<Instant>,

    run_tab: RunTab,
    task_list: ListState,
    tasks: Vec<String>,
    task_state: BTreeMap<String, TaskViewState>,
    task_logs: BTreeMap<String, VecDeque<String>>,
    checkpoint_status: Vec<crate::checkpoints::CheckpointStatus>,
    checkpoint_list: ListState,
    all_logs: VecDeque<String>,
    sidebar_logs: VecDeque<String>,
    all_logs_scroll: usize,
    task_log_scroll: usize,
    config_scroll: usize,
    task_error_log_dir: Option<PathBuf>,
    task_error_logs: Vec<PathBuf>,
    task_error_logged: BTreeSet<String>,

    system_snapshot: SystemSnapshot,
    system_last_sample: Instant,
    load_history: VecDeque<f64>,
    mem_history: VecDeque<f64>,
    disk_history: VecDeque<f64>,

    exec_rx: Option<std::sync::mpsc::Receiver<ExecEvent>>,
    exec_ctx: Option<ExecCtx>,
    exec_thread_done: bool,
    exec_total: usize,
    exec_done: usize,
    exec_ok: Option<bool>,
    exec_started_at: Option<Instant>,
    exec_last_tick: Instant,

    max_parallel: usize,

    input: InputMode,
    force_exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValueKind {
    Bool,
    Int,
    Float,
    String,
}

#[derive(Debug, Clone)]
enum InputMode {
    Normal,
    EditValue {
        full_path: String,
        kind: ValueKind,
        buffer: String,
        error: Option<String>,
    },
    SelectInputChoice {
        key: String,
        options: Vec<toml::Value>,
        selected: usize,
    },
    EditGlobalInt {
        name: String,
        buffer: String,
        error: Option<String>,
    },
    EditGlobalFloat {
        name: String,
        buffer: String,
        error: Option<String>,
    },
    SearchConfigParts {
        buffer: String,
    },
    ConfirmCancelRun,
}

impl App {
    fn new(builds_dir: PathBuf, max_parallel: usize) -> Result<Self> {
        let builds = find_tomls(&builds_dir)?;
        let mut build_list = ListState::default();
        if !builds.is_empty() {
            build_list.select(Some(0));
        }
        let quick_items = vec![
            QuickItem::Start,
            QuickItem::DryRun,
            QuickItem::MaxParallel,
            QuickItem::BuildrootPerfProfile,
            QuickItem::TopLevelLoad,
            QuickItem::Inputs,
            QuickItem::RefreshCheckpointStatus,
            QuickItem::RetryCheckpointUploads,
            QuickItem::CheckpointUsePolicy,
            QuickItem::CheckpointUploadPolicy,
            QuickItem::Clean,
            QuickItem::Modules,
            QuickItem::Back,
        ];
        let mut quick_list = ListState::default();
        if !quick_items.is_empty() {
            quick_list.select(Some(0));
        }
        Ok(Self {
            builds_dir,
            builds,
            build_list,
            screen: Screen::Picker,
            selected_build: None,
            doc_base: None,
            doc_effective: None,
            overrides: empty_overrides_table(),
            overrides_file: None,
            enabled_modules: Vec::new(),
            module_list: ListState::default(),
            config_keys: Vec::new(),
            config_key_list: ListState::default(),
            input_keys: Vec::new(),
            input_key_list: ListState::default(),
            config_parts: Vec::new(),
            config_part_query: String::new(),
            config_part_matches: Vec::new(),
            config_part_list: ListState::default(),
            quick_items,
            quick_list,
            dry_run: false,
            auto_exit_on_done: true,
            exit_at: None,
            run_tab: RunTab::Overview,
            task_list: ListState::default(),
            tasks: Vec::new(),
            task_state: BTreeMap::new(),
            task_logs: BTreeMap::new(),
            checkpoint_status: Vec::new(),
            checkpoint_list: ListState::default(),
            all_logs: VecDeque::new(),
            sidebar_logs: VecDeque::new(),
            all_logs_scroll: 0,
            task_log_scroll: 0,
            config_scroll: 0,
            task_error_log_dir: None,
            task_error_logs: Vec::new(),
            task_error_logged: BTreeSet::new(),
            system_snapshot: SystemSnapshot::default(),
            system_last_sample: Instant::now(),
            load_history: VecDeque::new(),
            mem_history: VecDeque::new(),
            disk_history: VecDeque::new(),
            exec_rx: None,
            exec_ctx: None,
            exec_thread_done: false,
            exec_total: 0,
            exec_done: 0,
            exec_ok: None,
            exec_started_at: None,
            exec_last_tick: Instant::now(),
            max_parallel,
            input: InputMode::Normal,
            force_exit_code: None,
        })
    }

    fn selected_build(&self) -> Option<&PathBuf> {
        let idx = self.build_list.selected()?;
        self.builds.get(idx)
    }

    fn select_next_build(&mut self) {
        if self.builds.is_empty() {
            return;
        }
        let i = self.build_list.selected().unwrap_or(0);
        let next = (i + 1).min(self.builds.len().saturating_sub(1));
        self.build_list.select(Some(next));
    }

    fn select_prev_build(&mut self) {
        if self.builds.is_empty() {
            return;
        }
        let i = self.build_list.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.build_list.select(Some(prev));
    }

    fn load_selected_build(&mut self) -> Result<()> {
        let Some(path) = self.selected_build().cloned() else {
            return Ok(());
        };
        let doc = crate::config::load(&path)?;
        let overrides_file = build_tui_overrides_path(&path);
        let overrides = match read_tui_overrides(&overrides_file) {
            Ok(v) => v,
            Err(e) => {
                self.push_sidebar(&format!(
                    "failed to load tui overrides {}: {e}",
                    overrides_file.display()
                ));
                empty_overrides_table()
            }
        };
        self.selected_build = Some(path);
        self.doc_base = Some(doc);
        self.overrides = overrides;
        self.overrides_file = Some(overrides_file);
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        self.screen = Screen::Quick;
        Ok(())
    }

    fn recompute_effective_doc(&mut self) -> Result<()> {
        let Some(base) = self.doc_base.as_ref() else {
            return Ok(());
        };
        let mut value = base.value.clone();
        crate::config::merge(&mut value, self.overrides.clone());
        let mut doc = ConfigDoc {
            path: base.path.clone(),
            value,
        };
        crate::build_inputs::apply_cli_overrides(&mut doc, &[])?;
        self.doc_effective = Some(doc);
        Ok(())
    }

    fn recompute_modules_and_tasks(&mut self) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref() else {
            return Ok(());
        };

        // Enabled modules are computed from compiled-in modules (for now).
        {
            let mut mods = Vec::new();
            let modules = crate::modules::builtin_modules();
            for m in modules {
                if m.detect(doc) {
                    mods.push(m.id().to_string());
                }
            }
            mods.sort();
            self.enabled_modules = mods;
            if !self.enabled_modules.is_empty() {
                self.module_list.select(Some(0));
            }
        }

        // Compute plan/tasks for progress views.
        let mut plan = crate::planner::Plan::default();
        let modules = crate::modules::builtin_modules();
        for m in &modules {
            if m.detect(doc) {
                m.plan(doc, &mut plan)?;
            }
        }
        crate::checkpoints::validate_against_plan(doc, &plan)?;
        plan.finalize_default()?;
        let ordered = plan.ordered()?;
        self.tasks = ordered.iter().map(|t| t.id.clone()).collect();
        let prev_selected_id = self.selected_checkpoint().map(|s| s.id.clone());
        self.checkpoint_status = crate::checkpoints::status_for_doc(doc)?;
        if self.checkpoint_status.is_empty() {
            self.checkpoint_list.select(None);
        } else if let Some(prev) = prev_selected_id {
            let idx = self
                .checkpoint_status
                .iter()
                .position(|s| s.id == prev)
                .unwrap_or(0);
            self.checkpoint_list.select(Some(idx));
        } else {
            self.checkpoint_list.select(Some(0));
        }
        self.exec_total = self.tasks.len();
        self.exec_done = 0;
        self.exec_ok = None;
        self.task_state.clear();
        self.task_logs.clear();
        for id in &self.tasks {
            self.task_state.insert(id.clone(), TaskViewState::default());
            self.task_logs.insert(id.clone(), VecDeque::new());
        }
        if !self.tasks.is_empty() {
            self.task_list.select(Some(0));
        }

        // Keep key list consistent with current module selection.
        self.recompute_config_keys();
        self.recompute_input_keys();
        self.recompute_config_parts();
        Ok(())
    }

    fn recompute_config_keys(&mut self) {
        self.config_keys.clear();
        self.config_key_list.select(None);

        let Some(doc) = self.doc_effective.as_ref() else {
            return;
        };
        let Some(mid) = self.selected_module() else {
            return;
        };
        let Some(tbl) = doc.table_path(mid) else {
            return;
        };

        // Top-level scalar keys in the module table.
        for (k, v) in tbl {
            if k == "imports" {
                continue;
            }
            if v.is_table() {
                continue;
            }
            if v.is_array() {
                // Arrays are often large; keep them out of the quick editor for now.
                continue;
            }
            self.config_keys.push(k.clone());
        }

        // Common sub-structure: steps.<name>.<field>
        if let Some(steps) = tbl.get("steps").and_then(|v| v.as_table()) {
            for (step_name, step_val) in steps {
                let Some(step_tbl) = step_val.as_table() else {
                    continue;
                };
                for (k, v) in step_tbl {
                    if k == "imports" {
                        continue;
                    }
                    if v.is_table() || v.is_array() {
                        continue;
                    }
                    self.config_keys.push(format!("steps.{}.{}", step_name, k));
                }
            }
        }

        self.config_keys.sort();
        self.config_key_list
            .select(self.config_keys.first().map(|_| 0));
    }

    fn recompute_input_keys(&mut self) {
        let previous = self.selected_input_key().map(ToOwned::to_owned);
        self.input_keys.clear();
        self.input_key_list.select(None);

        let Some(doc) = self.doc_effective.as_ref() else {
            return;
        };

        let mut keys = BTreeSet::<String>::new();
        if let Some(tbl) = doc.table_path("inputs.options") {
            for key in tbl.keys() {
                keys.insert(key.clone());
            }
        }
        if let Some(tbl) = doc.table_path("inputs.values") {
            for key in tbl.keys() {
                keys.insert(key.clone());
            }
        }
        if let Some(tbl) = doc.table_path("inputs.resolved") {
            for key in tbl.keys() {
                keys.insert(key.clone());
            }
        }

        self.input_keys = keys.into_iter().collect();
        if self.input_keys.is_empty() {
            self.input_key_list.select(None);
        } else if let Some(prev) = previous {
            let idx = self.input_keys.iter().position(|k| k == &prev).unwrap_or(0);
            self.input_key_list.select(Some(idx));
        } else {
            self.input_key_list.select(Some(0));
        }
    }

    fn recompute_config_parts(&mut self) {
        let previous = self.selected_config_part().map(ToOwned::to_owned);

        self.config_parts.clear();
        self.config_parts.push("all".into());

        let Some(doc) = self.doc_effective.as_ref() else {
            self.refresh_config_part_matches();
            self.select_config_part_from_name(previous.as_deref());
            return;
        };

        if let Some(root) = doc.value.as_table() {
            let mut entries = root
                .iter()
                .filter_map(|(k, v)| v.as_table().map(|t| (k.clone(), t)))
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, t) in entries {
                self.config_parts.push(k.clone());
                collect_table_paths(&k, t, &mut self.config_parts);
            }
        }

        self.refresh_config_part_matches();
        self.select_config_part_from_name(previous.as_deref());
    }

    fn refresh_config_part_matches(&mut self) {
        let query = self.config_part_query.trim().to_ascii_lowercase();
        if query.is_empty() {
            self.config_part_matches = (0..self.config_parts.len()).collect();
        } else {
            self.config_part_matches = self
                .config_parts
                .iter()
                .enumerate()
                .filter_map(|(idx, part)| part.to_ascii_lowercase().contains(&query).then_some(idx))
                .collect();
        }

        if self.config_part_matches.is_empty() {
            self.config_part_list.select(None);
        } else {
            let idx = self.config_part_list.selected().unwrap_or(0);
            let next = idx.min(self.config_part_matches.len().saturating_sub(1));
            self.config_part_list.select(Some(next));
        }
    }

    fn select_config_part_from_name(&mut self, name: Option<&str>) {
        if self.config_part_matches.is_empty() {
            self.config_part_list.select(None);
            return;
        }

        let idx = name
            .and_then(|target| {
                self.config_part_matches.iter().position(|part_idx| {
                    self.config_parts.get(*part_idx).map(|s| s.as_str()) == Some(target)
                })
            })
            .unwrap_or(0);
        self.config_part_list.select(Some(idx));
    }

    fn selected_config_part_idx(&self) -> Option<usize> {
        let visible_idx = self.config_part_list.selected()?;
        self.config_part_matches.get(visible_idx).copied()
    }

    fn selected_config_key(&self) -> Option<&str> {
        let idx = self.config_key_list.selected()?;
        self.config_keys.get(idx).map(|s| s.as_str())
    }

    fn selected_config_part(&self) -> Option<&str> {
        let idx = self.selected_config_part_idx()?;
        self.config_parts.get(idx).map(|s| s.as_str())
    }

    fn selected_input_key(&self) -> Option<&str> {
        let idx = self.input_key_list.selected()?;
        self.input_keys.get(idx).map(|s| s.as_str())
    }

    fn selected_input_resolved_value(&self) -> Option<toml::Value> {
        let doc = self.doc_effective.as_ref()?;
        let key = self.selected_input_key()?;
        doc.value_path(&format!("inputs.resolved.{key}")).cloned()
    }

    fn selected_input_is_bool(&self) -> bool {
        let Some(doc) = self.doc_effective.as_ref() else {
            return false;
        };
        let Some(key) = self.selected_input_key() else {
            return false;
        };
        if let Some(kind) = doc
            .value_path(&format!("inputs.options.{key}.type"))
            .and_then(|v| v.as_str())
        {
            return kind.eq_ignore_ascii_case("bool");
        }
        doc.value_path(&format!("inputs.resolved.{key}"))
            .is_some_and(toml::Value::is_bool)
    }

    fn selected_input_choices(&self) -> Vec<toml::Value> {
        let Some(doc) = self.doc_effective.as_ref() else {
            return Vec::new();
        };
        let Some(key) = self.selected_input_key() else {
            return Vec::new();
        };
        doc.value_path(&format!("inputs.options.{key}.choices"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default()
    }

    fn selected_input_choice_index(&self, choices: &[toml::Value]) -> usize {
        if choices.is_empty() {
            return 0;
        }
        let current = self.selected_input_resolved_value();
        current
            .as_ref()
            .and_then(|cur| {
                choices
                    .iter()
                    .position(|choice| toml_values_equal_loose(cur, choice))
            })
            .unwrap_or(0)
    }

    fn select_next_input_key(&mut self) {
        if self.input_keys.is_empty() {
            return;
        }
        let i = self.input_key_list.selected().unwrap_or(0);
        let next = (i + 1).min(self.input_keys.len().saturating_sub(1));
        self.input_key_list.select(Some(next));
    }

    fn select_prev_input_key(&mut self) {
        if self.input_keys.is_empty() {
            return;
        }
        let i = self.input_key_list.selected().unwrap_or(0);
        self.input_key_list.select(Some(i.saturating_sub(1)));
    }

    fn select_next_config_part(&mut self) {
        if self.config_part_matches.is_empty() {
            return;
        }
        let i = self.config_part_list.selected().unwrap_or(0);
        let next = (i + 1).min(self.config_part_matches.len().saturating_sub(1));
        self.config_part_list.select(Some(next));
        self.config_scroll = 0;
    }

    fn select_prev_config_part(&mut self) {
        if self.config_part_matches.is_empty() {
            return;
        }
        let i = self.config_part_list.selected().unwrap_or(0);
        self.config_part_list.select(Some(i.saturating_sub(1)));
        self.config_scroll = 0;
    }

    fn begin_config_part_search(&mut self) {
        self.input = InputMode::SearchConfigParts {
            buffer: self.config_part_query.clone(),
        };
    }

    fn apply_config_part_search(&mut self) {
        let buffer = match &self.input {
            InputMode::SearchConfigParts { buffer } => buffer.clone(),
            _ => return,
        };
        let previous = self.selected_config_part().map(ToOwned::to_owned);
        self.config_part_query = buffer.trim().to_string();
        self.refresh_config_part_matches();
        self.select_config_part_from_name(previous.as_deref());
        self.config_scroll = 0;
        self.input = InputMode::Normal;
    }

    fn clear_config_part_search(&mut self) {
        if self.config_part_query.is_empty() {
            return;
        }
        let previous = self.selected_config_part().map(ToOwned::to_owned);
        self.config_part_query.clear();
        self.refresh_config_part_matches();
        self.select_config_part_from_name(previous.as_deref());
        self.config_scroll = 0;
    }

    fn select_next_config_key(&mut self) {
        if self.config_keys.is_empty() {
            return;
        }
        let i = self.config_key_list.selected().unwrap_or(0);
        let next = (i + 1).min(self.config_keys.len().saturating_sub(1));
        self.config_key_list.select(Some(next));
    }

    fn select_prev_config_key(&mut self) {
        if self.config_keys.is_empty() {
            return;
        }
        let i = self.config_key_list.selected().unwrap_or(0);
        self.config_key_list.select(Some(i.saturating_sub(1)));
    }

    fn set_override_bool(&mut self, path: &str, v: bool) -> Result<()> {
        self.set_override_value(path, toml::Value::Boolean(v))
    }

    fn clear_override_path(&mut self, path: &str) -> Result<()> {
        let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
        if segs.is_empty() {
            return Err(Error::msg("empty override path"));
        }

        if let toml::Value::Table(root) = &mut self.overrides {
            remove_override_path_from_table(root, &segs);
        }
        self.persist_overrides()
    }

    fn set_override_value(&mut self, path: &str, v: toml::Value) -> Result<()> {
        {
            let toml::Value::Table(root) = &mut self.overrides else {
                self.overrides = empty_overrides_table();
                return self.set_override_value(path, v);
            };

            let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
            if segs.is_empty() {
                return Err(Error::msg("empty override path"));
            }

            let mut cur = root;
            for seg in &segs[..segs.len() - 1] {
                let entry = cur
                    .entry((*seg).to_string())
                    .or_insert_with(|| toml::Value::Table(Default::default()));
                cur = entry
                    .as_table_mut()
                    .ok_or_else(|| Error::msg(format!("override path collides at '{seg}'")))?;
            }
            cur.insert(segs[segs.len() - 1].to_string(), v);
        }

        self.persist_overrides()
    }

    fn persist_overrides(&self) -> Result<()> {
        let Some(path) = self.overrides_file.as_ref() else {
            return Ok(());
        };
        write_tui_overrides(path, &self.overrides)
    }

    fn clear_override_state(&mut self) {
        self.overrides = empty_overrides_table();
        self.overrides_file = None;
    }

    fn reset_for_picker(&mut self) {
        self.clear_override_state();
        self.selected_build = None;
        self.doc_base = None;
        self.doc_effective = None;
        self.enabled_modules.clear();
        self.module_list.select(None);
        self.config_keys.clear();
        self.config_key_list.select(None);
        self.input_keys.clear();
        self.input_key_list.select(None);
        self.config_parts.clear();
        self.config_part_query.clear();
        self.config_part_matches.clear();
        self.config_part_list.select(None);
        self.tasks.clear();
        self.task_list.select(None);
        self.task_state.clear();
        self.task_logs.clear();
        self.checkpoint_status.clear();
        self.checkpoint_list.select(None);
    }

    fn return_to_picker(&mut self) -> Result<()> {
        self.reset_for_picker();
        self.builds = find_tomls(&self.builds_dir)?;
        if self.builds.is_empty() {
            self.build_list.select(None);
        } else {
            self.build_list.select(Some(0));
        }
        self.screen = Screen::Picker;
        Ok(())
    }

    fn toggle_selected_bool(&mut self) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref() else {
            return Ok(());
        };
        let Some(mid) = self.selected_module() else {
            return Ok(());
        };
        let Some(key) = self.selected_config_key() else {
            return Ok(());
        };
        let full = format!("{mid}.{key}");
        let Some(val) = doc.value_path(&full) else {
            return Ok(());
        };
        let Some(b) = val.as_bool() else {
            self.push_sidebar("not a bool");
            return Ok(());
        };
        self.set_override_bool(&full, !b)?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        Ok(())
    }

    fn toggle_selected_input_bool(&mut self) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref() else {
            return Ok(());
        };
        let Some(key) = self.selected_input_key() else {
            return Ok(());
        };
        let resolved_path = format!("inputs.resolved.{key}");
        let Some(val) = doc.value_path(&resolved_path) else {
            self.push_sidebar("missing input value");
            return Ok(());
        };
        let Some(b) = val.as_bool() else {
            self.push_sidebar("input is not bool");
            return Ok(());
        };
        let override_path = format!("inputs.values.{key}");
        self.set_override_bool(&override_path, !b)?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        Ok(())
    }

    fn cycle_selected_input_choice(&mut self, forward: bool) -> Result<()> {
        let Some(key) = self.selected_input_key().map(str::to_string) else {
            return Ok(());
        };
        let choices = self.selected_input_choices();
        if choices.is_empty() {
            self.push_sidebar("input has no choices");
            return Ok(());
        }
        let cur_idx = self.selected_input_choice_index(&choices);
        let next_idx = if forward {
            (cur_idx + 1) % choices.len()
        } else if cur_idx == 0 {
            choices.len().saturating_sub(1)
        } else {
            cur_idx.saturating_sub(1)
        };
        let override_path = format!("inputs.values.{key}");
        self.set_override_value(&override_path, choices[next_idx].clone())?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        Ok(())
    }

    fn begin_select_input_choice(&mut self) {
        let Some(key) = self.selected_input_key().map(str::to_string) else {
            return;
        };
        let options = self.selected_input_choices();
        if options.is_empty() {
            self.push_sidebar("input has no choices");
            return;
        }
        let selected = self.selected_input_choice_index(&options);
        self.input = InputMode::SelectInputChoice {
            key,
            options,
            selected,
        };
    }

    fn apply_select_input_choice(&mut self) -> Result<()> {
        let (key, options, selected) = match &self.input {
            InputMode::SelectInputChoice {
                key,
                options,
                selected,
            } => (key.clone(), options.clone(), *selected),
            _ => return Ok(()),
        };
        let Some(value) = options.get(selected).cloned() else {
            self.push_sidebar("invalid selector state");
            self.input = InputMode::Normal;
            return Ok(());
        };
        let override_path = format!("inputs.values.{key}");
        self.set_override_value(&override_path, value)?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        self.input = InputMode::Normal;
        Ok(())
    }

    fn reset_input_key_to_default_override(&mut self, key: &str) -> Result<()> {
        let default = self
            .doc_effective
            .as_ref()
            .and_then(|doc| doc.value_path(&format!("inputs.options.{key}.default")))
            .cloned();
        let override_path = format!("inputs.values.{key}");
        match default {
            Some(v) if is_primitive_input_value(&v) => self.set_override_value(&override_path, v),
            Some(_) => self.clear_override_path(&override_path),
            None => self.clear_override_path(&override_path),
        }
    }

    fn reset_selected_input_to_default(&mut self) -> Result<()> {
        let Some(key) = self.selected_input_key().map(str::to_string) else {
            return Ok(());
        };
        self.reset_input_key_to_default_override(&key)?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        self.push_sidebar(&format!("reset '{key}' to default"));
        Ok(())
    }

    fn reset_all_inputs_to_default(&mut self) -> Result<()> {
        let keys = self.input_keys.clone();
        for key in &keys {
            self.reset_input_key_to_default_override(key)?;
        }
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        self.push_sidebar("reset all inputs to defaults");
        Ok(())
    }

    fn begin_edit_value(&mut self) {
        let Some(doc) = self.doc_effective.as_ref() else {
            return;
        };
        let Some(mid) = self.selected_module() else {
            return;
        };
        let Some(key) = self.selected_config_key() else {
            return;
        };
        let full = format!("{mid}.{key}");
        let Some(val) = doc.value_path(&full) else {
            self.push_sidebar("missing value");
            return;
        };

        let (kind, buffer) = match val {
            toml::Value::Boolean(b) => (ValueKind::Bool, b.to_string()),
            toml::Value::Integer(i) => (ValueKind::Int, i.to_string()),
            toml::Value::Float(f) => (ValueKind::Float, f.to_string()),
            toml::Value::String(s) => (ValueKind::String, s.clone()),
            _ => {
                self.push_sidebar("complex value; edit not supported yet");
                return;
            }
        };

        self.input = InputMode::EditValue {
            full_path: full,
            kind,
            buffer,
            error: None,
        };
    }

    fn begin_edit_input_value(&mut self) {
        if !self.selected_input_choices().is_empty() {
            self.begin_select_input_choice();
            return;
        }
        if self.selected_input_is_bool() {
            if let Err(e) = self.toggle_selected_input_bool() {
                self.push_sidebar(&format!("failed to toggle input: {e}"));
            }
            return;
        }

        let Some(doc) = self.doc_effective.as_ref() else {
            return;
        };
        let Some(key) = self.selected_input_key() else {
            return;
        };

        let resolved_path = format!("inputs.resolved.{key}");
        let Some(val) = doc.value_path(&resolved_path) else {
            self.push_sidebar("missing input value");
            return;
        };

        let kind = match val {
            toml::Value::Boolean(_) => ValueKind::Bool,
            toml::Value::Integer(_) => ValueKind::Int,
            toml::Value::Float(_) => ValueKind::Float,
            toml::Value::String(_) => ValueKind::String,
            _ => {
                self.push_sidebar("complex input value; edit not supported");
                return;
            }
        };
        let buffer = match val {
            toml::Value::Boolean(b) => b.to_string(),
            toml::Value::Integer(i) => i.to_string(),
            toml::Value::Float(f) => f.to_string(),
            toml::Value::String(s) => s.clone(),
            _ => String::new(),
        };

        let full = format!("inputs.values.{key}");
        self.input = InputMode::EditValue {
            full_path: full,
            kind,
            buffer,
            error: None,
        };
    }

    fn begin_edit_global_int(&mut self, name: &str, current: i64) {
        self.input = InputMode::EditGlobalInt {
            name: name.to_string(),
            buffer: current.to_string(),
            error: None,
        };
    }

    fn begin_edit_global_float(&mut self, name: &str, current: f64) {
        self.input = InputMode::EditGlobalFloat {
            name: name.to_string(),
            buffer: current.to_string(),
            error: None,
        };
    }

    fn apply_edit_value(&mut self) -> Result<()> {
        let (full_path, kind, buffer) = match &self.input {
            InputMode::EditValue {
                full_path,
                kind,
                buffer,
                ..
            } => (full_path.clone(), kind.clone(), buffer.clone()),
            _ => return Ok(()),
        };

        let val = match kind {
            ValueKind::Bool => match buffer.trim() {
                "true" => toml::Value::Boolean(true),
                "false" => toml::Value::Boolean(false),
                _ => {
                    self.input = InputMode::EditValue {
                        full_path,
                        kind,
                        buffer,
                        error: Some("expected true/false".into()),
                    };
                    return Ok(());
                }
            },
            ValueKind::Int => match buffer.trim().parse::<i64>() {
                Ok(v) => toml::Value::Integer(v),
                Err(_) => {
                    self.input = InputMode::EditValue {
                        full_path,
                        kind,
                        buffer,
                        error: Some("expected integer".into()),
                    };
                    return Ok(());
                }
            },
            ValueKind::Float => match buffer.trim().parse::<f64>() {
                Ok(v) => toml::Value::Float(v),
                Err(_) => {
                    self.input = InputMode::EditValue {
                        full_path,
                        kind,
                        buffer,
                        error: Some("expected float".into()),
                    };
                    return Ok(());
                }
            },
            ValueKind::String => toml::Value::String(buffer.clone()),
        };

        self.set_override_value(&full_path, val)?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        self.input = InputMode::Normal;
        Ok(())
    }

    fn apply_edit_global_int(&mut self) -> Result<()> {
        let (name, buffer) = match &self.input {
            InputMode::EditGlobalInt { name, buffer, .. } => (name.clone(), buffer.clone()),
            _ => return Ok(()),
        };

        let parsed = match buffer.trim().parse::<i64>() {
            Ok(v) => v,
            Err(_) => {
                self.input = InputMode::EditGlobalInt {
                    name,
                    buffer,
                    error: Some("expected integer".into()),
                };
                return Ok(());
            }
        };

        match name.as_str() {
            "max_parallel" => {
                self.max_parallel = parsed.max(0) as usize;
            }
            _ => {
                self.push_sidebar("unknown setting");
            }
        }

        self.input = InputMode::Normal;
        Ok(())
    }

    fn apply_edit_global_float(&mut self) -> Result<()> {
        let (name, buffer) = match &self.input {
            InputMode::EditGlobalFloat { name, buffer, .. } => (name.clone(), buffer.clone()),
            _ => return Ok(()),
        };

        let parsed = match buffer.trim().parse::<f64>() {
            Ok(v) => v,
            Err(_) => {
                self.input = InputMode::EditGlobalFloat {
                    name,
                    buffer,
                    error: Some("expected float".into()),
                };
                return Ok(());
            }
        };

        match name.as_str() {
            "buildroot.top_level_load" => {
                self.set_override_value("buildroot.top_level_load", toml::Value::Float(parsed))?;
                self.recompute_effective_doc()?;
            }
            _ => {
                self.push_sidebar("unknown setting");
            }
        }

        self.input = InputMode::Normal;
        Ok(())
    }

    fn effective_buildroot_top_level_load(&self) -> Option<f64> {
        let doc = self.doc_effective.as_ref()?;
        let v = doc.value_path("buildroot.top_level_load")?;
        if let Some(f) = v.as_float() {
            return Some(f);
        }
        v.as_integer().map(|i| i as f64)
    }

    fn effective_buildroot_performance_profile(&self) -> String {
        self.doc_effective
            .as_ref()
            .and_then(|d| d.value_path("buildroot.performance_profile"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "max".into())
    }

    fn cycle_buildroot_performance_profile(&mut self) -> Result<()> {
        let cur = self.effective_buildroot_performance_profile();
        let next = match cur.as_str() {
            "max" => "balanced",
            "balanced" => "safe",
            "safe" => "max",
            _ => "max",
        };
        self.set_override_value(
            "buildroot.performance_profile",
            toml::Value::String(next.into()),
        )?;
        self.recompute_effective_doc()?;
        Ok(())
    }

    fn toggle_module_enabled(&mut self) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref() else {
            return Ok(());
        };
        let Some(mid) = self.selected_module() else {
            return Ok(());
        };
        let full = format!("{mid}.enabled");
        let current = doc
            .value_path(&full)
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        self.set_override_bool(&full, !current)?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        Ok(())
    }

    fn enter_keys_screen(&mut self) {
        self.recompute_config_keys();
        self.screen = Screen::Keys;
    }

    fn enter_inputs_screen(&mut self) {
        self.recompute_input_keys();
        self.screen = Screen::Inputs;
    }

    fn start_run(&mut self) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref().cloned() else {
            return Ok(());
        };

        let (tx, rx) = std::sync::mpsc::channel::<ExecEvent>();
        let sink = Arc::new(ChannelSink::new(tx));
        let ctx = ExecCtx::new(self.dry_run, sink);

        // Clamp max_parallel similarly to CLI.
        let max_parallel = if self.max_parallel == 0 {
            num_cpus::get().max(1)
        } else {
            self.max_parallel.max(1)
        };

        let ctx_for_thread = ctx.clone();
        std::thread::spawn(move || {
            let mut plan = crate::planner::Plan::default();
            let modules = crate::modules::builtin_modules();
            for m in &modules {
                if m.detect(&doc) {
                    if let Err(e) = m.plan(&doc, &mut plan) {
                        ctx_for_thread.sink.emit(ExecEvent::ExecutorDone {
                            ok: false,
                            error: Some(e.to_string()),
                        });
                        return;
                    }
                }
            }
            if let Err(e) = crate::checkpoints::validate_against_plan(&doc, &plan) {
                ctx_for_thread.sink.emit(ExecEvent::ExecutorDone {
                    ok: false,
                    error: Some(e.to_string()),
                });
                return;
            }
            if let Err(e) = plan.finalize_default() {
                ctx_for_thread.sink.emit(ExecEvent::ExecutorDone {
                    ok: false,
                    error: Some(e.to_string()),
                });
                return;
            }

            let reg = match crate::executor::builtin_registry() {
                Ok(r) => r,
                Err(e) => {
                    ctx_for_thread.sink.emit(ExecEvent::ExecutorDone {
                        ok: false,
                        error: Some(e.to_string()),
                    });
                    return;
                }
            };

            let _ = crate::executor::execute_plan_parallel(
                &doc,
                &plan,
                &reg,
                &ctx_for_thread,
                max_parallel,
            );
        });

        self.exec_rx = Some(rx);
        self.exec_ctx = Some(ctx);
        self.exec_thread_done = false;
        self.exit_at = None;
        self.exec_started_at = Some(Instant::now());
        self.exec_last_tick = Instant::now();
        self.screen = Screen::Run;
        self.run_tab = RunTab::Overview;
        self.all_logs.clear();
        self.sidebar_logs.clear();
        self.all_logs_scroll = 0;
        self.task_log_scroll = 0;
        self.config_scroll = 0;
        self.task_error_log_dir = None;
        self.task_error_logs.clear();
        self.task_error_logged.clear();
        self.system_last_sample = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        self.sample_system_metrics();
        Ok(())
    }

    fn cancel_run(&mut self) {
        if let Some(ctx) = self.exec_ctx.as_ref() {
            ctx.request_cancel();
            self.push_sidebar("cancel requested");
        }
    }

    fn selected_module(&self) -> Option<&str> {
        let idx = self.module_list.selected()?;
        self.enabled_modules.get(idx).map(|s| s.as_str())
    }

    fn selected_task(&self) -> Option<&str> {
        let idx = self.task_list.selected()?;
        self.tasks.get(idx).map(|s| s.as_str())
    }

    fn selected_checkpoint(&self) -> Option<&crate::checkpoints::CheckpointStatus> {
        let idx = self.checkpoint_list.selected()?;
        self.checkpoint_status.get(idx)
    }

    fn selected_checkpoint_index(&self) -> Option<usize> {
        let idx = self.checkpoint_list.selected()?;
        (idx < self.checkpoint_status.len()).then_some(idx)
    }

    fn select_next_checkpoint(&mut self) {
        if self.checkpoint_status.is_empty() {
            return;
        }
        let i = self.checkpoint_list.selected().unwrap_or(0);
        let next = (i + 1).min(self.checkpoint_status.len().saturating_sub(1));
        self.checkpoint_list.select(Some(next));
    }

    fn select_prev_checkpoint(&mut self) {
        if self.checkpoint_status.is_empty() {
            return;
        }
        let i = self.checkpoint_list.selected().unwrap_or(0);
        self.checkpoint_list.select(Some(i.saturating_sub(1)));
    }

    fn selected_quick_item(&self) -> Option<QuickItem> {
        let idx = self.quick_list.selected()?;
        self.quick_items.get(idx).copied()
    }

    fn select_next_quick(&mut self) {
        if self.quick_items.is_empty() {
            return;
        }
        let i = self.quick_list.selected().unwrap_or(0);
        let next = (i + 1).min(self.quick_items.len().saturating_sub(1));
        self.quick_list.select(Some(next));
    }

    fn select_prev_quick(&mut self) {
        if self.quick_items.is_empty() {
            return;
        }
        let i = self.quick_list.selected().unwrap_or(0);
        self.quick_list.select(Some(i.saturating_sub(1)));
    }

    fn select_next_task(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let i = self.task_list.selected().unwrap_or(0);
        let next = (i + 1).min(self.tasks.len().saturating_sub(1));
        self.task_list.select(Some(next));
    }

    fn select_prev_task(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let i = self.task_list.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.task_list.select(Some(prev));
        self.task_log_scroll = 0;
    }

    fn select_next_run_tab(&mut self) {
        self.run_tab = match self.run_tab {
            RunTab::Overview => RunTab::Tasks,
            RunTab::Tasks => RunTab::Logs,
            RunTab::Logs => RunTab::TaskLog,
            RunTab::TaskLog => RunTab::Config,
            RunTab::Config => RunTab::Overview,
        };
    }

    fn select_prev_run_tab(&mut self) {
        self.run_tab = match self.run_tab {
            RunTab::Overview => RunTab::Config,
            RunTab::Tasks => RunTab::Overview,
            RunTab::Logs => RunTab::Tasks,
            RunTab::TaskLog => RunTab::Logs,
            RunTab::Config => RunTab::TaskLog,
        };
    }

    fn scroll_active_view(&mut self, delta: isize) {
        match self.run_tab {
            RunTab::Logs => {
                self.all_logs_scroll = add_signed_saturating(self.all_logs_scroll, delta);
            }
            RunTab::TaskLog => {
                self.task_log_scroll = add_signed_saturating(self.task_log_scroll, delta);
            }
            RunTab::Config => {
                // Config scroll is top-based; up should decrease offset.
                self.config_scroll = add_signed_saturating(self.config_scroll, -delta);
            }
            _ => {}
        }
    }

    fn home_active_view(&mut self) {
        match self.run_tab {
            RunTab::Logs => self.all_logs_scroll = usize::MAX,
            RunTab::TaskLog => self.task_log_scroll = usize::MAX,
            RunTab::Config => self.config_scroll = 0,
            _ => {}
        }
    }

    fn end_active_view(&mut self) {
        match self.run_tab {
            RunTab::Logs => self.all_logs_scroll = 0,
            RunTab::TaskLog => self.task_log_scroll = 0,
            RunTab::Config => self.config_scroll = usize::MAX,
            _ => {}
        }
    }

    fn sample_system_metrics(&mut self) {
        if self.system_last_sample.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.system_last_sample = Instant::now();

        let cores = num_cpus::get().max(1);
        let load_1m = read_loadavg_1m();
        let load_pct = load_1m.map(|v| ((v / cores as f64) * 100.0).clamp(0.0, 100.0));

        let (mem_total_kib, mem_used_kib, mem_pct) = match read_mem_usage_kib() {
            Some((total, used)) if total > 0 => {
                let pct = (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
                (Some(total), Some(used), Some(pct))
            }
            _ => (None, None, None),
        };

        let (disk_total_bytes, disk_used_bytes, disk_pct) = match read_root_disk_usage_bytes() {
            Some((total, used)) if total > 0 => {
                let pct = (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
                (Some(total), Some(used), Some(pct))
            }
            _ => (None, None, None),
        };

        self.system_snapshot = SystemSnapshot {
            cpu_cores: cores,
            load_1m,
            load_pct,
            mem_total_kib,
            mem_used_kib,
            mem_pct,
            disk_total_bytes,
            disk_used_bytes,
            disk_pct,
        };

        if let Some(v) = load_pct {
            push_bounded_f64(&mut self.load_history, v, 360);
        }
        if let Some(v) = mem_pct {
            push_bounded_f64(&mut self.mem_history, v, 360);
        }
        if let Some(v) = disk_pct {
            push_bounded_f64(&mut self.disk_history, v, 360);
        }
    }

    fn push_log(&mut self, line: String) {
        let line = sanitize_log_line(&line);
        push_bounded(&mut self.all_logs, line.clone(), 2000);
        push_bounded(&mut self.sidebar_logs, line.clone(), 200);
    }

    fn push_sidebar(&mut self, line: &str) {
        let line = sanitize_log_line(line);
        // Sidebar column was removed; keep these messages visible in the main Logs view.
        push_bounded(&mut self.all_logs, line.clone(), 2000);
        push_bounded(&mut self.sidebar_logs, line, 200);
    }

    fn effective_workspace_clean(&self) -> CleanMode {
        let Some(doc) = self.doc_effective.as_ref() else {
            return CleanMode::None;
        };
        let ws: WorkspaceConfig = doc
            .deserialize_path("workspace")
            .ok()
            .flatten()
            .unwrap_or_default();
        ws.clean
    }

    fn set_workspace_clean(&mut self, m: CleanMode) -> Result<()> {
        let s = match m {
            CleanMode::None => "none",
            CleanMode::Build => "build",
            CleanMode::Out => "out",
            CleanMode::All => "all",
        };
        self.set_override_value("workspace.clean", toml::Value::String(s.into()))?;
        self.recompute_effective_doc()?;
        Ok(())
    }

    fn cycle_workspace_clean(&mut self) -> Result<()> {
        let cur = self.effective_workspace_clean();
        let next = match cur {
            CleanMode::None => CleanMode::Build,
            CleanMode::Build => CleanMode::Out,
            CleanMode::Out => CleanMode::All,
            CleanMode::All => CleanMode::None,
        };
        self.set_workspace_clean(next)
    }

    fn retry_checkpoint_uploads(&mut self) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref().cloned() else {
            return Ok(());
        };
        let report = crate::checkpoints::retry_pending_uploads(&doc, None)?;
        self.push_sidebar(&format!(
            "checkpoint retry: attempted={} uploaded={} failed={}",
            report.attempted, report.uploaded, report.failed
        ));
        self.refresh_checkpoint_status()?;
        Ok(())
    }

    fn refresh_checkpoint_status(&mut self) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref().cloned() else {
            return Ok(());
        };
        self.checkpoint_status = crate::checkpoints::status_for_doc(&doc)?;
        if self.checkpoint_status.is_empty() {
            self.checkpoint_list.select(None);
        } else if self.checkpoint_list.selected().is_none() {
            self.checkpoint_list.select(Some(0));
        } else {
            let idx = self
                .checkpoint_list
                .selected()
                .unwrap_or(0)
                .min(self.checkpoint_status.len().saturating_sub(1));
            self.checkpoint_list.select(Some(idx));
        }
        Ok(())
    }

    fn set_checkpoint_config_override(
        &mut self,
        mutator: impl FnOnce(&mut crate::checkpoints::CheckpointsConfig, usize) -> crate::Result<()>,
    ) -> Result<()> {
        let Some(doc) = self.doc_effective.as_ref() else {
            return Ok(());
        };
        let Some(idx) = self.selected_checkpoint_index() else {
            self.push_sidebar("no checkpoint selected");
            return Ok(());
        };
        let mut cfg: crate::checkpoints::CheckpointsConfig =
            doc.deserialize_path("checkpoints")?.unwrap_or_default();
        if idx >= cfg.points.len() {
            return Ok(());
        }
        let selected_id = cfg
            .points
            .get(idx)
            .map(|p| p.id.clone())
            .unwrap_or_default();
        mutator(&mut cfg, idx)?;
        let val = toml::Value::try_from(cfg)
            .map_err(|e| Error::msg(format!("failed to encode checkpoint config override: {e}")))?;
        self.set_override_value("checkpoints", val)?;
        self.recompute_effective_doc()?;
        self.recompute_modules_and_tasks()?;
        if !selected_id.trim().is_empty()
            && let Some(pos) = self
                .checkpoint_status
                .iter()
                .position(|s| s.id == selected_id.trim())
        {
            self.checkpoint_list.select(Some(pos));
        }
        Ok(())
    }

    fn cycle_selected_checkpoint_use_policy(&mut self) -> Result<()> {
        self.set_checkpoint_config_override(|cfg, idx| {
            let cur = cfg.points[idx].use_policy.unwrap_or(cfg.default_use_policy);
            let next = match cur {
                crate::checkpoints::CheckpointUsePolicy::Auto => {
                    crate::checkpoints::CheckpointUsePolicy::Off
                }
                crate::checkpoints::CheckpointUsePolicy::Off => {
                    crate::checkpoints::CheckpointUsePolicy::Required
                }
                crate::checkpoints::CheckpointUsePolicy::Required => {
                    crate::checkpoints::CheckpointUsePolicy::Auto
                }
            };
            cfg.points[idx].use_policy = Some(next);
            Ok(())
        })
    }

    fn cycle_selected_checkpoint_upload_policy(&mut self) -> Result<()> {
        self.set_checkpoint_config_override(|cfg, idx| {
            let cur = cfg.points[idx]
                .upload_policy
                .unwrap_or(cfg.default_upload_policy);
            let next = match cur {
                crate::checkpoints::CheckpointUploadPolicy::Off => {
                    crate::checkpoints::CheckpointUploadPolicy::OnSuccess
                }
                crate::checkpoints::CheckpointUploadPolicy::OnSuccess => {
                    crate::checkpoints::CheckpointUploadPolicy::Always
                }
                crate::checkpoints::CheckpointUploadPolicy::Always => {
                    crate::checkpoints::CheckpointUploadPolicy::Off
                }
            };
            cfg.points[idx].upload_policy = Some(next);
            Ok(())
        })
    }

    fn request_force_exit(&mut self, code: i32) {
        if let Some(ctx) = self.exec_ctx.as_ref() {
            ctx.request_cancel();
            ctx.kill_running_children_force();
        }
        self.force_exit_code = Some(code);
    }

    fn push_task_log(&mut self, task_id: &str, line: String) {
        let line = sanitize_log_line(&line);
        if let Some(q) = self.task_logs.get_mut(task_id) {
            push_bounded(q, line.clone(), 2000);
        }
        if let Some((parent, _)) = task_id.split_once(':')
            && let Some(q) = self.task_logs.get_mut(parent)
        {
            push_bounded(q, line.clone(), 2000);
        }
        self.push_log(format!("[{task_id}] {line}"));
    }

    fn ensure_task_error_log_dir(&mut self) -> Option<PathBuf> {
        if let Some(dir) = self.task_error_log_dir.as_ref() {
            return Some(dir.clone());
        }

        let ws_cfg: WorkspaceConfig = self
            .doc_effective
            .as_ref()
            .and_then(|doc| doc.deserialize_path("workspace").ok().flatten())
            .unwrap_or_default();
        let base = crate::workspace::load_paths(&ws_cfg)
            .map(|p| p.build_dir)
            .unwrap_or_else(|_| PathBuf::from("build"))
            .join("error-logs");
        let dir = base.join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
        if let Err(e) = fs::create_dir_all(&dir) {
            self.push_sidebar(&format!(
                "failed to create error log dir {}: {e}",
                dir.display()
            ));
            return None;
        }
        self.task_error_log_dir = Some(dir.clone());
        Some(dir)
    }

    fn write_task_error_log(&mut self, task_id: &str, error: Option<&str>, elapsed_ms: u128) {
        if self.task_error_logged.contains(task_id) {
            return;
        }
        let Some(dir) = self.ensure_task_error_log_dir() else {
            return;
        };

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
        if let Some(lines) = self.task_logs.get(task_id) {
            for line in lines {
                body.push_str(line);
                body.push('\n');
            }
        }

        match fs::write(&path, body) {
            Ok(()) => {
                self.task_error_logged.insert(task_id.to_string());
                self.task_error_logs.push(path.clone());
                self.push_sidebar(&format!("error log: {}", path.display()));
            }
            Err(e) => {
                self.push_sidebar(&format!(
                    "failed to write error log {}: {e}",
                    path.display()
                ));
            }
        }
    }

    fn repair_log_buffers(&mut self, reason: &str) {
        for line in self.all_logs.iter_mut() {
            *line = sanitize_log_line(line);
        }
        for line in self.sidebar_logs.iter_mut() {
            *line = sanitize_log_line(line);
        }
        for logs in self.task_logs.values_mut() {
            for line in logs.iter_mut() {
                *line = sanitize_log_line(line);
            }
        }
        for state in self.task_state.values_mut() {
            if let Some(last) = state.last_line.as_mut() {
                *last = sanitize_log_line(last);
                if last.is_empty() {
                    state.last_line = None;
                }
            }
        }
        self.push_sidebar(&format!(
            "ui repaired log view: {}",
            sanitize_log_line(reason)
        ));
    }

    fn drain_exec_events(&mut self) {
        let mut events = Vec::new();
        if let Some(rx) = self.exec_rx.as_ref() {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            match ev {
                ExecEvent::TaskSpawned { id } => {
                    self.push_sidebar(&format!("spawn {id}"));
                }
                ExecEvent::TaskStarted { id } => {
                    if let Some(s) = self.task_state.get_mut(&id) {
                        s.status = TaskStatus::Running;
                    }
                    self.push_sidebar(&format!("run {id}"));
                }
                ExecEvent::TaskLog { id, line } => {
                    self.push_task_log(&id, line.clone());
                    let line = sanitize_log_line(&line);
                    if let Some(s) = self.task_state.get_mut(&id) {
                        if !line.is_empty() {
                            s.last_line = Some(line.clone());
                        }
                    } else if let Some((parent, _)) = id.split_once(':')
                        && let Some(s) = self.task_state.get_mut(parent)
                        && !line.is_empty()
                    {
                        s.last_line = Some(line);
                    }
                }
                ExecEvent::TaskFinished {
                    id,
                    ok,
                    error,
                    elapsed_ms,
                } => {
                    self.exec_done = self.exec_done.saturating_add(1);
                    if let Some(s) = self.task_state.get_mut(&id) {
                        s.status = if ok {
                            TaskStatus::Ok
                        } else {
                            TaskStatus::Failed
                        };
                        if let Some(e) = error.clone() {
                            let e = sanitize_log_line(&e);
                            if !e.is_empty() {
                                s.last_line = Some(e);
                            }
                        } else {
                            s.last_line = Some(format!("{elapsed_ms}ms"));
                        }
                    }
                    if ok {
                        self.push_sidebar(&format!("done {id} ({elapsed_ms}ms)"));
                    } else {
                        self.write_task_error_log(&id, error.as_deref(), elapsed_ms);
                        self.push_sidebar(&format!(
                            "fail {id} ({elapsed_ms}ms) {}",
                            error.unwrap_or_default()
                        ));
                    }
                }
                ExecEvent::ExecutorDone { ok, error } => {
                    self.exec_ok = Some(ok);
                    self.exec_thread_done = true;
                    if ok {
                        self.push_sidebar("executor done ok");
                    } else {
                        self.push_sidebar(&format!(
                            "executor done fail {}",
                            error.unwrap_or_default()
                        ));
                    }
                    if self.auto_exit_on_done && self.screen == Screen::Run {
                        // Give one more render tick so the user sees the final state.
                        self.exit_at = Some(Instant::now() + Duration::from_millis(700));
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        // Global Ctrl+C handling.
        if mods.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')) {
            // If we're not actively running, exit immediately.
            if self.screen != Screen::Run || self.exec_thread_done {
                self.request_force_exit(130);
                return Ok(true);
            }

            // Running: first Ctrl+C opens confirm; second Ctrl+C force quits.
            if matches!(self.input, InputMode::ConfirmCancelRun) {
                self.request_force_exit(130);
                return Ok(true);
            }
            self.input = InputMode::ConfirmCancelRun;
            return Ok(false);
        }

        // Modal input handling.
        match &mut self.input {
            InputMode::SelectInputChoice {
                selected, options, ..
            } => match code {
                KeyCode::Esc => {
                    self.input = InputMode::Normal;
                    return Ok(false);
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    return self.apply_select_input_choice().map(|_| false);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !options.is_empty() {
                        *selected = (*selected + 1).min(options.len().saturating_sub(1));
                    }
                    return Ok(false);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = selected.saturating_sub(1);
                    return Ok(false);
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    if !options.is_empty() {
                        *selected = (*selected + 1).min(options.len().saturating_sub(1));
                    }
                    return Ok(false);
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    *selected = selected.saturating_sub(1);
                    return Ok(false);
                }
                _ => return Ok(false),
            },
            InputMode::EditValue { buffer, .. }
            | InputMode::EditGlobalInt { buffer, .. }
            | InputMode::EditGlobalFloat { buffer, .. }
            | InputMode::SearchConfigParts { buffer } => match code {
                KeyCode::Esc => {
                    self.input = InputMode::Normal;
                    return Ok(false);
                }
                KeyCode::Enter => match self.input {
                    InputMode::EditValue { .. } => {
                        return self.apply_edit_value().map(|_| false);
                    }
                    InputMode::EditGlobalInt { .. } => {
                        return self.apply_edit_global_int().map(|_| false);
                    }
                    InputMode::EditGlobalFloat { .. } => {
                        return self.apply_edit_global_float().map(|_| false);
                    }
                    InputMode::SearchConfigParts { .. } => {
                        self.apply_config_part_search();
                        return Ok(false);
                    }
                    _ => {}
                },
                KeyCode::Backspace => {
                    buffer.pop();
                    return Ok(false);
                }
                KeyCode::Char(c) => {
                    if c == '\n' || c == '\r' {
                        return Ok(false);
                    }
                    buffer.push(c);
                    return Ok(false);
                }
                _ => return Ok(false),
            },
            InputMode::ConfirmCancelRun => match code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.input = InputMode::Normal;
                    return Ok(false);
                }
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.request_force_exit(130);
                    return Ok(true);
                }
                _ => return Ok(false),
            },
            _ => {}
        }

        match self.screen {
            Screen::Picker => match code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
                KeyCode::Down | KeyCode::Char('j') => self.select_next_build(),
                KeyCode::Up | KeyCode::Char('k') => self.select_prev_build(),
                KeyCode::Enter => self.load_selected_build()?,
                KeyCode::Char('r') => {
                    self.builds = find_tomls(&self.builds_dir)?;
                    if !self.builds.is_empty() {
                        self.build_list.select(Some(0));
                    }
                }
                _ => {}
            },
            Screen::Quick => match code {
                KeyCode::Char('q') | KeyCode::Esc => self.return_to_picker()?,
                KeyCode::Down | KeyCode::Char('j') => self.select_next_quick(),
                KeyCode::Up | KeyCode::Char('k') => self.select_prev_quick(),
                KeyCode::Char(']') => self.select_next_checkpoint(),
                KeyCode::Char('[') => self.select_prev_checkpoint(),
                KeyCode::Char('s') | KeyCode::Char('r') => self.start_run()?,
                KeyCode::Char('m') => self.screen = Screen::Modules,
                KeyCode::Char('i') => self.enter_inputs_screen(),
                KeyCode::Char('c') => self.cycle_workspace_clean()?,
                KeyCode::Char('u') => self.cycle_selected_checkpoint_use_policy()?,
                KeyCode::Char('p') => self.cycle_selected_checkpoint_upload_policy()?,
                KeyCode::Char('R') => self.refresh_checkpoint_status()?,
                KeyCode::Char(' ') => match self.selected_quick_item() {
                    Some(QuickItem::DryRun) => self.dry_run = !self.dry_run,
                    Some(QuickItem::MaxParallel) => {
                        self.begin_edit_global_int("max_parallel", self.max_parallel as i64);
                    }
                    Some(QuickItem::BuildrootPerfProfile) => {
                        self.cycle_buildroot_performance_profile()?;
                    }
                    Some(QuickItem::TopLevelLoad) => {
                        self.begin_edit_global_float(
                            "buildroot.top_level_load",
                            self.effective_buildroot_top_level_load().unwrap_or(0.0),
                        );
                    }
                    Some(QuickItem::Inputs) => self.enter_inputs_screen(),
                    Some(QuickItem::RefreshCheckpointStatus) => self.refresh_checkpoint_status()?,
                    Some(QuickItem::RetryCheckpointUploads) => self.retry_checkpoint_uploads()?,
                    Some(QuickItem::CheckpointUsePolicy) => {
                        self.cycle_selected_checkpoint_use_policy()?
                    }
                    Some(QuickItem::CheckpointUploadPolicy) => {
                        self.cycle_selected_checkpoint_upload_policy()?
                    }
                    Some(QuickItem::Clean) => self.cycle_workspace_clean()?,
                    _ => {}
                },
                KeyCode::Enter => match self.selected_quick_item() {
                    Some(QuickItem::Start) => self.start_run()?,
                    Some(QuickItem::DryRun) => self.dry_run = !self.dry_run,
                    Some(QuickItem::MaxParallel) => {
                        self.begin_edit_global_int("max_parallel", self.max_parallel as i64);
                    }
                    Some(QuickItem::BuildrootPerfProfile) => {
                        self.cycle_buildroot_performance_profile()?;
                    }
                    Some(QuickItem::TopLevelLoad) => {
                        self.begin_edit_global_float(
                            "buildroot.top_level_load",
                            self.effective_buildroot_top_level_load().unwrap_or(0.0),
                        );
                    }
                    Some(QuickItem::Inputs) => self.enter_inputs_screen(),
                    Some(QuickItem::RefreshCheckpointStatus) => self.refresh_checkpoint_status()?,
                    Some(QuickItem::RetryCheckpointUploads) => self.retry_checkpoint_uploads()?,
                    Some(QuickItem::CheckpointUsePolicy) => {
                        self.cycle_selected_checkpoint_use_policy()?
                    }
                    Some(QuickItem::CheckpointUploadPolicy) => {
                        self.cycle_selected_checkpoint_upload_policy()?
                    }
                    Some(QuickItem::Clean) => self.cycle_workspace_clean()?,
                    Some(QuickItem::Modules) => self.screen = Screen::Modules,
                    Some(QuickItem::Back) => self.screen = Screen::Picker,
                    None => {}
                },
                _ => {}
            },
            Screen::Modules => match code {
                KeyCode::Char('q') | KeyCode::Esc => self.screen = Screen::Quick,
                KeyCode::Down | KeyCode::Char('j') => {
                    let i = self.module_list.selected().unwrap_or(0);
                    let next = (i + 1).min(self.enabled_modules.len().saturating_sub(1));
                    self.module_list.select(Some(next));
                    self.recompute_config_keys();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let i = self.module_list.selected().unwrap_or(0);
                    self.module_list.select(Some(i.saturating_sub(1)));
                    self.recompute_config_keys();
                }
                KeyCode::Char(' ') | KeyCode::Char('e') => self.toggle_module_enabled()?,
                KeyCode::Enter => self.enter_keys_screen(),
                KeyCode::Char('r') => self.start_run()?,
                _ => {}
            },
            Screen::Keys => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Modules,
                KeyCode::Down | KeyCode::Char('j') => self.select_next_config_key(),
                KeyCode::Up | KeyCode::Char('k') => self.select_prev_config_key(),
                KeyCode::Char(' ') => self.toggle_selected_bool()?,
                KeyCode::Enter | KeyCode::Char('e') => self.begin_edit_value(),
                _ => {}
            },
            Screen::Inputs => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Quick,
                KeyCode::Down | KeyCode::Char('j') => self.select_next_input_key(),
                KeyCode::Up | KeyCode::Char('k') => self.select_prev_input_key(),
                KeyCode::Char(' ') => {
                    let choices = self.selected_input_choices();
                    if !choices.is_empty() {
                        self.cycle_selected_input_choice(true)?;
                    } else if self.selected_input_is_bool() {
                        self.toggle_selected_input_bool()?;
                    } else {
                        self.begin_edit_input_value();
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    if !self.selected_input_choices().is_empty() {
                        self.cycle_selected_input_choice(false)?;
                    }
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    if !self.selected_input_choices().is_empty() {
                        self.cycle_selected_input_choice(true)?;
                    }
                }
                KeyCode::Enter | KeyCode::Char('e') => self.begin_edit_input_value(),
                KeyCode::Char('d') => self.reset_selected_input_to_default()?,
                KeyCode::Char('D') => self.reset_all_inputs_to_default()?,
                KeyCode::Char('r') => self.start_run()?,
                _ => {}
            },
            Screen::Run => match code {
                KeyCode::Char('q') => {
                    // Only allow exit if not running.
                    if self.exec_thread_done {
                        return Ok(true);
                    }
                }
                KeyCode::Char('c') => self.cancel_run(),
                KeyCode::Esc => {
                    if self.run_tab == RunTab::Config && !self.config_part_query.is_empty() {
                        self.clear_config_part_search();
                    } else if self.exec_thread_done {
                        self.screen = Screen::Quick;
                    }
                }
                KeyCode::Tab | KeyCode::Right => self.select_next_run_tab(),
                KeyCode::Left => self.select_prev_run_tab(),
                KeyCode::Char('1') => self.run_tab = RunTab::Overview,
                KeyCode::Char('2') => self.run_tab = RunTab::Tasks,
                KeyCode::Char('3') => self.run_tab = RunTab::Logs,
                KeyCode::Char('4') => self.run_tab = RunTab::TaskLog,
                KeyCode::Char('5') => self.run_tab = RunTab::Config,
                KeyCode::Char('j') => self.select_next_task(),
                KeyCode::Char('k') => self.select_prev_task(),
                KeyCode::Char('[') | KeyCode::Char('h') => {
                    if self.run_tab == RunTab::Config {
                        self.select_prev_config_part();
                    }
                }
                KeyCode::Char(']') | KeyCode::Char('l') => {
                    if self.run_tab == RunTab::Config {
                        self.select_next_config_part();
                    }
                }
                KeyCode::Char('/') => {
                    if self.run_tab == RunTab::Config {
                        self.begin_config_part_search();
                    }
                }
                KeyCode::Char('n') => {
                    if self.run_tab == RunTab::Config {
                        self.select_next_config_part();
                    }
                }
                KeyCode::Char('N') => {
                    if self.run_tab == RunTab::Config {
                        self.select_prev_config_part();
                    }
                }
                KeyCode::Up => self.scroll_active_view(1),
                KeyCode::Down => self.scroll_active_view(-1),
                KeyCode::PageUp => self.scroll_active_view(10),
                KeyCode::PageDown => self.scroll_active_view(-10),
                KeyCode::Home => self.home_active_view(),
                KeyCode::End => self.end_active_view(),
                _ => {}
            },
        }
        Ok(false)
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let size = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(size);

        self.draw_header(f, chunks[0]);
        self.draw_main(f, chunks[1]);
        self.draw_footer(f, chunks[2]);

        self.draw_modal(f);
    }

    fn draw_modal(&self, f: &mut ratatui::Frame) {
        match &self.input {
            InputMode::Normal => {}
            InputMode::EditValue {
                full_path,
                kind,
                buffer,
                error,
            } => {
                let area = centered_rect(80, 30, f.area());
                let shadow = shadow_rect(area, f.area());
                f.render_widget(
                    Fill {
                        style: Style::default()
                            .bg(Color::Black)
                            .add_modifier(Modifier::DIM),
                    },
                    shadow,
                );
                f.render_widget(Clear, area);

                let kind_s = match kind {
                    ValueKind::Bool => "bool",
                    ValueKind::Int => "int",
                    ValueKind::Float => "float",
                    ValueKind::String => "string",
                };
                let mut text = Vec::new();
                text.push(Line::from(vec![
                    Span::styled("Edit: ", Style::default().fg(Color::Yellow)),
                    Span::raw(full_path.clone()),
                ]));
                text.push(Line::from(format!(
                    "type: {kind_s}  enter=save  esc=cancel"
                )));
                if let Some(e) = error {
                    text.push(Line::from(Span::styled(
                        format!("error: {e}"),
                        Style::default().fg(Color::Red),
                    )));
                }
                text.push(Line::from(""));
                text.push(Line::from(buffer.clone()));

                let p = Paragraph::new(Text::from(text))
                    .style(Style::default().fg(Color::White).bg(Color::DarkGray))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title("Edit Value")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Double),
                    );
                f.render_widget(p, area);
            }
            InputMode::SelectInputChoice {
                key,
                options,
                selected,
            } => {
                let area = centered_rect(75, 60, f.area());
                let shadow = shadow_rect(area, f.area());
                f.render_widget(
                    Fill {
                        style: Style::default()
                            .bg(Color::Black)
                            .add_modifier(Modifier::DIM),
                    },
                    shadow,
                );
                f.render_widget(Clear, area);

                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(3)])
                    .split(area);
                let header = Paragraph::new(Text::from(vec![
                    Line::from(vec![
                        Span::styled("Select input: ", Style::default().fg(Color::Yellow)),
                        Span::raw(key.clone()),
                    ]),
                    Line::from("j/k or arrows to move, enter/space to select, esc to cancel"),
                ]))
                .style(Style::default().fg(Color::White).bg(Color::DarkGray))
                .block(
                    Block::default()
                        .title("Input Selector")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Double),
                );
                f.render_widget(header, rows[0]);

                let items = options
                    .iter()
                    .map(|v| ListItem::new(short_value(v)))
                    .collect::<Vec<_>>();
                let list = List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded),
                    )
                    .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
                    .highlight_symbol("> ");
                let mut state = ListState::default();
                if !options.is_empty() {
                    state.select(Some(*selected.min(&options.len().saturating_sub(1))));
                }
                f.render_stateful_widget(list, rows[1], &mut state);
            }
            InputMode::EditGlobalInt {
                name,
                buffer,
                error,
            } => {
                let area = centered_rect(70, 25, f.area());
                let shadow = shadow_rect(area, f.area());
                f.render_widget(
                    Fill {
                        style: Style::default()
                            .bg(Color::Black)
                            .add_modifier(Modifier::DIM),
                    },
                    shadow,
                );
                f.render_widget(Clear, area);

                let mut text = Vec::new();
                text.push(Line::from(vec![
                    Span::styled("Setting: ", Style::default().fg(Color::Yellow)),
                    Span::raw(name.clone()),
                ]));
                text.push(Line::from("enter=save  esc=cancel"));
                if let Some(e) = error {
                    text.push(Line::from(Span::styled(
                        format!("error: {e}"),
                        Style::default().fg(Color::Red),
                    )));
                }
                text.push(Line::from(""));
                text.push(Line::from(buffer.clone()));

                let p = Paragraph::new(Text::from(text))
                    .style(Style::default().fg(Color::White).bg(Color::DarkGray))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title("Edit Setting")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Double),
                    );
                f.render_widget(p, area);
            }
            InputMode::EditGlobalFloat {
                name,
                buffer,
                error,
            } => {
                let area = centered_rect(70, 25, f.area());
                let shadow = shadow_rect(area, f.area());
                f.render_widget(
                    Fill {
                        style: Style::default()
                            .bg(Color::Black)
                            .add_modifier(Modifier::DIM),
                    },
                    shadow,
                );
                f.render_widget(Clear, area);

                let mut text = Vec::new();
                text.push(Line::from(vec![
                    Span::styled("Setting: ", Style::default().fg(Color::Yellow)),
                    Span::raw(name.clone()),
                ]));
                text.push(Line::from("enter=save  esc=cancel"));
                if let Some(e) = error {
                    text.push(Line::from(Span::styled(
                        format!("error: {e}"),
                        Style::default().fg(Color::Red),
                    )));
                }
                text.push(Line::from(""));
                text.push(Line::from(buffer.clone()));

                let p = Paragraph::new(Text::from(text))
                    .style(Style::default().fg(Color::White).bg(Color::DarkGray))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title("Edit Setting")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Double),
                    );
                f.render_widget(p, area);
            }
            InputMode::SearchConfigParts { buffer } => {
                let area = centered_rect(70, 25, f.area());
                let shadow = shadow_rect(area, f.area());
                f.render_widget(
                    Fill {
                        style: Style::default()
                            .bg(Color::Black)
                            .add_modifier(Modifier::DIM),
                    },
                    shadow,
                );
                f.render_widget(Clear, area);

                let text = vec![
                    Line::from("Filter Config Parts (case-insensitive)"),
                    Line::from("enter=apply  esc=cancel"),
                    Line::from(""),
                    Line::from(buffer.clone()),
                ];
                let p = Paragraph::new(Text::from(text))
                    .style(Style::default().fg(Color::White).bg(Color::DarkGray))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title("Config Search")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Double),
                    );
                f.render_widget(p, area);
            }
            InputMode::ConfirmCancelRun => {
                let area = centered_rect(70, 25, f.area());
                let shadow = shadow_rect(area, f.area());
                f.render_widget(
                    Fill {
                        style: Style::default()
                            .bg(Color::Black)
                            .add_modifier(Modifier::DIM),
                    },
                    shadow,
                );
                f.render_widget(Clear, area);

                let text = vec![
                    Line::from(Span::styled(
                        "Force quit build?",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from("Enter/y: force quit (SIGKILL subprocess groups) and exit"),
                    Line::from("Esc/n: keep running"),
                    Line::from("Ctrl+C: force quit immediately"),
                ];
                let p = Paragraph::new(Text::from(text))
                    .style(Style::default().fg(Color::White).bg(Color::DarkGray))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title("Confirm")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Double),
                    );
                f.render_widget(p, area);
            }
        }
    }

    fn draw_header(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let title = match self.screen {
            Screen::Picker => "Gaia Builder: Pick Build",
            Screen::Quick => "Gaia Builder: Quick",
            Screen::Modules => "Gaia Builder: Modules",
            Screen::Keys => "Gaia Builder: Module Keys",
            Screen::Inputs => "Gaia Builder: Inputs",
            Screen::Run => "Gaia Builder: Run",
        };
        let build = self
            .selected_build
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".into());
        let crumb = match self.screen {
            Screen::Picker => "pick".to_string(),
            Screen::Quick => "quick".to_string(),
            Screen::Modules => "modules".to_string(),
            Screen::Keys => format!("modules > {}", self.selected_module().unwrap_or("<none>")),
            Screen::Inputs => "inputs".to_string(),
            Screen::Run => "run".to_string(),
        };
        let line = Line::from(vec![
            Span::styled(title, Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled(build, Style::default().fg(Color::Gray)),
            Span::raw("  "),
            Span::styled(crumb, Style::default().fg(Color::LightBlue)),
            Span::raw("  "),
            Span::styled(now, Style::default().fg(Color::Yellow)),
        ]);
        let p = Paragraph::new(Text::from(line)).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_type(BorderType::Plain),
        );
        f.render_widget(p, area);
    }

    fn draw_footer(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let hint = match self.screen {
            Screen::Picker => "[j/k] Move  [Enter] Select  [r] Rescan  [q] Quit",
            Screen::Quick => {
                "[j/k] Menu  [[/]] Checkpoint  [u] UsePolicy  [p] UploadPolicy  [R] Refresh  [Enter] Select  [s/r] Start  [i] Inputs  [m] Modules  [Esc/q] Back"
            }
            Screen::Modules => {
                "[j/k] Move  [Enter] Keys  [Space/e] Toggle module.enabled  [r] Run  [Esc/q] Back"
            }
            Screen::Keys => "[j/k] Move  [Space] Toggle Bool  [Enter/e] Edit Value  [Esc/q] Back",
            Screen::Inputs => {
                "[j/k] Move  [Space] Toggle/Cycle  [h/l] Prev/Next Choice  [Enter/e] Selector/Edit  [d] Reset One  [D] Reset All  [r] Run  [Esc/q] Back"
            }
            Screen::Run => {
                "[Left/Right/Tab] Tabs  [j/k] Select Task  [[/]/h/l] Config Part  [/] Search  [n/N] Next/Prev Match  [Up/Down PgUp/PgDn] Scroll  [c] Cancel  [Esc] Clear Search/Back when done  [q] Quit when done"
            }
        };
        let p = Paragraph::new(hint)
            .style(Style::default().fg(Color::Gray))
            .block(Block::default().borders(Borders::TOP));
        f.render_widget(p, area);
    }

    fn draw_main(&mut self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        match self.screen {
            Screen::Picker => self.draw_picker(f, area),
            Screen::Quick => self.draw_quick(f, area),
            Screen::Modules => self.draw_modules(f, area),
            Screen::Keys => self.draw_keys(f, area),
            Screen::Inputs => self.draw_inputs(f, area),
            Screen::Run => self.draw_run(f, area),
        }
    }

    fn draw_quick(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);

        let clean = match self.effective_workspace_clean() {
            CleanMode::None => "none",
            CleanMode::Build => "build",
            CleanMode::Out => "out",
            CleanMode::All => "all",
        };
        let top_level_load = self
            .effective_buildroot_top_level_load()
            .map(|v| format!("{v:.1}"))
            .unwrap_or_else(|| "<unset>".into());
        let perf_profile = self.effective_buildroot_performance_profile();
        let selected_checkpoint = self.selected_checkpoint();

        let items: Vec<ListItem> = self
            .quick_items
            .iter()
            .map(|it| {
                let s = match it {
                    QuickItem::Start => "Start build".to_string(),
                    QuickItem::DryRun => format!("Dry run: {}", self.dry_run),
                    QuickItem::MaxParallel => format!("Max parallel: {}", self.max_parallel),
                    QuickItem::BuildrootPerfProfile => {
                        format!("Buildroot perf profile: {perf_profile}")
                    }
                    QuickItem::TopLevelLoad => {
                        format!("Buildroot top-level load: {top_level_load}")
                    }
                    QuickItem::Inputs => format!("Inputs ({})", self.input_keys.len()),
                    QuickItem::RefreshCheckpointStatus => "Refresh checkpoint status".to_string(),
                    QuickItem::RetryCheckpointUploads => "Retry checkpoint uploads".to_string(),
                    QuickItem::CheckpointUsePolicy => format!(
                        "Selected checkpoint use_policy: {}",
                        selected_checkpoint
                            .map(|s| format!("{:?}", s.use_policy).to_lowercase())
                            .unwrap_or_else(|| "-".to_string())
                    ),
                    QuickItem::CheckpointUploadPolicy => format!(
                        "Selected checkpoint upload_policy: {}",
                        selected_checkpoint
                            .map(|s| format!("{:?}", s.upload_policy).to_lowercase())
                            .unwrap_or_else(|| "-".to_string())
                    ),
                    QuickItem::Clean => format!("Clean: {clean}"),
                    QuickItem::Modules => "Modules".to_string(),
                    QuickItem::Back => "Back".to_string(),
                };
                ListItem::new(s)
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title("Quick Menu")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
            .highlight_symbol("> ");
        let mut state = self.quick_list.clone();
        f.render_stateful_widget(list, cols[0], &mut state);

        let right_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(11), Constraint::Min(8)])
            .split(cols[1]);

        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("Build: ", Style::default().fg(Color::Yellow)),
            Span::raw(
                self.selected_build
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<none>".into()),
            ),
        ]));
        lines.push(Line::from(format!(
            "modules detected: {}",
            self.enabled_modules.len()
        )));
        lines.push(Line::from(format!("tasks in plan: {}", self.exec_total)));
        lines.push(Line::from(format!(
            "buildroot.performance_profile: {}",
            self.effective_buildroot_performance_profile()
        )));
        lines.push(Line::from(format!(
            "buildroot.top_level_load: {}",
            self.effective_buildroot_top_level_load()
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "<unset>".into())
        )));
        lines.push(Line::from(format!(
            "inputs configured: {}",
            self.input_keys.len()
        )));
        let use_count = self.checkpoint_status.iter().filter(|s| s.will_use).count();
        let rebuild_count = self
            .checkpoint_status
            .iter()
            .filter(|s| s.will_rebuild)
            .count();
        let pending_count = self
            .checkpoint_status
            .iter()
            .filter(|s| s.pending_upload)
            .count();
        lines.push(Line::from(format!(
            "checkpoints: total={} use={} rebuild={} pending_upload={}",
            self.checkpoint_status.len(),
            use_count,
            rebuild_count,
            pending_count
        )));
        lines.push(Line::from(
            "Start build: Enter on 'Start build' or press 's'.",
        ));
        lines.push(Line::from(
            "Input controls: quick menu > Inputs or press 'i'.",
        ));
        lines.push(Line::from("Checkpoint controls: [/], u, p, R."));
        let p = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("Summary")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );
        f.render_widget(p, right_rows[0]);

        let ck_items: Vec<ListItem> = if self.checkpoint_status.is_empty() {
            vec![ListItem::new("no checkpoints configured")]
        } else {
            self.checkpoint_status
                .iter()
                .map(|s| {
                    let remote = s
                        .remote_exists
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let line = format!(
                        "id={} local={} remote={} dl={} use={:?} upload={:?} reason={}",
                        s.id,
                        s.exists,
                        remote,
                        s.will_download,
                        s.use_policy,
                        s.upload_policy,
                        s.reason
                    );
                    ListItem::new(line)
                })
                .collect()
        };
        let mut ck_state = self.checkpoint_list.clone();
        let ck_list = List::new(ck_items)
            .block(
                Block::default()
                    .title("Checkpoints")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightGreen))
            .highlight_symbol("> ");
        f.render_stateful_widget(ck_list, right_rows[1], &mut ck_state);
    }

    fn draw_picker(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let items: Vec<ListItem> = self
            .builds
            .iter()
            .map(|p| ListItem::new(p.display().to_string()))
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .title("Builds")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
            .highlight_symbol("> ");
        let mut state = self.build_list.clone();
        f.render_stateful_widget(list, area, &mut state);
    }

    fn draw_modules(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Percentage(40)])
            .split(area);

        let items: Vec<ListItem> = self
            .enabled_modules
            .iter()
            .map(|m| {
                let enabled = self
                    .doc_effective
                    .as_ref()
                    .and_then(|d| d.value_path(&format!("{m}.enabled")))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let mark = if enabled { "[x]" } else { "[ ]" };
                ListItem::new(format!("{mark} {m}"))
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .title("Modules")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
            .highlight_symbol("> ");
        let mut state = self.module_list.clone();
        f.render_stateful_widget(list, rows[0], &mut state);

        let detail =
            if let (Some(doc), Some(mid)) = (self.doc_effective.as_ref(), self.selected_module()) {
                let v = doc
                    .value_path(mid)
                    .cloned()
                    .unwrap_or(toml::Value::Table(Default::default()));
                toml::to_string_pretty(&v).unwrap_or_else(|_| format!("{v:?}"))
            } else {
                "No module selected.".into()
            };
        let p = Paragraph::new(detail).wrap(Wrap { trim: false }).block(
            Block::default()
                .title("Module Config Preview")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        f.render_widget(p, rows[1]);
    }

    fn draw_inputs(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Percentage(45)])
            .split(area);

        let items: Vec<ListItem> = if self.input_keys.is_empty() {
            vec![ListItem::new("<no inputs configured>")]
        } else {
            self.input_keys
                .iter()
                .map(|k| {
                    let short = self
                        .doc_effective
                        .as_ref()
                        .and_then(|d| d.value_path(&format!("inputs.resolved.{k}")))
                        .map(short_value)
                        .unwrap_or_else(|| "<unset>".to_string());
                    ListItem::new(format!("{k} = {short}"))
                })
                .collect()
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .title("Inputs")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
            .highlight_symbol("> ");
        let mut state = self.input_key_list.clone();
        if self.input_keys.is_empty() {
            state.select(None);
        }
        f.render_stateful_widget(list, rows[0], &mut state);

        let detail = if let (Some(doc), Some(key)) =
            (self.doc_effective.as_ref(), self.selected_input_key())
        {
            let mut lines = Vec::<Line>::new();
            let choices = self.selected_input_choices();
            lines.push(Line::from(vec![
                Span::styled("key: ", Style::default().fg(Color::Yellow)),
                Span::raw(key.to_string()),
            ]));
            if !choices.is_empty() {
                lines.push(Line::from("control: selector (enter/e), cycle (space/h/l)"));
            } else if self.selected_input_is_bool() {
                lines.push(Line::from("control: toggle (space or enter/e)"));
            } else {
                lines.push(Line::from("control: editor (enter/e)"));
            }
            if let Some(v) = doc.value_path(&format!("inputs.resolved.{key}")) {
                lines.push(Line::from(format!("resolved: {}", short_value(v))));
            }
            if let Some(v) = doc.value_path(&format!("inputs.values.{key}")) {
                lines.push(Line::from(format!("override/value: {}", short_value(v))));
            }
            if let Some(v) = doc.value_path(&format!("inputs.options.{key}.type")) {
                lines.push(Line::from(format!("type: {}", short_value(v))));
            }
            if let Some(v) = doc.value_path(&format!("inputs.options.{key}.default")) {
                lines.push(Line::from(format!("default: {}", short_value(v))));
            }
            if let Some(v) = doc.value_path(&format!("inputs.options.{key}.env")) {
                lines.push(Line::from(format!("env: {}", short_value(v))));
            }
            if let Some(v) = doc.value_path(&format!("inputs.options.{key}.required")) {
                lines.push(Line::from(format!("required: {}", short_value(v))));
            }
            if let Some(v) = doc.value_path(&format!("inputs.options.{key}.choices")) {
                lines.push(Line::from(format!(
                    "choices: {}",
                    toml::to_string(v)
                        .unwrap_or_else(|_| format!("{v:?}"))
                        .trim()
                )));
            }
            if let Some(v) = doc.value_path(&format!("inputs.options.{key}.description")) {
                lines.push(Line::from(""));
                lines.push(Line::from("description:"));
                lines.push(Line::from(short_value(v)));
            }
            Text::from(lines)
        } else {
            Text::from("Select an input.")
        };
        let p = Paragraph::new(detail).wrap(Wrap { trim: false }).block(
            Block::default()
                .title("Input Detail")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        f.render_widget(p, rows[1]);
    }

    fn draw_keys(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Percentage(40)])
            .split(area);

        let items: Vec<ListItem> = self
            .config_keys
            .iter()
            .map(|k| {
                if let (Some(doc), Some(mid)) =
                    (self.doc_effective.as_ref(), self.selected_module())
                {
                    let full = format!("{mid}.{k}");
                    if let Some(v) = doc.value_path(&full) {
                        let short = match v {
                            toml::Value::Boolean(b) => b.to_string(),
                            toml::Value::Integer(i) => i.to_string(),
                            toml::Value::Float(fl) => fl.to_string(),
                            toml::Value::String(s) => {
                                if s.len() > 24 {
                                    format!("\"{}...\"", &s[..24])
                                } else {
                                    format!("\"{s}\"")
                                }
                            }
                            _ => "<complex>".into(),
                        };
                        return ListItem::new(format!("{k} = {short}"));
                    }
                }
                ListItem::new(k.clone())
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .title("Keys (space toggles bool)")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
            .highlight_symbol("> ");
        let mut state = self.config_key_list.clone();
        f.render_stateful_widget(list, rows[0], &mut state);

        let detail = if let (Some(doc), Some(mid), Some(key)) = (
            self.doc_effective.as_ref(),
            self.selected_module(),
            self.selected_config_key(),
        ) {
            let full = format!("{mid}.{key}");
            let v = doc
                .value_path(&full)
                .cloned()
                .unwrap_or(toml::Value::String("<missing>".into()));
            toml::to_string_pretty(&v).unwrap_or_else(|_| format!("{v:?}"))
        } else {
            "Select a key.".into()
        };
        let p = Paragraph::new(detail).wrap(Wrap { trim: false }).block(
            Block::default()
                .title("Value")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        f.render_widget(p, rows[1]);
    }

    fn draw_run(&mut self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        self.sample_system_metrics();

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(area);

        let done = self.exec_done.min(self.exec_total);
        let total = self.exec_total.max(1);
        let pct = (done as f64 / total as f64 * 100.0) as u16;
        let remaining = self.exec_total.saturating_sub(done);
        let elapsed_hms = self
            .exec_started_at
            .map(|start| format_elapsed_hms(start.elapsed()))
            .unwrap_or_else(|| "00:00:00".to_string());

        let label = if let Some(ok) = self.exec_ok {
            if ok {
                format!("Done ({done}/{})  elapsed={elapsed_hms}", self.exec_total)
            } else {
                format!("Failed ({done}/{})  elapsed={elapsed_hms}", self.exec_total)
            }
        } else {
            format!(
                "Running ({done}/{})  remaining={remaining}  elapsed={elapsed_hms}",
                self.exec_total
            )
        };

        let g = Gauge::default()
            .block(
                Block::default()
                    .title("Progress")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .gauge_style(Style::default().fg(Color::Green))
            .percent(pct)
            .label(label);
        f.render_widget(g, rows[0]);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(46), Constraint::Min(0)])
            .split(rows[1]);

        self.draw_run_task_panel(f, cols[0]);

        match self.run_tab {
            RunTab::Overview => self.draw_overview_panel(f, cols[1]),
            RunTab::Config => self.draw_config_explorer(f, cols[1]),
            _ => {
                let main = match self.run_tab {
                    RunTab::Tasks => self.render_tasks_view(cols[1].height as usize),
                    RunTab::Logs => self.render_all_logs(cols[1].height as usize),
                    RunTab::TaskLog => self.render_task_log(cols[1].height as usize),
                    RunTab::Overview | RunTab::Config => unreachable!(),
                };
                let view_title = match self.run_tab {
                    RunTab::Overview => "Overview",
                    RunTab::Tasks => "Task Details",
                    RunTab::Logs => "All Logs",
                    RunTab::TaskLog => "Selected Task Log",
                    RunTab::Config => unreachable!(),
                };
                let p = Paragraph::new(main).wrap(Wrap { trim: false }).block(
                    Block::default()
                        .title(view_title)
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                );
                f.render_widget(p, cols[1]);
            }
        }

        let titles = ["Overview", "Tasks", "Logs", "TaskLog", "Config"]
            .iter()
            .map(|t| Line::from(*t))
            .collect::<Vec<_>>();
        let idx = match self.run_tab {
            RunTab::Overview => 0,
            RunTab::Tasks => 1,
            RunTab::Logs => 2,
            RunTab::TaskLog => 3,
            RunTab::Config => 4,
        };
        let tabs = Tabs::new(titles)
            .select(idx)
            .block(
                Block::default()
                    .title("Tabs [Left/Right]")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow));
        f.render_widget(tabs, rows[2]);
    }

    fn draw_run_task_panel(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let items: Vec<ListItem> = self
            .tasks
            .iter()
            .map(|id| {
                let status = self
                    .task_state
                    .get(id)
                    .map(|s| s.status)
                    .unwrap_or(TaskStatus::Pending);
                let (mark, color) = match status {
                    TaskStatus::Pending => ("PEND", Color::DarkGray),
                    TaskStatus::Running => ("RUN ", Color::Yellow),
                    TaskStatus::Ok => ("OK  ", Color::Green),
                    TaskStatus::Failed => ("FAIL", Color::Red),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("[{mark}] "), Style::default().fg(color)),
                    Span::raw(id.clone()),
                ]))
            })
            .collect();

        let mut state = self.task_list.clone();
        let list = List::new(items)
            .block(
                Block::default()
                    .title("Tasks [j/k selects]")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_symbol(">> ")
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn draw_overview_panel(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(0)])
            .split(area);

        let summary = self.render_overview_summary(rows[0].height as usize);
        let summary_p = Paragraph::new(summary).wrap(Wrap { trim: false }).block(
            Block::default()
                .title("Overview")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        f.render_widget(summary_p, rows[0]);

        let charts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(34),
                Constraint::Percentage(33),
                Constraint::Percentage(33),
            ])
            .split(rows[1]);

        self.draw_percent_line_chart(
            f,
            charts[0],
            "CPU Load %",
            &self.load_history,
            Color::Yellow,
        );
        self.draw_percent_line_chart(f, charts[1], "Memory %", &self.mem_history, Color::Cyan);
        self.draw_percent_line_chart(f, charts[2], "Disk / %", &self.disk_history, Color::Magenta);
    }

    fn render_overview_summary(&self, height: usize) -> Text<'static> {
        let mut lines = Vec::new();
        let (pending, running, ok, failed) = self.task_status_counts();

        lines.push(Line::from(Span::styled(
            "System",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        if let Some(start) = self.exec_started_at {
            lines.push(Line::from(format!(
                "elapsed: {}",
                format_elapsed_hms(start.elapsed())
            )));
        }
        lines.push(Line::from(format!(
            "cpu cores: {}  max_parallel: {}  dry_run: {}",
            self.system_snapshot.cpu_cores, self.max_parallel, self.dry_run
        )));
        if let Some(load) = self.system_snapshot.load_1m {
            let pct = self.system_snapshot.load_pct.unwrap_or(0.0);
            lines.push(Line::from(format!(
                "load(1m): {:.2}  ({:.1}% of cores)",
                load, pct
            )));
        } else {
            lines.push(Line::from("load(1m): n/a"));
        }
        if let (Some(total), Some(used), Some(pct)) = (
            self.system_snapshot.mem_total_kib,
            self.system_snapshot.mem_used_kib,
            self.system_snapshot.mem_pct,
        ) {
            lines.push(Line::from(format!(
                "mem: {} / {} ({:.1}%)",
                format_kib(used),
                format_kib(total),
                pct
            )));
        } else {
            lines.push(Line::from("mem: n/a"));
        }
        if let (Some(total), Some(used), Some(pct)) = (
            self.system_snapshot.disk_total_bytes,
            self.system_snapshot.disk_used_bytes,
            self.system_snapshot.disk_pct,
        ) {
            lines.push(Line::from(format!(
                "disk /: {} / {} ({:.1}%)",
                format_bytes(used),
                format_bytes(total),
                pct
            )));
        } else {
            lines.push(Line::from("disk /: n/a"));
        }

        lines.push(Line::from(Span::styled(
            "Run",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(format!(
            "tasks => pending:{pending} running:{running} ok:{ok} failed:{failed}"
        )));
        if let Some(sel) = self.selected_task() {
            let status = self
                .task_state
                .get(sel)
                .map(|s| match s.status {
                    TaskStatus::Pending => "pending",
                    TaskStatus::Running => "running",
                    TaskStatus::Ok => "ok",
                    TaskStatus::Failed => "failed",
                })
                .unwrap_or("pending");
            lines.push(Line::from(format!("selected: {sel} ({status})")));
        }

        lines.truncate(height.saturating_sub(1));
        Text::from(lines)
    }

    fn draw_percent_line_chart(
        &self,
        f: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        title: &str,
        history: &VecDeque<f64>,
        color: Color,
    ) {
        let raw_points = history_to_points(history);
        let points = densify_points(&raw_points, 4);
        let x_max = points.last().map(|(x, _)| *x).unwrap_or(1.0).max(1.0);
        let dataset = Dataset::default()
            .name(title)
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(color))
            .data(&points);

        let chart = Chart::new(vec![dataset])
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .x_axis(
                Axis::default()
                    .bounds([0.0, x_max])
                    .labels(vec![Span::raw("old"), Span::raw("now")])
                    .style(Style::default().fg(Color::DarkGray)),
            )
            .y_axis(
                Axis::default()
                    .bounds([0.0, 100.0])
                    .labels(vec![Span::raw("0"), Span::raw("50"), Span::raw("100")])
                    .style(Style::default().fg(Color::DarkGray)),
            );
        f.render_widget(chart, area);
    }

    fn render_tasks_view(&self, height: usize) -> Text<'static> {
        let mut lines = Vec::new();
        let Some(id) = self.selected_task() else {
            return Text::from("no task selected");
        };
        let Some(state) = self.task_state.get(id) else {
            return Text::from("task state unavailable");
        };
        let status = match state.status {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Ok => "ok",
            TaskStatus::Failed => "failed",
        };
        lines.push(Line::from(format!("task: {id}")));
        lines.push(Line::from(format!("status: {status}")));
        if let Some(last) = state.last_line.as_ref() {
            lines.push(Line::from("last line:"));
            lines.push(Line::from(last.clone()));
        }
        lines.push(Line::from(""));

        if let Some(logs) = self.task_logs.get(id) {
            lines.push(Line::from("recent log lines:"));
            for s in logs
                .iter()
                .rev()
                .take(height.saturating_sub(lines.len() + 1))
                .rev()
            {
                lines.push(Line::from(s.clone()));
            }
        }
        Text::from(lines)
    }

    fn render_all_logs(&self, height: usize) -> Text<'static> {
        let mut lines = Vec::new();
        let max = self.all_logs.len().saturating_sub(height.saturating_sub(1));
        let scroll = self.all_logs_scroll.min(max);
        let start = self
            .all_logs
            .len()
            .saturating_sub(height.saturating_sub(1).saturating_add(scroll));
        let end = self.all_logs.len().saturating_sub(scroll);
        for s in self
            .all_logs
            .iter()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            lines.push(Line::from(s.clone()));
        }
        Text::from(lines)
    }

    fn render_task_log(&self, height: usize) -> Text<'static> {
        let mut lines = Vec::new();
        let Some(id) = self.selected_task() else {
            return Text::from("no task selected");
        };
        let Some(q) = self.task_logs.get(id) else {
            return Text::from("no logs");
        };
        let max = q.len().saturating_sub(height.saturating_sub(1));
        let scroll = self.task_log_scroll.min(max);
        let start = q
            .len()
            .saturating_sub(height.saturating_sub(1).saturating_add(scroll));
        let end = q.len().saturating_sub(scroll);
        for s in q.iter().skip(start).take(end.saturating_sub(start)) {
            lines.push(Line::from(s.clone()));
        }
        Text::from(lines)
    }

    fn draw_config_explorer(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(36), Constraint::Min(0)])
            .split(area);

        let items: Vec<ListItem> = if self.config_part_matches.is_empty() {
            vec![ListItem::new("<no matches>")]
        } else {
            self.config_part_matches
                .iter()
                .filter_map(|idx| self.config_parts.get(*idx))
                .map(|p| {
                    let label = if p == "all" {
                        "all".to_string()
                    } else {
                        p.clone()
                    };
                    ListItem::new(label)
                })
                .collect()
        };
        let mut state = self.config_part_list.clone();
        if self.config_part_matches.is_empty() {
            state.select(None);
        }
        let list_title = if self.config_part_query.is_empty() {
            "Config Parts [[/]/h/l  / search]".to_string()
        } else {
            format!(
                "Config Parts [{} match{}] query='{}'",
                self.config_part_matches.len(),
                if self.config_part_matches.len() == 1 {
                    ""
                } else {
                    "es"
                },
                self.config_part_query
            )
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .title(list_title)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_symbol(">> ")
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_stateful_widget(list, cols[0], &mut state);

        let content = self.render_selected_config_part(cols[1].height as usize);
        let title = format!(
            "Config: {}",
            self.selected_config_part().unwrap_or("<no match>")
        );
        let panel = Paragraph::new(content).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        f.render_widget(panel, cols[1]);
    }

    fn render_selected_config_part(&self, height: usize) -> Text<'static> {
        let Some(doc) = self.doc_effective.as_ref() else {
            return Text::from("no config loaded");
        };
        let Some(section) = self.selected_config_part() else {
            if self.config_part_query.is_empty() {
                return Text::from("no config section selected");
            }
            return Text::from(format!(
                "no config section matches '{}'",
                self.config_part_query
            ));
        };
        let value = if section == "all" {
            doc.value.clone()
        } else {
            doc.value_path(section)
                .cloned()
                .unwrap_or_else(|| toml::Value::String("<missing section>".into()))
        };

        let s = toml::to_string_pretty(&value).unwrap_or_else(|_| format!("{:?}", value));
        let raw_lines = s.lines().collect::<Vec<_>>();
        let visible = height.saturating_sub(1);
        let max_start = raw_lines.len().saturating_sub(visible);
        let start = self.config_scroll.min(max_start);
        let end = (start + visible).min(raw_lines.len());
        let mut lines = Vec::new();
        for l in raw_lines.iter().skip(start).take(end.saturating_sub(start)) {
            lines.push(Line::from((*l).to_string()));
        }
        Text::from(lines)
    }

    fn task_status_counts(&self) -> (usize, usize, usize, usize) {
        let mut pending = 0usize;
        let mut running = 0usize;
        let mut ok = 0usize;
        let mut failed = 0usize;
        for state in self.task_state.values() {
            match state.status {
                TaskStatus::Pending => pending += 1,
                TaskStatus::Running => running += 1,
                TaskStatus::Ok => ok += 1,
                TaskStatus::Failed => failed += 1,
            }
        }
        (pending, running, ok, failed)
    }

    fn final_console_summary(&self) -> Option<String> {
        if self.exec_started_at.is_none() && self.task_error_logs.is_empty() {
            return None;
        }

        let (pending, running, ok, failed) = self.task_status_counts();
        let status = if self.exec_thread_done {
            match self.exec_ok {
                Some(true) => "ok",
                Some(false) => "failed",
                None => "unknown",
            }
        } else if self.force_exit_code.is_some() {
            "interrupted"
        } else {
            "running"
        };

        let elapsed_hms = self
            .exec_started_at
            .map(|t| format_elapsed_hms(t.elapsed()))
            .unwrap_or_else(|| "00:00:00".to_string());

        let mut lines = Vec::new();
        lines.push("SUMMARY:".to_string());
        if let Some(path) = self.selected_build.as_ref() {
            lines.push(format!("  build: {}", path.display()));
        }
        lines.push(format!("  status: {status}"));
        lines.push(format!(
            "  tasks: total={} ok={} failed={} pending={} running={}",
            self.exec_total, ok, failed, pending, running
        ));
        lines.push(format!("  elapsed: {elapsed_hms}"));

        if failed > 0 {
            let mut failed_ids = self
                .task_state
                .iter()
                .filter_map(|(id, st)| (st.status == TaskStatus::Failed).then_some(id.clone()))
                .collect::<Vec<_>>();
            failed_ids.sort();
            if !failed_ids.is_empty() {
                lines.push(format!("  failed_tasks: {}", failed_ids.join(", ")));
            }
        }
        if !self.task_error_logs.is_empty() {
            lines.push("  error_logs:".to_string());
            for p in &self.task_error_logs {
                lines.push(format!("    {}", p.display()));
            }
        }

        Some(lines.join("\n"))
    }
}

pub fn run_tui(builds_dir: &Path, max_parallel: usize) -> Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| Error::msg(e.to_string()))?;
    execute!(stdout, EnterAlternateScreen, Hide).map_err(|e| Error::msg(e.to_string()))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| Error::msg(e.to_string()))?;
    terminal
        .clear()
        .map_err(|e| Error::msg(format!("tui clear failed: {e}")))?;

    let result = run_loop(
        &mut terminal,
        App::new(builds_dir.to_path_buf(), max_parallel)?,
    );

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen, Show).ok();

    let (exit_code, summary) = result?;

    if let Some(summary) = summary {
        println!("{summary}");
    }

    match exit_code {
        Some(code) => std::process::exit(code),
        None => Ok(()),
    }
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
) -> Result<(Option<i32>, Option<String>)> {
    // Ensure normal CLI runs still use stdout logging; keep this to avoid unused imports.
    let _ = StdoutSink::default();

    let tick = Duration::from_millis(100);
    loop {
        app.drain_exec_events();
        let mut draw_panicked = false;
        let draw_result = terminal.draw(|f| {
            if catch_unwind(AssertUnwindSafe(|| app.draw(f))).is_err() {
                draw_panicked = true;
            }
        });
        if draw_panicked {
            app.repair_log_buffers("draw panic");
            let _ = terminal.clear();
            continue;
        }
        if let Err(e) = draw_result {
            app.repair_log_buffers(&format!("draw error: {e}"));
            let _ = terminal.clear();
            continue;
        }

        if app.force_exit_code.is_some() {
            let summary = app.final_console_summary();
            return Ok((app.force_exit_code, summary));
        }

        if let Some(at) = app.exit_at {
            if Instant::now() >= at {
                break;
            }
        }

        // Poll for events so we can keep updating progress/logs.
        if event::poll(tick).map_err(|e| Error::msg(e.to_string()))? {
            match event::read().map_err(|e| Error::msg(e.to_string()))? {
                Event::Key(k) => {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    if app.handle_key(k.code, k.modifiers)? {
                        break;
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        // Basic tick to avoid unused state (reserved for future animations).
        app.exec_last_tick = Instant::now();
    }
    let summary = app.final_console_summary();
    Ok((None, summary))
}

fn short_value(v: &toml::Value) -> String {
    match v {
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => format!("{f}"),
        toml::Value::String(s) => s.clone(),
        toml::Value::Array(arr) => arr.iter().map(short_value).collect::<Vec<_>>().join(", "),
        _ => toml::to_string(v)
            .unwrap_or_else(|_| format!("{v:?}"))
            .trim()
            .to_string(),
    }
}

fn empty_overrides_table() -> toml::Value {
    toml::Value::Table(Default::default())
}

fn build_tui_overrides_path(build_path: &Path) -> PathBuf {
    let parent = build_path.parent().unwrap_or_else(|| Path::new("."));
    let build_name = build_path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("build.toml");
    parent
        .join(".gaia")
        .join(format!("{build_name}.tui-overrides.toml"))
}

fn read_tui_overrides(path: &Path) -> Result<toml::Value> {
    if !path.exists() {
        return Ok(empty_overrides_table());
    }

    let raw = fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("failed to read {}: {e}", path.display())))?;
    if raw.trim().is_empty() {
        return Ok(empty_overrides_table());
    }

    let value: toml::Value = toml::from_str(&raw)
        .map_err(|e| Error::msg(format!("failed to parse {}: {e}", path.display())))?;
    if !value.is_table() {
        return Err(Error::msg(format!(
            "invalid tui overrides {}: expected table at root",
            path.display()
        )));
    }
    Ok(value)
}

fn write_tui_overrides(path: &Path, overrides: &toml::Value) -> Result<()> {
    if !overrides.is_table() {
        return Err(Error::msg("invalid overrides: root value must be a table"));
    }

    let parent = path.parent().ok_or_else(|| {
        Error::msg(format!(
            "invalid overrides path (no parent): {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;

    let body = toml::to_string_pretty(overrides)
        .map_err(|e| Error::msg(format!("failed to encode tui overrides: {e}")))?;
    fs::write(path, body)
        .map_err(|e| Error::msg(format!("failed to write {}: {e}", path.display())))
}

fn is_primitive_input_value(v: &toml::Value) -> bool {
    matches!(
        v,
        toml::Value::String(_)
            | toml::Value::Boolean(_)
            | toml::Value::Integer(_)
            | toml::Value::Float(_)
    )
}

fn toml_values_equal_loose(a: &toml::Value, b: &toml::Value) -> bool {
    match (a, b) {
        (toml::Value::Integer(x), toml::Value::Float(y)) => (*x as f64) == *y,
        (toml::Value::Float(x), toml::Value::Integer(y)) => *x == (*y as f64),
        _ => a == b,
    }
}

fn remove_override_path_from_table(tbl: &mut toml::value::Table, segs: &[&str]) -> bool {
    if segs.is_empty() {
        return tbl.is_empty();
    }
    if segs.len() == 1 {
        tbl.remove(segs[0]);
        return tbl.is_empty();
    }

    let key = segs[0];
    let remove_parent = tbl
        .get_mut(key)
        .and_then(toml::Value::as_table_mut)
        .is_some_and(|child| remove_override_path_from_table(child, &segs[1..]));
    if remove_parent {
        tbl.remove(key);
    }
    tbl.is_empty()
}

fn find_tomls(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let entries = match fs::read_dir(&p) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                if path.file_name().and_then(|s| s.to_str()) == Some(".gaia") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.ends_with(".tui-overrides.toml") {
                    continue;
                }
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn push_bounded(q: &mut VecDeque<String>, v: String, max: usize) {
    if max == 0 {
        return;
    }
    while q.len() >= max {
        q.pop_front();
    }
    q.push_back(v);
}

fn push_bounded_f64(q: &mut VecDeque<f64>, v: f64, max: usize) {
    if max == 0 {
        return;
    }
    while q.len() >= max {
        q.pop_front();
    }
    q.push_back(v);
}

fn add_signed_saturating(base: usize, delta: isize) -> usize {
    if delta >= 0 {
        base.saturating_add(delta as usize)
    } else {
        base.saturating_sub(delta.unsigned_abs())
    }
}

fn read_loadavg_1m() -> Option<f64> {
    let s = fs::read_to_string("/proc/loadavg").ok()?;
    let first = s.split_whitespace().next()?;
    first.parse::<f64>().ok()
}

fn read_mem_usage_kib() -> Option<(u64, u64)> {
    let s = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total_kib = None;
    let mut avail_kib = None;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kib = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok());
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kib = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok());
        }
    }
    let total = total_kib?;
    let avail = avail_kib?;
    Some((total, total.saturating_sub(avail)))
}

#[cfg(unix)]
fn read_root_disk_usage_bytes() -> Option<(u64, u64)> {
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let root = b"/\0";
    let rc = unsafe { libc::statvfs(root.as_ptr().cast(), &mut st) };
    if rc != 0 {
        return None;
    }

    let frsize = st.f_frsize as u128;
    let blocks = st.f_blocks as u128;
    let bavail = st.f_bavail as u128;

    let total = blocks.saturating_mul(frsize);
    let avail = bavail.saturating_mul(frsize);
    let used = total.saturating_sub(avail);

    let total_u64 = total.min(u64::MAX as u128) as u64;
    let used_u64 = used.min(u64::MAX as u128) as u64;
    Some((total_u64, used_u64))
}

#[cfg(not(unix))]
fn read_root_disk_usage_bytes() -> Option<(u64, u64)> {
    None
}

fn format_kib(kib: u64) -> String {
    let mib = kib as f64 / 1024.0;
    if mib >= 1024.0 {
        format!("{:.2} GiB", mib / 1024.0)
    } else {
        format!("{:.1} MiB", mib)
    }
}

fn format_bytes(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gib >= 1024.0 {
        format!("{:.2} TiB", gib / 1024.0)
    } else {
        format!("{:.2} GiB", gib)
    }
}

fn format_elapsed_hms(elapsed: Duration) -> String {
    let total_secs = elapsed.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn history_to_points(history: &VecDeque<f64>) -> Vec<(f64, f64)> {
    if history.is_empty() {
        return vec![(0.0, 0.0), (1.0, 0.0)];
    }

    history
        .iter()
        .enumerate()
        .map(|(idx, v)| (idx as f64, (*v).clamp(0.0, 100.0)))
        .collect()
}

fn densify_points(points: &[(f64, f64)], segments_per_step: usize) -> Vec<(f64, f64)> {
    if points.len() <= 1 || segments_per_step <= 1 {
        return points.to_vec();
    }

    let mut out = Vec::with_capacity(points.len() * segments_per_step);
    for w in points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        out.push((x0, y0));
        for i in 1..segments_per_step {
            let t = i as f64 / segments_per_step as f64;
            out.push((x0 + (x1 - x0) * t, y0 + (y1 - y0) * t));
        }
    }
    if let Some(last) = points.last().copied() {
        out.push(last);
    }
    out
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

fn collect_table_paths(prefix: &str, table: &toml::Table, out: &mut Vec<String>) {
    let mut entries = table
        .iter()
        .filter_map(|(k, v)| v.as_table().map(|t| (k.clone(), t)))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (k, t) in entries {
        let path = format!("{prefix}.{k}");
        out.push(path.clone());
        collect_table_paths(&path, t, out);
    }
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    let vertical = popup_layout[1];
    let popup_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical);
    popup_layout[1]
}

fn shadow_rect(
    inner: ratatui::layout::Rect,
    bounds: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let max_x = bounds.x.saturating_add(bounds.width);
    let max_y = bounds.y.saturating_add(bounds.height);
    let x = inner.x.saturating_add(1).min(max_x.saturating_sub(1));
    let y = inner.y.saturating_add(1).min(max_y.saturating_sub(1));
    let w = inner.width.min(max_x.saturating_sub(x));
    let h = inner.height.min(max_y.saturating_sub(y));
    ratatui::layout::Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

struct Fill {
    style: Style,
}

impl Widget for Fill {
    fn render(self, area: ratatui::layout::Rect, buf: &mut Buffer) {
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                buf[(x, y)].set_char(' ').set_style(self.style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn tui_override_path_is_under_dot_gaia() {
        let p = PathBuf::from("/tmp/image/build.toml");
        let out = build_tui_overrides_path(&p);
        assert_eq!(
            out,
            PathBuf::from("/tmp/image/.gaia/build.toml.tui-overrides.toml")
        );
    }

    #[test]
    fn read_missing_tui_overrides_returns_empty_table() {
        let tmp = tempdir().expect("tmp");
        let p = tmp.path().join("missing.toml");
        let v = read_tui_overrides(&p).expect("read");
        assert!(v.as_table().is_some());
        assert!(v.as_table().is_some_and(|t| t.is_empty()));
    }

    #[test]
    fn write_and_read_tui_overrides_round_trip() {
        let tmp = tempdir().expect("tmp");
        let p = tmp.path().join(".gaia/build.toml.tui-overrides.toml");
        let mut table = toml::Table::new();
        table.insert("dry_run".into(), toml::Value::Boolean(true));
        let mut inputs = toml::Table::new();
        inputs.insert("value".into(), toml::Value::String("release".into()));
        table.insert("inputs".into(), toml::Value::Table(inputs));
        let v = toml::Value::Table(table);

        write_tui_overrides(&p, &v).expect("write");
        let loaded = read_tui_overrides(&p).expect("read");
        assert_eq!(loaded, v);
    }

    #[test]
    fn find_tomls_skips_tui_override_state_files() {
        let tmp = tempdir().expect("tmp");
        let build = tmp.path().join("build.toml");
        fs::write(&build, "imports = []\n").expect("write build");

        let state_dir = tmp.path().join(".gaia");
        fs::create_dir_all(&state_dir).expect("mkdir .gaia");
        let state = state_dir.join("build.toml.tui-overrides.toml");
        fs::write(&state, "[inputs]\n").expect("write state");

        let list = find_tomls(tmp.path()).expect("scan");
        assert!(list.contains(&build), "missing build.toml from scan");
        assert!(
            !list.contains(&state),
            "tui override state file should be excluded"
        );
    }
}
