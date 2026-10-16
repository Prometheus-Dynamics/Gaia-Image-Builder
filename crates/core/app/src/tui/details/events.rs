//! The Events tab: the run's output, newest last, searchable with `/`, filtered
//! to one operation with `o`, and followed by default. Each operation has its
//! colour and error and warning lines are coloured by level.

use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::note;
use crate::commands::live_status::LogLine;
use crate::tui::run_view::RunView;
use crate::tui::theme;
use crate::tui::widgets::chrome;
use crate::tui::widgets::format;
use crate::tui::widgets::search::{contains_ignore_case, highlight};

/// Width of the operation column.
const OP_WIDTH: usize = 20;
/// The colours operations take, by a hash of their id.
const OP_COLOURS: [Color; 6] = [
    Color::LightCyan,
    Color::LightMagenta,
    Color::LightBlue,
    Color::LightGreen,
    Color::Yellow,
    Color::Cyan,
];

/// The tab's own state: follow, scroll, search and the operation filter.
pub(crate) struct EventsState {
    follow: bool,
    /// Lines up from the newest line, when not following.
    back: usize,
    search: Option<String>,
    typing: bool,
    op: Option<String>,
}

impl Default for EventsState {
    fn default() -> Self {
        Self {
            follow: true,
            back: 0,
            search: None,
            typing: false,
            op: None,
        }
    }
}

impl EventsState {
    pub(crate) fn typing(&self) -> bool {
        self.typing
    }

    /// Keys while the search text is typed.
    pub(crate) fn search_key(&mut self, code: KeyCode) {
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
            KeyCode::Enter => self.typing = false,
            KeyCode::Esc => {
                self.search = None;
                self.typing = false;
            }
            _ => {}
        }
    }

    pub(crate) fn start_search(&mut self) {
        self.search = Some(String::new());
        self.typing = true;
    }

    /// Clears the search; `false` when there was none.
    pub(crate) fn clear_search(&mut self) -> bool {
        self.typing = false;
        self.search.take().is_some()
    }

    /// Switches following on or off; returns whether it follows now.
    pub(crate) fn toggle_follow(&mut self) -> bool {
        if self.follow {
            self.follow = false;
        } else {
            self.follow_newest();
        }
        self.follow
    }

    pub(crate) fn follow_newest(&mut self) {
        self.follow = true;
        self.back = 0;
    }

    /// Scrolls by `step` lines: negative is older, positive is newer.
    pub(crate) fn scroll_by(&mut self, step: isize) {
        let next = (self.back as isize - step).max(0) as usize;
        self.back = next;
        self.follow = next == 0 && (step > 0 || self.follow);
    }

    /// Moves the operation filter: all, then each operation in turn.
    pub(crate) fn cycle_op(&mut self, ops: Vec<String>) {
        self.op = match &self.op {
            None => ops.first().cloned(),
            Some(current) => ops
                .iter()
                .position(|id| id == current)
                .and_then(|index| ops.get(index + 1).cloned()),
        };
    }
}

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    view: Option<&RunView>,
    state: &mut EventsState,
) {
    let Some(view) = view else {
        chrome::empty_state(
            frame,
            area,
            "Events",
            &[
                "no output yet",
                "the output of a run appears here as it builds",
            ],
        );
        return;
    };
    let lines = view
        .logs
        .iter()
        .filter(|log| state.op.as_ref().is_none_or(|id| &log.op == id))
        .collect::<Vec<_>>();
    let scope = state
        .op
        .as_deref()
        .map(|id| format!("op {id}"))
        .unwrap_or_else(|| "all operations".to_string());
    let follow = if state.follow { "follow" } else { "held" };
    let search = match (&state.search, state.typing) {
        (Some(text), true) => format!("  search: {text}_"),
        (Some(text), false) if !text.is_empty() => {
            let matches = lines
                .iter()
                .filter(|log| contains_ignore_case(&log.line, text))
                .count();
            format!("  search \"{text}\": {matches} line(s)")
        }
        _ => String::new(),
    };
    let block = theme::panel(format!(
        " Events  {} lines  {scope}  {follow}{search} ",
        lines.len()
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if lines.is_empty() {
        frame.render_widget(Paragraph::new(note("no output for this selection")), inner);
        return;
    }

    let height = inner.height as usize;
    let max_back = lines.len().saturating_sub(height);
    state.back = state.back.min(max_back);
    let end = if state.follow {
        lines.len()
    } else {
        lines.len() - state.back
    };
    let start = end.saturating_sub(height);
    let rendered = lines[start..end]
        .iter()
        .map(|log| line_for(log, state.op.is_none(), state.search.as_deref()))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(rendered), inner);
}

fn line_for(log: &LogLine, show_op: bool, search: Option<&str>) -> Line<'static> {
    let mut spans = Vec::new();
    if show_op {
        spans.push(Span::styled(
            format!("{:<OP_WIDTH$} ", format::truncate(&log.op, OP_WIDTH)),
            Style::default().fg(op_colour(&log.op)),
        ));
    }
    let level = level_style(&log.line);
    match search.filter(|text| !text.is_empty()) {
        Some(pattern) => spans.extend(highlight(&log.line, pattern)),
        None => spans.push(Span::styled(log.line.clone(), level.unwrap_or_default())),
    }
    Line::from(spans)
}

/// The colour an operation's output takes: stable for its id.
pub(crate) fn op_colour(id: &str) -> Color {
    let hash = id.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as usize)
    });
    OP_COLOURS[hash % OP_COLOURS.len()]
}

/// Errors and warnings stand out wherever they appear in the output.
fn level_style(line: &str) -> Option<Style> {
    let lower = line.to_ascii_lowercase();
    if ["error", "failed", "fatal"]
        .iter()
        .any(|word| lower.contains(word))
    {
        Some(theme::failed())
    } else if lower.contains("warning") {
        Some(theme::warn())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_and_scroll_move_together() {
        let mut state = EventsState::default();
        assert!(state.follow);
        state.scroll_by(-3);
        assert!(!state.follow);
        assert_eq!(state.back, 3);
        state.scroll_by(3);
        assert_eq!(state.back, 0);
        assert!(state.follow, "scrolling back to the newest line follows it");
        assert!(!state.toggle_follow());
        assert!(state.toggle_follow());
    }

    #[test]
    fn operation_filter_cycles_through_the_operations_then_all() {
        let mut state = EventsState::default();
        let ops = vec!["a".to_string(), "b".to_string()];
        state.cycle_op(ops.clone());
        assert_eq!(state.op.as_deref(), Some("a"));
        state.cycle_op(ops.clone());
        assert_eq!(state.op.as_deref(), Some("b"));
        state.cycle_op(ops);
        assert_eq!(state.op, None);
    }

    #[test]
    fn search_is_typed_then_kept_or_cleared() {
        let mut state = EventsState::default();
        state.start_search();
        assert!(state.typing());
        state.search_key(KeyCode::Char('l'));
        state.search_key(KeyCode::Char('x'));
        state.search_key(KeyCode::Backspace);
        assert_eq!(state.search.as_deref(), Some("l"));
        state.search_key(KeyCode::Enter);
        assert!(!state.typing());
        assert!(state.clear_search());
        assert!(!state.clear_search());
    }

    #[test]
    fn errors_and_warnings_have_their_own_level() {
        assert!(level_style("make: *** Error 1").is_some());
        assert!(level_style("warning: unused").is_some());
        assert!(level_style("compiling").is_none());
        assert_eq!(op_colour("image:buildroot"), op_colour("image:buildroot"));
    }
}
