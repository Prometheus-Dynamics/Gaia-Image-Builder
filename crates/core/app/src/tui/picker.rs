//! The start screen: the gaia builds on this system that run now or ran
//! lately, above the build configs of this directory. A running build shows
//! its progress and estimate; a build config shows its target, profile and
//! the outcome of its last run.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use super::*;
use crate::commands::live_status::unix_now_ms;
use crate::commands::run_registry::ListedRun;
use crate::commands::status::short_elapsed;
use crate::tui::run_view::{RunSource, RunView};
use crate::tui::theme;
use crate::tui::widgets::badge::phase_badge;
use crate::tui::widgets::bars::bar;
use crate::tui::widgets::chrome;
use crate::tui::widgets::format;

/// The marker of the row under the cursor.
const CURSOR: &str = "▸ ";

/// The start screen. Without running or recent builds it is the build configs
/// alone.
pub(crate) fn render(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    if state.runs.is_empty() {
        render_builds(frame, area, state);
        return;
    }
    // Each run takes its two-line card, each section a heading, and the frame
    // two more lines.
    let live = state.runs.iter().any(|run| run.is_live());
    let ended = state.runs.iter().any(|run| !run.is_live());
    let headings = u16::from(live) + u16::from(ended);
    let wanted = state.runs.len() as u16 * 2 + headings + 2;
    let runs_height = wanted.min((area.height * 2 / 3).max(4));
    let split = Layout::vertical([Constraint::Length(runs_height), Constraint::Min(3)]).split(area);
    render_runs(frame, split[0], state);
    render_builds(frame, split[1], state);
}

fn highlight() -> Style {
    theme::selected().add_modifier(Modifier::BOLD)
}

/// The rows of the running and recent builds, with section headings. The
/// cursor is on a run, so the heading rows are skipped by the cursor.
fn render_runs(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let now = unix_now_ms();
    let live = state.runs.iter().filter(|run| run.is_live()).count();
    let mut items = Vec::new();
    let mut run_row_of_item = Vec::new();
    let mut heading_shown = [false, false];
    for (index, run) in state.runs.iter().enumerate() {
        let section = usize::from(!run.is_live());
        if !heading_shown[section] {
            heading_shown[section] = true;
            let text = if run.is_live() {
                "Running now"
            } else {
                "Recent (ended in the last day)"
            };
            items.push(ListItem::new(Line::styled(text, theme::accent())));
            run_row_of_item.push(None);
        }
        let inner_width = area.width.saturating_sub(6) as usize;
        items.push(ListItem::new(run_lines(run, now, inner_width)));
        run_row_of_item.push(Some(index));
    }
    let display_selected = if state.picker_focus == PickerFocus::Runs {
        state.run_list.selected().and_then(|selected| {
            run_row_of_item
                .iter()
                .position(|row| *row == Some(selected))
        })
    } else {
        None
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" Builds on this system ({live} running) "))
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .highlight_symbol(CURSOR)
        .highlight_style(highlight());
    let mut list_state = ListState::default().with_offset(state.run_list.offset());
    list_state.select(display_selected);
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// A run's card: its badge, name and variant, then its progress and times for
/// a live run, or how it ended for a recent one.
fn run_lines(run: &ListedRun, now: u64, width: usize) -> Vec<Line<'static>> {
    let (view, detail) = match (&run.live, &run.ended) {
        (Some(live), _) => {
            let view = match &live.status {
                Some(status) => RunView::live(status, RunSource::Attached, live.paused(), now),
                None => RunView::starting(Some(live.pid), RunSource::Attached, now),
            };
            (Some(view), None)
        }
        (None, Some(last)) => (
            Some(RunView::from_last(last, RunSource::Attached, now)),
            Some(format!(
                "ended {} ago",
                short_elapsed(now.saturating_sub(last.ended_at * 1000) / 1000)
            )),
        ),
        (None, None) => (None, None),
    };
    let Some(view) = view else {
        return vec![Line::from(format!(
            "{}  state unknown",
            run.run.display_name
        ))];
    };
    let mut title = vec![
        phase_badge(view.phase),
        Span::raw(" "),
        Span::styled(run.run.display_name.clone(), theme::accent()),
    ];
    if let Some(variant) = &view.variant {
        title.push(Span::styled(
            format!(
                "  variant {} {}/{}",
                variant.id, variant.index, variant.count
            ),
            theme::reused(),
        ));
    }
    title.push(Span::styled(
        format!("  pid {}", run.run.pid),
        theme::muted(),
    ));
    if view.is_over() {
        let mut lines = vec![Line::from(title)];
        lines.push(Line::styled(
            format!(
                "  took {}  ops {}/{}  {}",
                format::duration(view.elapsed_ms()),
                view.counts.total().min(view.ops_total),
                view.ops_total,
                detail.unwrap_or_default()
            ),
            theme::muted(),
        ));
        return lines;
    }
    let percent = view.progress_percent();
    let bar_width = width.saturating_sub(44).clamp(6, 24);
    let mut detail_spans = vec![
        Span::raw(format!(
            "  {} {:>3}%  ",
            bar(bar_width, percent as f64 / 100.0),
            percent
        )),
        Span::styled(
            format!(
                "ops {}/{}  elapsed {}",
                view.counts.total().min(view.ops_total),
                view.ops_total,
                format::duration(view.elapsed_ms())
            ),
            theme::muted(),
        ),
    ];
    if let Some(eta) = view.eta_ms() {
        detail_spans.push(Span::raw(format!("  ETA {}", format::eta(eta))));
    }
    if let Some(op) = view.current_op() {
        detail_spans.push(Span::styled(
            format!("  now {}", format::truncate(&op.id, 24)),
            theme::running(),
        ));
    }
    vec![Line::from(title), Line::from(detail_spans)]
}

/// The build configs of this directory, each with its target, profile and
/// last run.
fn render_builds(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    if state.build_entries.is_empty() {
        chrome::empty_state(
            frame,
            area,
            "Build configs",
            &[
                "No build configs in this directory.",
                "Run gaia tui from a project directory, or use --builds-dir <dir>.",
            ],
        );
        return;
    }
    let now = unix_now_ms();
    let items = state
        .build_entries
        .iter()
        .map(|entry| ListItem::new(build_lines(&entry.label, &entry.path, state, now)))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" Build configs ({}) ", state.build_entries.len()))
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .highlight_symbol(CURSOR)
        .highlight_style(highlight());
    if state.picker_focus == PickerFocus::Builds {
        frame.render_stateful_widget(list, area, &mut state.build_list);
    } else {
        frame.render_stateful_widget(list, area, &mut ListState::default());
    }
}

/// A build config's entry: its name, target and profile, then its live run or
/// the outcome of its last run.
fn build_lines(label: &str, path: &str, state: &TuiState<'_>, now: u64) -> Vec<Line<'static>> {
    let mut title = vec![Span::styled(label.to_string(), theme::accent())];
    if let Some(Some(meta)) = state.build_meta.get(path) {
        let mut parts = Vec::new();
        if let Some(target) = &meta.target {
            parts.push(format!("target {target}"));
        }
        if let Some(profile) = &meta.profile {
            parts.push(format!("profile {profile}"));
        }
        if !parts.is_empty() {
            title.push(Span::styled(
                format!("  {}", parts.join(" · ")),
                theme::muted(),
            ));
        }
    }
    let status = if let Some(live) = state.live_marks.get(path) {
        let view = match &live.status {
            Some(status) => RunView::live(status, RunSource::Attached, live.paused(), now),
            None => RunView::starting(Some(live.pid), RunSource::Attached, now),
        };
        let percent = view.progress_percent();
        vec![
            phase_badge(view.phase),
            Span::raw(format!(
                "  {} {:>3}%  elapsed {}",
                bar(10, percent as f64 / 100.0),
                percent,
                format::duration(view.elapsed_ms())
            )),
        ]
    } else if let Some(last) = state.build_last.get(path) {
        let view = RunView::from_last(last, RunSource::Attached, now);
        let age = short_elapsed(now.saturating_sub(last.ended_at * 1000) / 1000);
        vec![
            Span::styled("last run ", theme::muted()),
            phase_badge(view.phase),
            Span::raw(format!(
                "  {}  {} ago",
                format::duration(view.elapsed_ms()),
                age
            )),
        ]
    } else {
        vec![Span::styled("never run here", theme::muted())]
    };
    let mut status_line = vec![Span::raw("  ")];
    status_line.extend(status);
    vec![Line::from(title), Line::from(status_line)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::live_status::{LastRun, LiveRun, RunFiles, fixture_status};
    use crate::commands::run_registry::fixture_registered_run;
    use gaia_testkit::TempDir;
    use ratatui::backend::TestBackend;

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn live(name: &str, pid: u32) -> ListedRun {
        let mut status = fixture_status();
        status.pid = pid;
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: Some(LiveRun {
                pid,
                status: Some(status),
                stopped: false,
            }),
            ended: None,
        }
    }

    fn ended(name: &str, pid: u32) -> ListedRun {
        let mut status = fixture_status();
        status.pid = pid;
        status.running.clear();
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: None,
            ended: Some(LastRun {
                outcome: "completed".into(),
                ended_at: 1_500,
                status,
                errors: Vec::new(),
            }),
        }
    }

    /// A picker over an empty directory: no build configs of its own.
    fn picker_without_project(runs: Vec<ListedRun>) -> (TempDir, TuiState<'static>) {
        let context: &'static AppContext = Box::leak(Box::new(AppContext::with_defaults()));
        let scratch = TempDir::new("gaia-tui-picker");
        let empty = scratch.path();
        let mut state = TuiState::new(
            context,
            TuiLaunch {
                build: "missing-build.toml",
                build_explicit: false,
                builds_dir: empty.to_str(),
            },
            &ResolveOptions::default(),
        );
        state.screen = Screen::Picker;
        state.runs = runs;
        state.ensure_picker_selection();
        (scratch, state)
    }

    fn draw(state: &mut TuiState<'_>, width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        terminal
            .draw(|frame| crate::tui::render(frame, state))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn running_builds_show_progress_and_their_estimate() {
        let card = run_lines(&live("Evk image", 4242), 1_000_000 + 12 * 60_000, 80);
        let text: String = card
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect();
        assert!(text.contains("Evk image"), "{text}");
        assert!(text.contains("pid 4242"), "{text}");
        assert!(text.contains("ops 3/10"), "{text}");
        assert!(text.contains("elapsed 12m"), "{text}");
    }

    #[test]
    fn start_screen_shows_running_and_recent_builds_without_a_project() {
        let (_scratch, mut state) =
            picker_without_project(vec![live("Evk image", 4242), ended("Pi zero", 99)]);
        assert_eq!(state.picker_focus, PickerFocus::Runs);

        let text = draw(&mut state, 110, 24);
        assert!(text.contains("Builds on this system (1 running)"), "{text}");
        assert!(text.contains("Running now"), "{text}");
        assert!(text.contains("Recent (ended in the last day)"), "{text}");
        assert!(text.contains("Evk image"), "{text}");
        assert!(text.contains("pid 99"), "{text}");
        assert!(text.contains("ops 3/10"), "{text}");
        assert!(
            text.contains("No build configs in this directory."),
            "{text}"
        );
        assert!(text.contains("[Enter] open/attach"), "{text}");
    }

    #[test]
    fn start_screen_without_runs_or_projects_still_opens() {
        let (_scratch, mut state) = picker_without_project(Vec::new());
        let text = draw(&mut state, 110, 16);
        assert!(text.contains("Build configs"), "{text}");
        assert!(
            text.contains("No build configs in this directory."),
            "{text}"
        );
    }

    #[test]
    fn cursor_moves_through_running_builds_then_build_configs() {
        let (_scratch, mut state) =
            picker_without_project(vec![live("One", 4242), live("Two", 4343)]);
        // A build config of this directory, below the running builds.
        state.build_entries = vec![BuildEntry {
            label: "evk.toml".into(),
            path: "evk.toml".into(),
        }];
        state.ensure_build_selection();

        state.handle_key(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(state.run_list.selected(), Some(1));
        state.handle_key(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(state.picker_focus, PickerFocus::Builds);
        assert_eq!(state.build_list.selected(), Some(0));
        state.handle_key(KeyCode::Up, KeyModifiers::empty());
        assert_eq!(state.picker_focus, PickerFocus::Runs);
        assert_eq!(state.run_list.selected(), Some(1));
    }

    #[test]
    fn build_configs_show_target_profile_and_last_outcome() {
        let (_scratch, mut state) = picker_without_project(Vec::new());
        state.build_entries = vec![BuildEntry {
            label: "evk.toml".into(),
            path: "evk.toml".into(),
        }];
        state.build_meta.insert(
            "evk.toml".into(),
            Some(crate::tui::state::BuildMeta {
                build_dir: PathBuf::from("/nonexistent"),
                target: Some("evk".into()),
                profile: Some("release".into()),
            }),
        );
        let mut last = ended("evk", 7).ended.expect("ended");
        last.ended_at = 1_100;
        state.build_last.insert("evk.toml".into(), last);
        let text = draw(&mut state, 110, 16);
        assert!(text.contains("evk.toml"), "{text}");
        assert!(text.contains("target evk · profile release"), "{text}");
        assert!(text.contains("last run"), "{text}");
        assert!(text.contains("DONE"), "{text}");
    }

    #[test]
    fn enter_on_a_running_build_attaches_through_its_registry_entry() {
        let (_scratch, mut state) = picker_without_project(vec![live("Evk image", 4242)]);
        state.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(state.screen, Screen::Attach);
        let attach = state.attach.as_ref().expect("attached");
        assert_eq!(
            attach.files,
            RunFiles::in_build_dir(&PathBuf::from("/builds/demo/build"))
        );
    }
}
