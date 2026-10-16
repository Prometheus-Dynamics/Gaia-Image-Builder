//! The setup screen: the form on the left, a preview of the next run on the
//! right. The form shows each value with a validation badge; the preview says
//! what the run does (operations to run and why, the recorded time, the
//! validation outcome) and carries the Start button.

use std::collections::BTreeMap;

use gaia_plan::{ExecutionPlan, OperationReuse};
use gaia_validate::{DiagnosticSeverity, ValidationReport};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use super::*;
use crate::tui::theme;
use crate::tui::widgets::bars::{bar, share};
use crate::tui::widgets::format;

/// Reason codes listed in the preview, most frequent first.
const REASONS: usize = 6;
/// Validation messages listed in the preview.
const MESSAGES: usize = 3;

/// What the next run does, counted from the plan and the recorded durations.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PlanPreview {
    pub(crate) run: usize,
    pub(crate) reused: usize,
    /// Operations that run, per reason code, most frequent first.
    pub(crate) reasons: Vec<(String, usize)>,
    /// The recorded time of what runs, summed over the operations that have
    /// one (a serial figure; parallel work overlaps).
    pub(crate) recorded_ms: u64,
    /// Operations that run and have a recorded time.
    pub(crate) timed: usize,
    /// Operations that run without a recorded time.
    pub(crate) untimed: usize,
}

/// Counts what `plan` runs and reuses, and what the running part took last.
pub(crate) fn preview_of(plan: &ExecutionPlan, durations: &BTreeMap<String, u64>) -> PlanPreview {
    let mut preview = PlanPreview::default();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for operation in &plan.operations {
        match &operation.reuse {
            OperationReuse::Reuse { .. } => preview.reused += 1,
            OperationReuse::Execute(reason) => {
                preview.run += 1;
                *reasons.entry(reason.code.to_string()).or_default() += 1;
                match durations.get(operation.id.as_str()) {
                    Some(ms) => {
                        preview.recorded_ms += ms;
                        preview.timed += 1;
                    }
                    None => preview.untimed += 1,
                }
            }
        }
    }
    let mut reasons = reasons.into_iter().collect::<Vec<_>>();
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    preview.reasons = reasons;
    preview
}

/// The validation badge of a form item: an error or warning that names it, or
/// a tick when the build is checked and nothing does.
fn field_badge(validation: Option<&ValidationReport>, item: &SetupItem) -> (&'static str, Style) {
    let Some(validation) = validation else {
        return (" ", theme::muted());
    };
    let keyword = match item {
        SetupItem::Branch => "branch".to_string(),
        SetupItem::Target => "target".to_string(),
        SetupItem::Profile => "profile".to_string(),
        SetupItem::Input(name) => name.to_ascii_lowercase(),
        SetupItem::Variant => "variant".to_string(),
        SetupItem::Jobs => "jobs".to_string(),
        _ => return (" ", theme::muted()),
    };
    let names = |diagnostic: &gaia_validate::ValidationDiagnostic| {
        let location = diagnostic.location.clone().unwrap_or_default();
        format!("{location} {}", diagnostic.message)
            .to_ascii_lowercase()
            .contains(&keyword)
    };
    if validation
        .diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Error && names(d))
    {
        ("✗", theme::failed())
    } else if validation
        .diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Warning && names(d))
    {
        ("!", theme::warn())
    } else {
        ("✓", theme::ok())
    }
}

/// Whether the build can start: no validation errors, no plan diagnostics,
/// and a build state to run.
pub(crate) fn start_blocker(state: &TuiState<'_>) -> Option<String> {
    let validation = state.validation.as_ref()?;
    if error_count(validation) > 0 {
        return Some(format!("{} validation error(s)", error_count(validation)));
    }
    if !state.plan_diagnostics.is_empty() {
        return Some(format!("{} plan problem(s)", state.plan_diagnostics.len()));
    }
    None
}

fn error_count(validation: &ValidationReport) -> usize {
    validation
        .diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .count()
        .max(validation.errors.len())
}

fn warning_count(validation: &ValidationReport) -> usize {
    validation
        .diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Warning)
        .count()
        .max(validation.warnings.len())
}

/// The form on the left and the plan preview on the right.
pub(crate) fn render(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let form_width = (area.width * 45 / 100).clamp(30, 60);
    let cols = Layout::horizontal([Constraint::Length(form_width), Constraint::Min(0)]).split(area);
    render_form(frame, cols[0], state);
    render_preview(frame, cols[1], state);
}

fn render_form(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let blocker = start_blocker(state);
    let items = state
        .setup_items
        .iter()
        .map(|item| ListItem::new(form_line(state, item, blocker.as_deref())))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(theme::panel(" Setup "))
        .highlight_symbol("▸ ")
        .highlight_style(theme::selected());
    frame.render_stateful_widget(list, area, &mut state.setup_list);
}

/// One form row: the badge, the item and its value, or the action it opens.
fn form_line(state: &TuiState<'_>, item: &SetupItem, blocker: Option<&str>) -> Line<'static> {
    match item {
        SetupItem::StartBuild => match blocker {
            Some(reason) => {
                Line::styled(format!("✗ Start build  blocked: {reason}"), theme::failed())
            }
            None => Line::styled(
                "▶ Start build  [Enter]",
                theme::running().add_modifier(Modifier::BOLD),
            ),
        },
        SetupItem::Plan | SetupItem::Validation | SetupItem::Reports | SetupItem::Spec => {
            let hint = match item {
                SetupItem::Plan => state
                    .plan
                    .as_ref()
                    .map(|plan| {
                        let run = plan
                            .operations
                            .iter()
                            .filter(|operation| operation.reuse.should_execute())
                            .count();
                        format!("{run} to run")
                    })
                    .unwrap_or_default(),
                SetupItem::Validation => state
                    .validation
                    .as_ref()
                    .map(|validation| {
                        format!(
                            "{} err, {} warn",
                            error_count(validation),
                            warning_count(validation)
                        )
                    })
                    .unwrap_or_default(),
                SetupItem::Reports => "last run".to_string(),
                _ => "build".to_string(),
            };
            Line::from(vec![
                Span::raw("  "),
                Span::raw(format!("{:<12}", item.title())),
                Span::styled(format!("{hint}  →"), theme::muted()),
            ])
        }
        SetupItem::PickBuild | SetupItem::Refresh => {
            Line::from(vec![Span::raw("  "), Span::raw(item.title().to_string())])
        }
        _ => {
            let (badge, style) = field_badge(state.validation.as_ref(), item);
            let label = state.setup_item_label(item.clone());
            let (name, value) = label.split_once(": ").unwrap_or((label.as_str(), ""));
            Line::from(vec![
                Span::styled(format!("{badge} "), style),
                Span::raw(format!("{name:<12}")),
                Span::styled(value.to_string(), theme::accent()),
            ])
        }
    }
}

fn render_preview(frame: &mut Frame<'_>, area: Rect, state: &TuiState<'_>) {
    let block = Block::default()
        .title(" Plan preview ")
        .borders(Borders::ALL)
        .border_style(theme::border());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = preview_lines(state, inner.width as usize);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// The preview's lines. Without a plan it says what is missing.
pub(crate) fn preview_lines(state: &TuiState<'_>, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![start_button(state), Line::from("")];
    let Some(spec) = state.spec.as_ref() else {
        lines.push(Line::styled("loading the build...", theme::muted()));
        return lines;
    };
    lines.push(Line::styled(
        spec.identity.display_name.clone(),
        theme::accent(),
    ));
    let mut selection = Vec::new();
    if let Some(target) = &spec.metadata.target {
        selection.push(format!("target {target}"));
    }
    if let Some(profile) = &spec.metadata.profile {
        selection.push(format!("profile {profile}"));
    }
    if let Some(variant) = &spec.selection.variant {
        selection.push(format!("variant {variant}"));
    }
    if !selection.is_empty() {
        lines.push(Line::styled(selection.join(" · "), theme::muted()));
    }
    lines.push(Line::from(""));

    let Some(plan) = state.plan.as_ref() else {
        lines.push(Line::styled("plan not loaded yet", theme::muted()));
        return lines;
    };
    let preview = preview_of(plan, &state.operation_durations);
    lines.push(Line::styled("Operations", theme::accent()));
    let total = (preview.run + preview.reused).max(1);
    let bar_width = width.saturating_sub(30).clamp(6, 30);
    lines.push(Line::from(vec![
        Span::styled(format!("  {} run", preview.run), theme::running()),
        Span::raw("  "),
        Span::styled(format!("{} reused", preview.reused), theme::reused()),
        Span::raw("  "),
        Span::raw(bar(bar_width, share(preview.run as u64, total as u64))),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::styled("Why they run", theme::accent()));
    if preview.reasons.is_empty() {
        lines.push(Line::styled(
            "  nothing to run: every operation is reused",
            theme::ok(),
        ));
    }
    let widest = preview
        .reasons
        .iter()
        .map(|(code, _)| code.len())
        .max()
        .unwrap_or(0);
    for (code, count) in preview.reasons.iter().take(REASONS) {
        lines.push(Line::from(format!("  {code:<widest$}  {count}")));
    }
    if preview.reasons.len() > REASONS {
        lines.push(Line::styled(
            format!(
                "  ... and {} more reason(s)",
                preview.reasons.len() - REASONS
            ),
            theme::muted(),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("Time", theme::accent()));
    if preview.timed == 0 {
        lines.push(Line::styled(
            "  no recorded time yet; the first run records it",
            theme::muted(),
        ));
    } else {
        lines.push(Line::from(format!(
            "  ~{} of recorded work over {} operation(s)",
            format::duration(preview.recorded_ms),
            preview.timed
        )));
    }
    if preview.untimed > 0 && preview.run > 0 {
        lines.push(Line::styled(
            format!("  {} operation(s) have no recorded time", preview.untimed),
            theme::muted(),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("Validation", theme::accent()));
    lines.extend(validation_lines(state));
    lines
}

/// The start button: a banner that says how to start, or why it cannot.
fn start_button(state: &TuiState<'_>) -> Line<'static> {
    match start_blocker(state) {
        None if state.spec.is_some() => Line::styled(
            " ▶ START BUILD   Enter / s ",
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(ratatui::style::Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Some(reason) => Line::styled(
            format!(" ✗ CANNOT START: {reason}; see Validation (d) "),
            theme::failed(),
        ),
        None => Line::styled(" loading the build... ", theme::muted()),
    }
}

fn validation_lines(state: &TuiState<'_>) -> Vec<Line<'static>> {
    let Some(validation) = state.validation.as_ref() else {
        return vec![Line::styled("  not checked yet", theme::muted())];
    };
    let errors = error_count(validation);
    let warnings = warning_count(validation);
    let mut lines = Vec::new();
    if errors > 0 {
        lines.push(Line::styled(
            format!("  ✗ {errors} error(s), {warnings} warning(s)"),
            theme::failed(),
        ));
    } else if warnings > 0 {
        lines.push(Line::styled(
            format!("  ! {warnings} warning(s), no errors"),
            theme::warn(),
        ));
    } else {
        lines.push(Line::styled("  ✓ valid", theme::ok()));
    }
    let messages = validation
        .diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .map(|d| d.message.clone())
        .chain(validation.errors.iter().cloned())
        .take(MESSAGES)
        .collect::<Vec<_>>();
    for message in messages {
        lines.push(Line::styled(
            format!("    {}", message.lines().next().unwrap_or("")),
            theme::failed(),
        ));
    }
    if !state.plan_diagnostics.is_empty() {
        lines.push(Line::styled(
            format!("  ✗ {} plan problem(s)", state.plan_diagnostics.len()),
            theme::failed(),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_plan::{OperationId, OperationKind, PlannedOperation};

    #[test]
    fn preview_groups_the_running_operations_by_reason() {
        let mut plan = ExecutionPlan {
            build_id: gaia_spec::BuildId::new("demo"),
            operations: Vec::new(),
        };
        for (id, reuse) in [
            (
                "source:repo",
                OperationReuse::Reuse {
                    source: "state".into(),
                },
            ),
            (
                "artifact:cli",
                OperationReuse::execute("config_changed", "config changed"),
            ),
            (
                "artifact:kernel",
                OperationReuse::execute("config_changed", "config changed"),
            ),
            (
                "image:buildroot",
                OperationReuse::execute("source_changed", "source changed"),
            ),
        ] {
            let mut operation =
                PlannedOperation::new(OperationId::new(id), OperationKind::ResolveBuild);
            operation.reuse = reuse;
            plan.operations.push(operation);
        }
        let durations = BTreeMap::from([("artifact:cli".to_string(), 2_000u64)]);
        let preview = preview_of(&plan, &durations);
        assert_eq!((preview.run, preview.reused), (3, 1));
        assert_eq!(
            preview.reasons,
            vec![
                ("config_changed".to_string(), 2),
                ("source_changed".to_string(), 1)
            ]
        );
        assert_eq!(preview.recorded_ms, 2_000);
        assert_eq!((preview.timed, preview.untimed), (1, 2));
    }
}
