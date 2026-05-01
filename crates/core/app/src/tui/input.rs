use super::*;

impl<'a> TuiState<'a> {
    pub(crate) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match self.screen {
            Screen::Picker => self.handle_picker_key(code),
            Screen::Setup => self.handle_setup_key(code, modifiers),
            Screen::Monitor => self.handle_monitor_key(code, modifiers),
        }
    }

    pub(crate) fn handle_picker_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.select_next_build(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev_build(),
            KeyCode::Enter => self.open_selected_build(),
            KeyCode::Char('r') => {
                self.build_entries = discover_build_entries(&self.build);
                self.ensure_build_selection();
                self.set_status("reloaded build list");
            }
            KeyCode::Esc => self.set_status("picker escape ignored; press Enter to load a build"),
            _ => {}
        }
    }

    pub(crate) fn handle_setup_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.edit_field.is_some() {
            self.handle_edit_key(code);
            return;
        }
        match code {
            KeyCode::Char('b') => self.screen = Screen::Picker,
            KeyCode::Char('p') => self.refresh(),
            KeyCode::Char('r') | KeyCode::Char('s') => self.start_run(),
            KeyCode::Enter => self.activate_setup_item(),
            KeyCode::Down => self.move_setup_down(),
            KeyCode::Up => self.move_setup_up(),
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(10),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(10),
            KeyCode::Left if modifiers.is_empty() => self.prev_setup_detail(),
            KeyCode::Right if modifiers.is_empty() => self.next_setup_detail(),
            _ => {}
        }
    }

    pub(crate) fn handle_edit_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.edit_field = None;
                self.edit_buffer.clear();
                self.set_status("edit cancelled");
            }
            KeyCode::Enter => self.apply_edit_buffer(),
            KeyCode::Backspace => {
                self.edit_buffer.pop();
            }
            KeyCode::Char(ch) => {
                self.edit_buffer.push(ch);
            }
            _ => {}
        }
    }

    pub(crate) fn handle_monitor_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Esc => {
                if matches!(self.run_state, RunState::Running { .. }) {
                    self.set_status("cannot leave monitor while a build is running");
                } else {
                    self.screen = Screen::Setup;
                }
            }
            KeyCode::Char('c') => self.cancel_run(),
            KeyCode::Down => self.move_operation_down(),
            KeyCode::Up => self.move_operation_up(),
            KeyCode::Left if modifiers.is_empty() => self.prev_monitor_view(),
            KeyCode::Right if modifiers.is_empty() => self.next_monitor_view(),
            KeyCode::PageDown => {
                self.detail_follow_tail = false;
                self.detail_scroll = self.detail_scroll.saturating_add(10);
            }
            KeyCode::PageUp => {
                self.detail_follow_tail = false;
                self.detail_scroll = self.detail_scroll.saturating_sub(10);
            }
            KeyCode::Home => {
                self.detail_follow_tail = false;
                self.detail_scroll = 0;
            }
            KeyCode::End => {
                self.detail_follow_tail = true;
            }
            _ => {}
        }
    }
}
