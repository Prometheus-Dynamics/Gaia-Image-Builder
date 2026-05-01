use super::*;

pub(crate) fn discover_build_entries(current_build: &str) -> Vec<BuildEntry> {
    let mut paths = Vec::new();

    let build_configs_dir = PathBuf::from("configs").join("builds");
    if build_configs_dir.exists() {
        collect_toml_files(&build_configs_dir, &mut paths);
    } else {
        let configs_dir = PathBuf::from("configs");
        if configs_dir.exists() {
            collect_toml_files(&configs_dir, &mut paths);
        }
    }

    let current_path = PathBuf::from(current_build);
    if current_path.is_file() && !paths.iter().any(|path| path == &current_path) {
        paths.push(current_path);
    }

    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| BuildEntry {
            label: path.display().to_string(),
            path: path.display().to_string(),
        })
        .collect()
}

pub(crate) fn collect_toml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_toml_files(&path, out);
        } else if path.extension().and_then(|value| value.to_str()) == Some("toml") {
            out.push(path);
        }
    }
}

pub(crate) fn image_provider_label(spec: &ResolvedBuildSpec) -> &'static str {
    match spec.image.provider_kind() {
        gaia_spec::ImageProviderKind::Buildroot => "Buildroot",
        gaia_spec::ImageProviderKind::StartingPoint => "StartingPoint",
    }
}

pub(crate) fn live_operation_status(
    events: &[ExecutionEvent],
    operation_id: &str,
) -> Option<(&'static str, Color)> {
    let mut status = None;
    for event in events {
        match event {
            ExecutionEvent::Started { operation_id: id } if id.as_str() == operation_id => {
                status = Some(("RUN", Color::LightCyan));
            }
            ExecutionEvent::Succeeded { operation_id: id } if id.as_str() == operation_id => {
                status = Some(("OK", Color::Green));
            }
            ExecutionEvent::Reused { operation_id: id } if id.as_str() == operation_id => {
                status = Some(("REUSE", Color::LightBlue));
            }
            ExecutionEvent::Cancelled { operation_id: id } if id.as_str() == operation_id => {
                status = Some(("CANCEL", Color::LightYellow));
            }
            ExecutionEvent::Failed {
                operation_id: id, ..
            } if id.as_str() == operation_id => {
                status = Some(("FAIL", Color::Red));
            }
            _ => {}
        }
    }
    status
}

pub(crate) fn current_operation_label(events: &[ExecutionEvent]) -> Option<&str> {
    for event in events.iter().rev() {
        if let ExecutionEvent::Started { operation_id } = event {
            return Some(operation_id.as_str());
        }
    }
    None
}

pub(crate) fn live_completed_count(events: &[ExecutionEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                ExecutionEvent::Succeeded { .. } | ExecutionEvent::Reused { .. }
            )
        })
        .count()
}

pub(crate) fn render_event_line(event: &ExecutionEvent) -> Line<'static> {
    match event {
        ExecutionEvent::Started { operation_id } => {
            Line::from(format!("started: {}", operation_id.as_str()))
        }
        ExecutionEvent::Succeeded { operation_id } => {
            Line::from(format!("succeeded: {}", operation_id.as_str()))
        }
        ExecutionEvent::Reused { operation_id } => {
            Line::from(format!("reused: {}", operation_id.as_str()))
        }
        ExecutionEvent::Cancelled { operation_id } => {
            Line::from(format!("cancelled: {}", operation_id.as_str()))
        }
        ExecutionEvent::Failed {
            operation_id,
            message,
        } => Line::from(format!("failed: {}  {}", operation_id.as_str(), message)),
        ExecutionEvent::Log {
            operation_id,
            message,
        } => Line::from(format!("log: {}  {}", operation_id.as_str(), message)),
    }
}

pub(crate) fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let remaining = seconds % 60;
    format!("{hours:02}:{minutes:02}:{remaining:02}")
}

pub(crate) fn index_of_setup_item(item: SetupItem) -> usize {
    SetupItem::all()
        .iter()
        .position(|candidate| *candidate == item)
        .unwrap_or(0)
}

pub(crate) fn index_of_monitor_view(view: MonitorView) -> usize {
    MonitorView::all()
        .iter()
        .position(|candidate| *candidate == view)
        .unwrap_or(0)
}
