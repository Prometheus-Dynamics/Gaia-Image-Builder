use super::*;

impl<'a> TuiState<'a> {
    pub(crate) fn activate_setup_item(&mut self) {
        match self.selected_setup_item() {
            SetupItem::StartBuild => self.start_run(),
            SetupItem::Branch => {
                self.begin_edit(SetupEditField::Branch, self.current_branch_value())
            }
            SetupItem::Target => self.cycle_target(1),
            SetupItem::Profile => self.cycle_profile(1),
            SetupItem::Input(name) => self.activate_input(&name, 1),
            SetupItem::Jobs => self.begin_edit(SetupEditField::Jobs, self.current_jobs_value()),
            SetupItem::PickBuild => self.screen = Screen::Picker,
            SetupItem::Refresh => self.refresh(),
            _ => {}
        }
    }

    pub(crate) fn selected_setup_item(&self) -> SetupItem {
        self.setup_items
            .get(self.setup_list.selected().unwrap_or(0))
            .cloned()
            .unwrap_or(SetupItem::StartBuild)
    }

    pub(crate) fn selected_monitor_view(&self) -> MonitorView {
        MonitorView::all()[self.monitor_view]
    }

    pub(crate) fn next_setup_detail(&mut self) {
        match self.selected_setup_item() {
            SetupItem::Branch => self.cycle_branch_mode(1),
            SetupItem::Target => self.cycle_target(1),
            SetupItem::Profile => self.cycle_profile(1),
            SetupItem::Input(name) => self.activate_input(&name, 1),
            SetupItem::Jobs => self.cycle_jobs(1),
            SetupItem::Overview => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Selection))),
            SetupItem::Selection => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Validation))),
            SetupItem::Validation => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Plan))),
            SetupItem::Plan => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Reports))),
            SetupItem::Reports => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Spec))),
            SetupItem::Spec => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Overview))),
            _ => {}
        }
        self.detail_scroll = 0;
    }

    pub(crate) fn prev_setup_detail(&mut self) {
        match self.selected_setup_item() {
            SetupItem::Branch => self.cycle_branch_mode(-1),
            SetupItem::Target => self.cycle_target(-1),
            SetupItem::Profile => self.cycle_profile(-1),
            SetupItem::Input(name) => self.activate_input(&name, -1),
            SetupItem::Jobs => self.cycle_jobs(-1),
            SetupItem::Overview => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Spec))),
            SetupItem::Selection => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Overview))),
            SetupItem::Validation => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Selection))),
            SetupItem::Plan => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Validation))),
            SetupItem::Reports => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Plan))),
            SetupItem::Spec => self
                .setup_list
                .select(Some(self.index_of_setup_item(&SetupItem::Reports))),
            _ => {}
        }
        self.detail_scroll = 0;
    }

    pub(crate) fn next_monitor_view(&mut self) {
        self.monitor_view = (self.monitor_view + 1) % MonitorView::all().len();
        self.detail_scroll = 0;
        self.detail_follow_tail = true;
    }

    pub(crate) fn prev_monitor_view(&mut self) {
        self.monitor_view = if self.monitor_view == 0 {
            MonitorView::all().len() - 1
        } else {
            self.monitor_view - 1
        };
        self.detail_scroll = 0;
        self.detail_follow_tail = true;
    }

    pub(crate) fn move_setup_down(&mut self) {
        let total = self.setup_items.len();
        let current = self.setup_list.selected().unwrap_or(0);
        self.setup_list.select(Some((current + 1).min(total - 1)));
        self.detail_scroll = 0;
    }

    pub(crate) fn move_setup_up(&mut self) {
        let current = self.setup_list.selected().unwrap_or(0);
        self.setup_list.select(Some(current.saturating_sub(1)));
        self.detail_scroll = 0;
    }

    pub(crate) fn begin_edit(&mut self, field: SetupEditField, current: String) {
        self.edit_field = Some(field);
        self.edit_buffer = current;
        self.set_status("editing value");
    }

    pub(crate) fn apply_edit_buffer(&mut self) {
        let Some(field) = self.edit_field.take() else {
            return;
        };
        let value = self.edit_buffer.trim().to_string();
        self.edit_buffer.clear();
        match field {
            SetupEditField::Branch => {
                self.set_or_clear_override("build.branch", &value);
                self.request_refresh(format!("branch set to {}", self.current_branch_value()));
            }
            SetupEditField::Input(name) => {
                self.set_input_override(&name, &value);
                self.request_refresh(format!(
                    "{} set to {}",
                    name,
                    self.current_input_value(&name)
                ));
            }
            SetupEditField::Jobs => {
                if value.parse::<u32>().is_ok() {
                    self.set_or_clear_override("execution.jobs", &value);
                    self.request_refresh(format!("jobs set to {}", self.current_jobs_label()));
                } else {
                    self.set_status("jobs must be a non-negative integer");
                }
            }
        }
    }

    pub(crate) fn set_or_clear_override(&mut self, key: &str, value: &str) {
        if value.is_empty() {
            self.options
                .explicit_overrides
                .retain(|(entry_key, _)| entry_key != key);
        } else if let Some((_, existing)) = self
            .options
            .explicit_overrides
            .iter_mut()
            .find(|(entry_key, _)| entry_key == key)
        {
            *existing = value.to_string();
        } else {
            self.options
                .explicit_overrides
                .push((key.to_string(), value.to_string()));
        }
    }

    pub(crate) fn clear_override(&mut self, key: &str) {
        self.options
            .explicit_overrides
            .retain(|(entry_key, _)| entry_key != key);
    }

    pub(crate) fn current_branch_value(&self) -> String {
        self.options
            .explicit_overrides
            .iter()
            .find(|(key, _)| key == "build.branch")
            .map(|(_, value)| value.clone())
            .or_else(|| {
                self.spec
                    .as_ref()
                    .and_then(|spec| spec.metadata.branch.clone())
            })
            .unwrap_or_default()
    }

    pub(crate) fn current_target_value(&self) -> String {
        self.options
            .explicit_overrides
            .iter()
            .find(|(key, _)| key == "input.target")
            .map(|(_, value)| value.clone())
            .or_else(|| {
                self.spec.as_ref().and_then(|spec| {
                    spec.selection
                        .selected_inputs
                        .iter()
                        .find(|(name, _)| name == "target")
                        .map(|(_, value)| value.clone())
                })
            })
            .or_else(|| {
                self.spec
                    .as_ref()
                    .and_then(|spec| spec.metadata.target.clone())
            })
            .unwrap_or_default()
    }

    pub(crate) fn current_profile_value(&self) -> String {
        self.options
            .explicit_overrides
            .iter()
            .find(|(key, _)| key == "input.profile")
            .map(|(_, value)| value.clone())
            .or_else(|| {
                self.spec.as_ref().and_then(|spec| {
                    spec.selection
                        .selected_inputs
                        .iter()
                        .find(|(name, _)| name == "profile")
                        .map(|(_, value)| value.clone())
                })
            })
            .or_else(|| {
                self.spec
                    .as_ref()
                    .and_then(|spec| spec.metadata.profile.clone())
            })
            .unwrap_or_default()
    }

    pub(crate) fn current_input_value(&self, name: &str) -> String {
        let input_key = format!("input.{name}");
        self.options
            .explicit_overrides
            .iter()
            .find(|(key, _)| key == &input_key)
            .map(|(_, value)| value.clone())
            .or_else(|| {
                self.spec.as_ref().and_then(|spec| {
                    spec.selection
                        .selected_inputs
                        .iter()
                        .find(|(selected_name, _)| selected_name == name)
                        .map(|(_, value)| value.clone())
                })
            })
            .or_else(|| {
                self.spec.as_ref().and_then(|spec| {
                    spec.inputs
                        .declared
                        .iter()
                        .find(|input| input.name == name)
                        .and_then(|input| input.default.clone())
                })
            })
            .unwrap_or_default()
    }

    pub(crate) fn current_jobs_value(&self) -> String {
        self.options
            .explicit_overrides
            .iter()
            .find(|(key, _)| key == "execution.jobs" || key == "policy.execution.jobs")
            .map(|(_, value)| value.clone())
            .or_else(|| {
                self.spec
                    .as_ref()
                    .map(|spec| spec.policy.execution.jobs.to_string())
            })
            .unwrap_or_else(|| "0".into())
    }

    pub(crate) fn current_jobs_label(&self) -> String {
        format_jobs_label(self.current_jobs_value().as_str())
    }

    pub(crate) fn current_git_branch(&self) -> Option<String> {
        let output = std::process::Command::new("git")
            .arg("branch")
            .arg("--show-current")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!branch.is_empty()).then_some(branch)
    }

    pub(crate) fn cycle_branch_mode(&mut self, _direction: i32) {
        let has_branch_override = self
            .options
            .explicit_overrides
            .iter()
            .any(|(key, _)| key == "build.branch");
        if has_branch_override {
            self.clear_override("build.branch");
            self.request_refresh(format!(
                "branch restored to configured {}",
                self.current_branch_value()
            ));
        } else if let Some(git_branch) = self.current_git_branch() {
            self.set_or_clear_override("build.branch", &git_branch);
            self.request_refresh(format!("branch set to git {}", self.current_branch_value()));
        } else {
            self.set_status("current git branch is unavailable");
        }
    }

    pub(crate) fn cycle_profile(&mut self, direction: i32) {
        let Some(choices) = self.input_choices("profile") else {
            self.set_status("profile input is not declared");
            return;
        };
        if choices.is_empty() {
            self.set_status("profile has no choices to cycle");
            return;
        }
        let current = self.current_profile_value();
        let current_index = choices
            .iter()
            .position(|choice| choice == &current)
            .unwrap_or(0);
        let len = choices.len() as i32;
        let next = (current_index as i32 + direction).rem_euclid(len) as usize;
        self.set_or_clear_override("input.profile", &choices[next]);
        self.set_or_clear_override("build.profile", &choices[next]);
        self.request_refresh(format!("profile set to {}", self.current_profile_value()));
    }

    pub(crate) fn cycle_target(&mut self, direction: i32) {
        let Some(choices) = self.input_choices("target") else {
            self.set_status("target input is not declared");
            return;
        };
        if choices.is_empty() {
            self.set_status("target has no choices to cycle");
            return;
        }
        let current = self.current_target_value();
        let current_index = choices
            .iter()
            .position(|choice| choice == &current)
            .unwrap_or(0);
        let len = choices.len() as i32;
        let next = (current_index as i32 + direction).rem_euclid(len) as usize;
        self.set_or_clear_override("input.target", &choices[next]);
        self.set_or_clear_override("build.target", &choices[next]);
        self.request_refresh(format!("target set to {}", self.current_target_value()));
    }

    pub(crate) fn activate_input(&mut self, name: &str, direction: i32) {
        let Some(input) = self.input_option(name) else {
            self.set_status(format!("input '{name}' is not declared"));
            return;
        };
        match input.kind {
            gaia_spec::InputKindSpec::Enum => self.cycle_input_choice(name, direction),
            gaia_spec::InputKindSpec::Boolean => self.toggle_boolean_input(name),
            _ => self.begin_edit(
                SetupEditField::Input(name.to_string()),
                self.current_input_value(name),
            ),
        }
    }

    fn cycle_input_choice(&mut self, name: &str, direction: i32) {
        let Some(choices) = self.input_choices(name) else {
            self.set_status(format!("input '{name}' is not declared"));
            return;
        };
        if choices.is_empty() {
            self.set_status(format!("input '{name}' has no choices to cycle"));
            return;
        }
        let current = self.current_input_value(name);
        let current_index = choices
            .iter()
            .position(|choice| choice == &current)
            .unwrap_or(0);
        let len = choices.len() as i32;
        let next = (current_index as i32 + direction).rem_euclid(len) as usize;
        self.set_input_override(name, &choices[next]);
        self.request_refresh(format!("{name} set to {}", self.current_input_value(name)));
    }

    fn toggle_boolean_input(&mut self, name: &str) {
        let current = self.current_input_value(name);
        let next = if current.eq_ignore_ascii_case("true") {
            "false"
        } else {
            "true"
        };
        self.set_input_override(name, next);
        self.request_refresh(format!("{name} set to {}", self.current_input_value(name)));
    }

    fn set_input_override(&mut self, name: &str, value: &str) {
        self.set_or_clear_override(&format!("input.{name}"), value);
        match name {
            "target" => self.set_or_clear_override("build.target", value),
            "profile" => self.set_or_clear_override("build.profile", value),
            _ => {}
        }
    }

    fn input_option(&self, name: &str) -> Option<gaia_spec::InputOptionSpec> {
        self.spec.as_ref().and_then(|spec| {
            spec.inputs
                .declared
                .iter()
                .find(|input| input.name == name)
                .cloned()
        })
    }

    fn input_choices(&self, name: &str) -> Option<Vec<String>> {
        self.spec.as_ref().and_then(|spec| {
            spec.inputs
                .declared
                .iter()
                .find(|input| input.name == name)
                .map(|input| input.choices.clone())
        })
    }

    pub(crate) fn cycle_jobs(&mut self, direction: i32) {
        let steps = [0u32, 1, 2, 4, 8, 12, 16];
        let current = self.current_jobs_value().parse::<u32>().unwrap_or(0);
        let current_index = steps
            .iter()
            .position(|value| *value == current)
            .unwrap_or(0) as i32;
        let next = (current_index + direction).clamp(0, (steps.len() - 1) as i32) as usize;
        self.set_or_clear_override("execution.jobs", &steps[next].to_string());
        self.request_refresh(format!("jobs set to {}", self.current_jobs_label()));
    }

    pub(crate) fn setup_item_label(&self, item: SetupItem) -> String {
        match item {
            SetupItem::Branch => format!("Branch: {}", self.current_branch_value()),
            SetupItem::Target => format!("Target: {}", self.current_target_value()),
            SetupItem::Profile => format!("Profile: {}", self.current_profile_value()),
            SetupItem::Input(name) => format!("{name}: {}", self.current_input_value(&name)),
            SetupItem::Jobs => format!("Jobs: {}", self.current_jobs_label()),
            _ => item.title().to_string(),
        }
    }

    pub(crate) fn index_of_setup_item(&self, item: &SetupItem) -> usize {
        self.setup_items
            .iter()
            .position(|candidate| candidate == item)
            .unwrap_or(0)
    }

    pub(crate) fn move_operation_down(&mut self) {
        let total = self.operation_total();
        if total == 0 {
            return;
        }
        let current = self.operation_list.selected().unwrap_or(0);
        self.operation_list
            .select(Some((current + 1).min(total - 1)));
        self.detail_scroll = 0;
        self.detail_follow_tail = true;
    }

    pub(crate) fn move_operation_up(&mut self) {
        let current = self.operation_list.selected().unwrap_or(0);
        self.operation_list.select(Some(current.saturating_sub(1)));
        self.detail_scroll = 0;
        self.detail_follow_tail = true;
    }

    pub(crate) fn select_next_build(&mut self) {
        let total = self.build_entries.len();
        if total == 0 {
            return;
        }
        let current = self.build_list.selected().unwrap_or(0);
        self.build_list.select(Some((current + 1).min(total - 1)));
    }

    pub(crate) fn select_prev_build(&mut self) {
        let current = self.build_list.selected().unwrap_or(0);
        self.build_list.select(Some(current.saturating_sub(1)));
    }

    pub(crate) fn open_selected_build(&mut self) {
        let Some(index) = self.build_list.selected() else {
            return;
        };
        let Some(entry) = self.build_entries.get(index) else {
            return;
        };
        let path = entry.path.clone();
        let label = entry.label.clone();
        self.build = path;
        self.refresh();
        self.screen = Screen::Setup;
        self.set_status(format!("loaded build {label}"));
    }

    pub(crate) fn ensure_build_selection(&mut self) {
        let total = self.build_entries.len();
        match total {
            0 => self.build_list.select(None),
            _ if self.build_list.selected().is_none() => self.build_list.select(Some(0)),
            _ => {
                let current = self.build_list.selected().unwrap_or(0);
                if current >= total {
                    self.build_list.select(Some(total - 1));
                }
            }
        }
    }
}

pub(crate) fn format_jobs_label(value: &str) -> String {
    if value == "0" {
        "0 (all cores)".to_string()
    } else {
        value.to_string()
    }
}
