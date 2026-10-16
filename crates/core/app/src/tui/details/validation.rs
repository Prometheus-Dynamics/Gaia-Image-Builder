//! The Validation tab: the problems of the build, grouped by severity: errors
//! block a run, warnings do not, plan diagnostics come from the plan.

use gaia_validate::{DiagnosticSeverity, ValidationReport};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::{heading, note, render_page, severity_style};
use crate::tui::theme;

/// Lines for a list of problems, each with its code, location and message.
fn group(
    title: &str,
    style: ratatui::style::Style,
    items: Vec<(String, Option<String>, String)>,
    lines: &mut Vec<Line<'static>>,
) {
    if items.is_empty() {
        return;
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(format!("{title} ({})", items.len()), style));
    for (code, location, message) in items {
        let location = location
            .map(|value| format!("  [{value}]"))
            .unwrap_or_default();
        lines.push(Line::styled(format!("  {code}{location}"), style));
        for text in message.lines() {
            lines.push(Line::from(format!("      {text}")));
        }
    }
}

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    validation: Option<&ValidationReport>,
    plan_diagnostics: &[gaia_plan::PlanDiagnostic],
    scroll: &mut u16,
) {
    let Some(validation) = validation else {
        render_page(
            frame,
            area,
            "Validation",
            vec![note("validation has not run yet")],
            scroll,
        );
        return;
    };
    let errors = validation
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        .map(|diagnostic| {
            (
                diagnostic.code.to_string(),
                diagnostic.location.clone(),
                diagnostic.message.clone(),
            )
        })
        .collect::<Vec<_>>();
    let warnings = validation
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
        .map(|diagnostic| {
            (
                diagnostic.code.to_string(),
                diagnostic.location.clone(),
                diagnostic.message.clone(),
            )
        })
        .collect::<Vec<_>>();
    let plan_problems = plan_diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code.to_string(),
                None,
                diagnostic.message.to_string(),
            )
        })
        .collect::<Vec<_>>();

    let error_count = validation.errors.len().max(errors.len());
    let warning_count = validation.warnings.len().max(warnings.len());
    let mut lines = vec![Line::styled(
        format!(
            "errors {error_count}   warnings {warning_count}   plan problems {}",
            plan_problems.len()
        ),
        if error_count + plan_problems.len() > 0 {
            theme::failed()
        } else {
            theme::ok()
        },
    )];
    if error_count == 0 && warning_count == 0 && plan_problems.is_empty() {
        lines.push(Line::styled(
            "no problems: the build is valid and the plan has no diagnostics",
            theme::ok(),
        ));
    }
    group("Errors", severity_style(true), errors, &mut lines);
    group("Warnings", severity_style(false), warnings, &mut lines);
    group(
        "Plan problems",
        severity_style(true),
        plan_problems,
        &mut lines,
    );
    if error_count == 0 && validation.diagnostics.is_empty() && !validation.errors.is_empty() {
        lines.push(heading("Errors"));
        lines.extend(
            validation
                .errors
                .iter()
                .map(|text| Line::from(format!("  {text}"))),
        );
    }
    render_page(frame, area, "Validation", lines, scroll);
}
