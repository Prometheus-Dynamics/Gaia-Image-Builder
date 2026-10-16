use super::*;

impl<'a> TuiState<'a> {
    pub(crate) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.show_help {
            // Any key closes the help overlay.
            self.show_help = false;
            return;
        }
        if code == KeyCode::Char('?') && self.edit_field.is_none() {
            self.show_help = true;
            return;
        }
        match self.screen {
            Screen::Picker => self.handle_picker_key(code),
            Screen::Setup => self.handle_setup_key(code, modifiers),
            Screen::Details => self.handle_details_key(code, modifiers),
            Screen::Monitor | Screen::Attach => self.handle_dashboard_key(code, modifiers),
        }
    }

    pub(crate) fn handle_picker_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.picker_down(),
            KeyCode::Char('k') | KeyCode::Up => self.picker_up(),
            KeyCode::Enter => self.open_picker_selection(),
            KeyCode::Char('r') => {
                self.build_entries =
                    discover_build_entries(&self.build, self.builds_dir.as_deref());
                self.build_meta.clear();
                self.picker_polled_at = None;
                self.ensure_build_selection();
                self.ensure_picker_selection();
                self.set_status("reloaded build list and running builds");
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
            KeyCode::Char('b') => self.open_picker(),
            KeyCode::Char('p') => self.refresh(),
            KeyCode::Char('r') | KeyCode::Char('s') => self.start_run(),
            KeyCode::Char('m') => self.open_monitor_if_any(),
            KeyCode::Char('d') => self.open_details(DetailTab::Plan),
            KeyCode::Enter => self.activate_setup_item(),
            KeyCode::Char('j') | KeyCode::Down => self.move_setup_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_setup_up(),
            KeyCode::Left if modifiers.is_empty() => self.change_setup_value(-1),
            KeyCode::Right if modifiers.is_empty() => self.change_setup_value(1),
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
}
