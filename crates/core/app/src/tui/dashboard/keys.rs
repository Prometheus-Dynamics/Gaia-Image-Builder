//! The keys of the dashboard. Keys only change the dashboard's own state or
//! ask for an action; the screen carries out the actions that touch the run.

use crossterm::event::{KeyCode, KeyModifiers};

use super::{ChartTab, DashboardState};
use crate::tui::run_view::RunView;

/// What a key asks the screen to do beyond the dashboard's own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DashAction {
    None,
    /// Show a message in the footer.
    Notice(String),
    /// Cancel was asked for once; a second confirming key cancels.
    CancelArmed,
    Pause,
    Resume,
    Cancel,
    /// Back to the setup screen (Esc on the monitor).
    Back,
    /// The build picker.
    Picker,
}

/// How far a page key moves the output pane.
const PAGE: usize = 5;
/// The output pane's height limits.
const MIN_LOG_HEIGHT: u16 = 3;
const MAX_LOG_HEIGHT: u16 = 30;

impl DashboardState {
    /// Handles a key on the dashboard. `view` is the run shown, if any.
    pub(crate) fn handle_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        view: Option<&RunView>,
    ) -> DashAction {
        if self.search_typing {
            return self.search_key(code);
        }
        if self.cancel_armed {
            self.cancel_armed = false;
            return match code {
                KeyCode::Char('c' | 'y') => DashAction::Cancel,
                _ => {
                    DashAction::Notice("cancel not confirmed; the build keeps running".to_string())
                }
            };
        }
        let Some(view) = view else {
            return match code {
                KeyCode::Esc => DashAction::Back,
                KeyCode::Char('b') => DashAction::Picker,
                _ => DashAction::None,
            };
        };
        let plain = modifiers.is_empty();
        match code {
            KeyCode::Esc => {
                if self.popup {
                    self.popup = false;
                    DashAction::None
                } else if self.search.is_some() {
                    self.search = None;
                    DashAction::None
                } else {
                    DashAction::Back
                }
            }
            KeyCode::Enter => {
                if self.popup {
                    self.popup = false;
                } else {
                    self.popup = self.selected.is_some();
                }
                DashAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(view, -1);
                DashAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(view, 1);
                DashAction::None
            }
            KeyCode::Char('f') => {
                self.filter = self.filter.next();
                self.table = Default::default();
                DashAction::Notice(format!("filter: {}", self.filter.label()))
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                DashAction::Notice(format!("sort: {}", self.sort.label()))
            }
            KeyCode::Right if plain => {
                self.move_tab(1);
                DashAction::None
            }
            KeyCode::Left if plain => {
                self.move_tab(-1);
                DashAction::None
            }
            KeyCode::Tab => {
                self.move_tab(1);
                DashAction::None
            }
            KeyCode::BackTab => {
                self.move_tab(-1);
                DashAction::None
            }
            KeyCode::Char(digit @ '1'..='5') => {
                self.tab = ChartTab::ALL[digit as usize - '1' as usize];
                DashAction::None
            }
            KeyCode::Char('v') => {
                self.charts_narrow = !self.charts_narrow;
                DashAction::None
            }
            KeyCode::Char('F') => {
                self.log_follow = !self.log_follow;
                self.log_scroll = 0;
                DashAction::Notice(if self.log_follow {
                    "output follows the newest line".to_string()
                } else {
                    "output is held in place".to_string()
                })
            }
            KeyCode::Char('l') => {
                self.log_only_selected = !self.log_only_selected;
                DashAction::None
            }
            KeyCode::Char('/') => {
                self.search = Some(String::new());
                self.search_typing = true;
                DashAction::None
            }
            KeyCode::Char('+' | '=') => {
                self.log_height = (self.log_height + 1).min(MAX_LOG_HEIGHT);
                DashAction::None
            }
            KeyCode::Char('-' | '_') => {
                self.log_height = self.log_height.saturating_sub(1).max(MIN_LOG_HEIGHT);
                DashAction::None
            }
            KeyCode::PageUp => {
                self.log_follow = false;
                self.log_scroll = self.log_scroll.saturating_add(PAGE);
                DashAction::None
            }
            KeyCode::PageDown => {
                self.log_scroll = self.log_scroll.saturating_sub(PAGE);
                if self.log_scroll == 0 {
                    self.log_follow = true;
                }
                DashAction::None
            }
            KeyCode::Home => {
                self.log_follow = false;
                self.log_scroll = usize::MAX / 2;
                DashAction::None
            }
            KeyCode::End => {
                self.log_follow = true;
                self.log_scroll = 0;
                DashAction::None
            }
            KeyCode::Char('r') => {
                if view.phase == crate::tui::run_view::RunPhase::Paused {
                    DashAction::Resume
                } else {
                    DashAction::Notice("the build is not paused".to_string())
                }
            }
            KeyCode::Char('p') => {
                if view.is_over() {
                    DashAction::Notice("the build has ended".to_string())
                } else if view.phase == crate::tui::run_view::RunPhase::Paused {
                    DashAction::Resume
                } else {
                    DashAction::Pause
                }
            }
            KeyCode::Char('c') => {
                if view.is_over() {
                    DashAction::Notice("the build has ended; nothing to cancel".to_string())
                } else {
                    self.cancel_armed = true;
                    DashAction::CancelArmed
                }
            }
            KeyCode::Char('b') => DashAction::Picker,
            _ => DashAction::None,
        }
    }

    fn search_key(&mut self, code: KeyCode) -> DashAction {
        match code {
            KeyCode::Char(ch) => {
                if let Some(text) = self.search.as_mut() {
                    text.push(ch);
                }
            }
            KeyCode::Backspace => {
                if let Some(text) = self.search.as_mut() {
                    text.pop();
                }
            }
            KeyCode::Enter => self.search_typing = false,
            KeyCode::Esc => {
                self.search = None;
                self.search_typing = false;
            }
            _ => {}
        }
        DashAction::None
    }

    /// Moves the selection through the visible operations and stops
    /// following the running one.
    fn move_selection(&mut self, view: &RunView, step: isize) {
        self.follow = false;
        let order = view.display_order(self.filter, self.sort);
        if order.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_deref()
            .and_then(|id| order.iter().position(|&index| view.ops[index].id == id))
            .unwrap_or(0) as isize;
        let next = (current + step).clamp(0, order.len() as isize - 1) as usize;
        self.selected = Some(view.ops[order[next]].id.clone());
    }
}
