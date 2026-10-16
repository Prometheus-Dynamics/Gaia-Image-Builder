//! The Spec tab: the resolved build in sections (the build, its selection,
//! what it contains), one key and value per line.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::{heading, key_value, note, render_page};
use crate::tui::discovery::image_provider_label;
use crate::tui::setup::format_jobs_label;
use gaia_spec::ResolvedBuildSpec;

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    spec: Option<&ResolvedBuildSpec>,
    build: &str,
    scroll: &mut u16,
) {
    let Some(spec) = spec else {
        render_page(
            frame,
            area,
            "Spec",
            vec![note("loading the build...")],
            scroll,
        );
        return;
    };
    let mut lines = vec![heading("Build")];
    lines.push(key_value("name", spec.identity.display_name.clone()));
    lines.push(key_value("id", spec.identity.id.as_str().to_string()));
    lines.push(key_value(
        "version",
        spec.identity.version.clone().unwrap_or_else(|| "-".into()),
    ));
    lines.push(key_value("build file", build.to_string()));
    lines.push(key_value(
        "branch",
        spec.metadata.branch.clone().unwrap_or_else(|| "-".into()),
    ));
    lines.push(key_value(
        "target",
        spec.metadata.target.clone().unwrap_or_else(|| "-".into()),
    ));
    lines.push(key_value(
        "profile",
        spec.metadata.profile.clone().unwrap_or_else(|| "-".into()),
    ));
    lines.push(key_value(
        "jobs",
        format_jobs_label(spec.policy.execution.jobs.to_string().as_str()),
    ));

    lines.push(Line::from(""));
    lines.push(heading("Workspace"));
    lines.push(key_value("root", spec.workspace.root_dir.clone()));
    lines.push(key_value("build dir", spec.workspace.build_dir.clone()));
    lines.push(key_value("out dir", spec.workspace.out_dir.clone()));

    lines.push(Line::from(""));
    lines.push(heading("Selection"));
    lines.push(key_value(
        "preset",
        spec.selection
            .selected_preset
            .clone()
            .unwrap_or_else(|| "-".into()),
    ));
    lines.push(key_value(
        "requested",
        spec.selection
            .requested_build
            .clone()
            .unwrap_or_else(|| "-".into()),
    ));
    for (name, value) in &spec.selection.selected_inputs {
        lines.push(key_value(&format!("input {name}"), value.clone()));
    }
    if !spec.selection.env_files.is_empty() {
        lines.push(key_value("env files", spec.selection.env_files.join(", ")));
    }
    if !spec.selection.precedence_order.is_empty() {
        lines.push(key_value(
            "precedence",
            spec.selection.precedence_order.join(" -> "),
        ));
    }
    if !spec.selection.variants.is_empty() {
        lines.push(key_value(
            "variant",
            spec.selection.variant.clone().unwrap_or_else(|| "-".into()),
        ));
        let declared = spec
            .selection
            .variants
            .iter()
            .map(|variant| variant.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(key_value("declared", declared));
    }

    lines.push(Line::from(""));
    lines.push(heading("Contents"));
    lines.push(key_value(
        "provider",
        image_provider_label(spec).to_string(),
    ));
    lines.push(key_value("sources", spec.sources.len().to_string()));
    lines.push(key_value("artifacts", spec.artifacts.len().to_string()));
    lines.push(key_value("install", spec.install.entries.len().to_string()));
    lines.push(key_value(
        "stage",
        format!(
            "{} file(s), {} env set(s), {} service(s)",
            spec.stage.files.len(),
            spec.stage.env_sets.len(),
            spec.stage.services.len()
        ),
    ));
    lines.push(key_value(
        "checkpoints",
        spec.checkpoints.points.len().to_string(),
    ));
    if !spec.image.feed.install_entries.is_empty() {
        lines.push(Line::from(""));
        lines.push(heading("Runtime install targets"));
        for install_id in &spec.image.feed.install_entries {
            if let Some(install) = spec
                .install
                .entries
                .iter()
                .find(|entry| entry.id == *install_id)
            {
                lines.push(Line::from(format!(
                    "  {} -> {}",
                    install.id.as_str(),
                    install.dest
                )));
            }
        }
    }
    render_page(frame, area, "Spec", lines, scroll);
}
