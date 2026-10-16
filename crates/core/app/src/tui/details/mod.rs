//! The details screen: everything about the build beyond the setup form, as
//! full-screen tabs. Plan lists what the next run does and why; Validation
//! groups the problems by severity; Reports summarises the last run; Spec
//! shows the resolved build in sections; Events is the run's searchable output.
//! Every tab scrolls with the same keys, and Esc returns to the setup.

mod events;
mod plan;
mod reports;
mod spec;
mod validation;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, TableState, Wrap};

pub(crate) use plan::PlanFilter;

use super::*;
use crate::commands::live_status::unix_now;
use crate::tui::run_view::{RunSource, RunView};
use crate::tui::theme;

/// Lines a page key moves a text tab.
const PAGE: u16 = 10;

/// What the details screen remembers between keys.
#[derive(Default)]
pub(crate) struct DetailsState {
    pub(crate) tab: Option<DetailTab>,
    pub(crate) plan_filter: PlanFilter,
    pub(crate) plan_table: TableState,
    /// Lines a text tab is scrolled down.
    pub(crate) scroll: u16,
    pub(crate) events: events::EventsState,
}

impl DetailsState {
    pub(crate) fn tab(&self) -> DetailTab {
        self.tab.unwrap_or(DetailTab::Plan)
    }
}

impl<'a> TuiState<'a> {
    /// Opens the details screen on `tab`.
    pub(crate) fn open_details(&mut self, tab: DetailTab) {
        self.details.tab = Some(tab);
        self.details.scroll = 0;
        self.screen = Screen::Details;
    }

    /// The run the reports and events describe: the run this TUI started or
    /// the attached one, else the build's last recorded run. `None` before
    /// any run of the build.
    pub(crate) fn current_view(&self) -> Option<RunView> {
        if let Some(view) = self.monitor_run_view() {
            return Some(view);
        }
        if let Some(view) = self.attach.as_ref().and_then(|attach| attach.view.clone()) {
            return Some(view);
        }
        let spec = self.spec.as_ref()?;
        let last = crate::commands::live_status::read_last(std::path::Path::new(
            &spec.workspace.build_dir,
        ))?;
        Some(RunView::from_last(
            &last,
            RunSource::Attached,
            unix_now() * 1000,
        ))
    }

    /// A key on the details screen.
    pub(crate) fn handle_details_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.details.events.typing() {
            self.details.events.search_key(code);
            return;
        }
        let plain = modifiers.is_empty();
        match code {
            KeyCode::Esc => {
                if self.details.tab() == DetailTab::Events && self.details.events.clear_search() {
                    return;
                }
                self.screen = Screen::Setup;
            }
            KeyCode::Left if plain => self.step_details_tab(-1),
            KeyCode::Right if plain => self.step_details_tab(1),
            KeyCode::Tab => self.step_details_tab(1),
            KeyCode::BackTab => self.step_details_tab(-1),
            KeyCode::Char(digit @ '1'..='5') => {
                let index = digit as usize - '1' as usize;
                self.open_details(DetailTab::ALL[index]);
            }
            KeyCode::Up | KeyCode::Char('k') => self.details_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.details_move(1),
            KeyCode::PageUp => self.details_move(-(PAGE as isize)),
            KeyCode::PageDown => self.details_move(PAGE as isize),
            KeyCode::Home => self.details_home(),
            KeyCode::End => self.details_end(),
            KeyCode::Char('f') if self.details.tab() == DetailTab::Plan => {
                self.details.plan_filter = self.details.plan_filter.next();
                self.details.plan_table = TableState::default();
                self.set_status(format!("plan: {}", self.details.plan_filter.label()));
            }
            KeyCode::Char('/') if self.details.tab() == DetailTab::Events => {
                self.details.events.start_search();
            }
            KeyCode::Char('F') if self.details.tab() == DetailTab::Events => {
                let follow = self.details.events.toggle_follow();
                self.set_status(if follow {
                    "output follows the newest line"
                } else {
                    "output is held in place"
                });
            }
            KeyCode::Char('o') if self.details.tab() == DetailTab::Events => {
                let ops = self
                    .current_view()
                    .map(|view| view.ops.iter().map(|op| op.id.clone()).collect::<Vec<_>>());
                self.details.events.cycle_op(ops.unwrap_or_default());
            }
            KeyCode::Char('s') => self.start_run(),
            KeyCode::Char('m') => self.open_monitor_if_any(),
            KeyCode::Char('b') => self.open_picker(),
            _ => {}
        }
    }

    fn step_details_tab(&mut self, step: isize) {
        let tab = self.details.tab().step(step);
        self.open_details(tab);
    }

    fn details_move(&mut self, step: isize) {
        if self.details.tab() == DetailTab::Plan {
            let total = self.plan.as_ref().map_or(0, |plan| plan.operations.len());
            plan::move_selection(&mut self.details.plan_table, total, step);
        } else if self.details.tab() == DetailTab::Events {
            self.details.events.scroll_by(step);
        } else {
            self.details.scroll = (self.details.scroll as isize + step).max(0) as u16;
        }
    }

    fn details_home(&mut self) {
        match self.details.tab() {
            DetailTab::Plan => self.details.plan_table.select(Some(0)),
            DetailTab::Events => self.details.events.scroll_by(-(isize::MAX / 2)),
            _ => self.details.scroll = 0,
        }
    }

    fn details_end(&mut self) {
        match self.details.tab() {
            DetailTab::Plan => {
                let total = self.plan.as_ref().map_or(0, |plan| plan.operations.len());
                self.details
                    .plan_table
                    .select(Some(total.saturating_sub(1)));
            }
            DetailTab::Events => self.details.events.follow_newest(),
            _ => self.details.scroll = u16::MAX,
        }
    }

    /// Opens the monitor when a run of this session exists.
    pub(crate) fn open_monitor_if_any(&mut self) {
        if matches!(self.run_state, RunState::Running { .. }) || self.last_run.is_some() {
            self.screen = Screen::Monitor;
        } else {
            self.set_status("no build to monitor yet; press s to start one");
        }
    }
}

/// Draws the details screen: the tab bar, then the tab.
pub(crate) fn render(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    render_tab_bar(frame, rows[0], state.details.tab());
    let tab = state.details.tab();
    let view = state.current_view();
    let TuiState {
        details,
        plan,
        validation,
        plan_diagnostics,
        operation_durations,
        spec,
        build,
        ..
    } = state;
    match tab {
        DetailTab::Plan => plan::render(
            frame,
            rows[1],
            plan.as_ref(),
            operation_durations,
            details.plan_filter,
            &mut details.plan_table,
        ),
        DetailTab::Validation => validation::render(
            frame,
            rows[1],
            validation.as_ref(),
            plan_diagnostics,
            &mut details.scroll,
        ),
        DetailTab::Reports => reports::render(
            frame,
            rows[1],
            view.as_ref(),
            spec.as_ref(),
            &mut details.scroll,
        ),
        DetailTab::Spec => spec::render(frame, rows[1], spec.as_ref(), build, &mut details.scroll),
        DetailTab::Events => events::render(frame, rows[1], view.as_ref(), &mut details.events),
    }
}

fn render_tab_bar(frame: &mut Frame<'_>, area: Rect, current: DetailTab) {
    let mut spans = Vec::new();
    for (index, tab) in DetailTab::ALL.iter().enumerate() {
        let style = if *tab == current {
            theme::selected()
        } else {
            theme::muted()
        };
        spans.push(Span::styled(
            format!(" {} {} ", index + 1, tab.title()),
            style,
        ));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A text tab: a framed page of lines, scrolled by `scroll` and clamped so it
/// never scrolls past its end. Long lines wrap.
pub(crate) fn render_page(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    scroll: &mut u16,
) {
    let block = theme::panel(format!(" {title} "));
    let inner = block.inner(area);
    let width = inner.width.max(1) as usize;
    let rows: usize = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum();
    let max = rows
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    *scroll = (*scroll).min(max);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((*scroll, 0))
            .block(block),
        area,
    );
}

/// A heading line of a text tab.
pub(crate) fn heading(text: impl Into<String>) -> Line<'static> {
    Line::styled(text.into(), theme::accent())
}

/// A `key  value` line with the key in the muted style.
pub(crate) fn key_value(key: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<14}"), theme::muted()),
        Span::raw(value.into()),
    ])
}

/// A muted line, for a tab with nothing to show.
pub(crate) fn note(text: impl Into<String>) -> Line<'static> {
    Line::styled(text.into(), theme::muted())
}

/// The style of a line of problems: the style of the severity.
pub(crate) fn severity_style(error: bool) -> Style {
    if error {
        theme::failed()
    } else {
        theme::warn()
    }
}

/// The footer hint of the details screen, for its tab.
pub(crate) fn footer_hint(tab: DetailTab) -> &'static str {
    match tab {
        DetailTab::Plan => {
            "[←→] tab  [↑↓] move  [f] filter run/reuse  [s] start  [m] monitor  [Esc] setup"
        }
        DetailTab::Events => {
            "[←→] tab  [↑↓] scroll  [/] search  [o] operation  [F] follow  [s] start  [Esc] setup"
        }
        _ => "[←→] tab  [↑↓] scroll  [PgUp/PgDn] page  [s] start  [m] monitor  [Esc] setup",
    }
}
