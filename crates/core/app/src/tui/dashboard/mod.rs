//! The run dashboard: one view of a build, shared by the Monitor (a run this
//! TUI started) and the Attach screen (a run another process started). It
//! draws a [`RunView`] and keeps the view's own state (selection, filters,
//! chart tab, log follow) in a [`DashboardState`].
//!
//! Layout, from the top: a header bar (state, name, variant, elapsed, ETA),
//! a progress block (the overall gauge and the Buildroot packages), the
//! operations table with the charts beside it (side by side on wide
//! terminals; the charts replace the table on narrow ones, toggled with `v`),
//! the output pane, and the footer. Nothing is drawn below 40x8.

mod charts;
mod header;
mod keys;
mod logs;
mod popup;
mod session;
mod summary;
mod table;
#[cfg(test)]
mod tests;

pub(crate) use keys::DashAction;
pub(crate) use session::RunEnd;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::TableState;

use crate::commands::live_status::LiveOperation;
use crate::tui::metrics::Metrics;
use crate::tui::run_view::{OpFilter, OpSort, RunView};
use crate::tui::widgets::chrome;

/// From this width the charts sit beside the operations table.
pub(crate) const WIDE_WIDTH: u16 = 110;
/// From this height the progress block has room for its frame.
const TALL_HEIGHT: u16 = 24;
/// From this height the output pane is shown.
const LOGS_HEIGHT: u16 = 18;

/// The tabs of the chart area, in the order their number keys take. The
/// summary is last, and is shown as soon as the run is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartTab {
    Progress,
    Resources,
    Steps,
    Packages,
    Summary,
}

impl ChartTab {
    /// Every chart tab, always: the numbers never move.
    pub(crate) const ALL: [Self; 5] = [
        Self::Progress,
        Self::Resources,
        Self::Steps,
        Self::Packages,
        Self::Summary,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Progress => "Progress",
            Self::Resources => "Resources",
            Self::Steps => "Steps",
            Self::Packages => "Packages",
        }
    }
}

/// What the dashboard remembers between frames and keys.
pub(crate) struct DashboardState {
    /// The selected operation, by id.
    pub(crate) selected: Option<String>,
    pub(crate) table: TableState,
    pub(crate) filter: OpFilter,
    pub(crate) sort: OpSort,
    /// Selection follows the running operation until the user moves it.
    pub(crate) follow: bool,
    pub(crate) tab: ChartTab,
    /// The run has ended and the summary has been offered.
    ended_seen: bool,
    /// On narrow terminals: show the charts instead of the table.
    pub(crate) charts_narrow: bool,
    /// The output pane shows the newest lines.
    pub(crate) log_follow: bool,
    /// The output pane shows only the selected operation.
    pub(crate) log_only_selected: bool,
    /// Rows of the output pane, when it is shown.
    pub(crate) log_height: u16,
    /// Lines the output pane is scrolled up from its newest line.
    pub(crate) log_scroll: usize,
    /// The text to highlight in the output, while typing or after.
    pub(crate) search: Option<String>,
    /// Keys go to the search text.
    pub(crate) search_typing: bool,
    /// The operation popup is open.
    pub(crate) popup: bool,
    /// `c` was pressed: the next `c` or `y` cancels the run.
    pub(crate) cancel_armed: bool,
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            selected: None,
            table: TableState::default(),
            filter: OpFilter::All,
            sort: OpSort::Plan,
            follow: true,
            tab: ChartTab::Progress,
            ended_seen: false,
            charts_narrow: false,
            log_follow: true,
            log_only_selected: false,
            log_height: 8,
            log_scroll: 0,
            search: None,
            search_typing: false,
            popup: false,
            cancel_armed: false,
        }
    }
}

impl DashboardState {
    /// Keeps the selection on a visible operation, follows the running one
    /// when asked, and offers the summary once the run is over.
    pub(crate) fn sync(&mut self, view: &RunView) {
        if view.is_over() && !self.ended_seen {
            self.ended_seen = true;
            self.tab = ChartTab::Summary;
        }
        if self.follow
            && let Some(op) = view.current_op()
        {
            self.selected = Some(op.id.clone());
        }
        let order = view.display_order(self.filter, self.sort);
        let position = self
            .selected
            .as_deref()
            .and_then(|id| order.iter().position(|&index| view.ops[index].id == id));
        let position = match position {
            Some(position) => Some(position),
            None => {
                self.selected = order.first().map(|&index| view.ops[index].id.clone());
                (!order.is_empty()).then_some(0)
            }
        };
        self.table.select(position);
    }

    /// The operation under the cursor.
    pub(crate) fn selected_op<'v>(&self, view: &'v RunView) -> Option<&'v LiveOperation> {
        self.selected.as_deref().and_then(|id| view.op(id))
    }

    /// The chart tabs, in order.
    pub(crate) fn tabs(&self) -> [ChartTab; 5] {
        ChartTab::ALL
    }

    /// Moves the tab by `step` (wrapping).
    pub(crate) fn move_tab(&mut self, step: isize) {
        let tabs = self.tabs();
        let current = tabs.iter().position(|tab| *tab == self.tab).unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(tabs.len() as isize) as usize;
        self.tab = tabs[next];
    }
}

/// Draws the dashboard of `view` in `area`. `tail` gives the output lines of
/// an operation for the popup, from whatever the caller keeps.
pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &RunView,
    ui: &mut DashboardState,
    metrics: &Metrics,
    tail: &dyn Fn(&str) -> Vec<String>,
) {
    if !chrome::fits(area) {
        chrome::too_small(frame, area);
        return;
    }
    ui.sync(view);
    let log_rows = if area.height >= LOGS_HEIGHT && ui.log_height > 0 {
        ui.log_height.min((area.height / 3).max(3))
    } else {
        0
    };
    let progress_rows = if area.height >= TALL_HEIGHT { 4 } else { 2 };
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(progress_rows),
        Constraint::Min(4),
        Constraint::Length(log_rows),
    ])
    .split(area);

    header::render(frame, rows[0], view);
    header::render_progress(frame, rows[1], view, area.height >= TALL_HEIGHT);

    if area.width >= WIDE_WIDTH {
        let columns =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Min(0)]).split(rows[2]);
        table::render(frame, columns[0], view, ui);
        charts::render(frame, columns[1], view, ui, metrics);
    } else if ui.charts_narrow {
        charts::render(frame, rows[2], view, ui, metrics);
    } else {
        table::render(frame, rows[2], view, ui);
    }

    if log_rows > 0 {
        logs::render(frame, rows[3], view, ui);
    }
    if ui.popup
        && let Some(op) = ui.selected_op(view)
    {
        popup::render(frame, area, view, op, tail);
    }
}

/// The footer hints of the dashboard. Pause and cancel are left out once the
/// run is over, where they do nothing.
pub(crate) fn footer_hint(view_over: bool) -> String {
    let common =
        "[Enter] details  [f] filter  [s] sort  [←→/1-5] charts  [F] follow output  [/] search";
    if view_over {
        format!("{common}  [?] help  [q] quit")
    } else {
        format!("{common}  [p] pause/resume  [c] cancel  [?] help  [q] quit")
    }
}
