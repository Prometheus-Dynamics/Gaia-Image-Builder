use super::*;

pub(crate) fn render(frame: &mut Frame<'_>, state: &mut TuiState<'_>) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(3)])
        .split(frame.area());

    match state.screen {
        Screen::Picker => render_picker(frame, layout[0], state),
        Screen::Setup => render_setup(frame, layout[0], state),
        Screen::Monitor => render_monitor(frame, layout[0], state),
    }
    render_footer(frame, layout[1], state);
}

pub(crate) fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &TuiState<'_>) {
    let hint = if state.edit_field.is_some() {
        "[Type] edit  [Backspace] delete  [Enter] apply  [Esc] cancel"
    } else {
        match state.screen {
            Screen::Picker => "[Up/Down] move  [Enter] open build  [r] reload build list  [q] quit",
            Screen::Setup => {
                "[Up/Down] move  [Left/Right] detail/value  [Enter] select/edit  [s/r] start build  [b] builds  [p] refresh  [q] quit"
            }
            Screen::Monitor => {
                "[Up/Down] select op  [Left/Right] view  [PgUp/PgDn] scroll  [End] tail  [c] cancel  [q] quit"
            }
        }
    };
    let line = if let Some(notice) = state.footer_notice() {
        Line::from(vec![
            Span::raw(hint),
            Span::styled("  |  ", Style::default().fg(Color::DarkGray)),
            Span::styled(notice.to_string(), Style::default().fg(Color::LightYellow)),
        ])
    } else {
        Line::from(hint)
    };
    let footer =
        Paragraph::new(Text::from(vec![line])).block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, area);
}

pub(crate) fn render_picker(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let items = state
        .build_entries
        .iter()
        .map(|entry| ListItem::new(entry.label.clone()))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().title("Build Picker").borders(Borders::ALL))
        .highlight_symbol(">> ")
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut state.build_list);
}

pub(crate) fn render_setup(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(0)])
        .split(area);
    render_setup_menu(frame, cols[0], state);
    render_detail_panel(frame, cols[1], state, &state.setup_panel_title());
}

pub(crate) fn render_monitor(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(8)])
        .split(area);
    render_monitor_progress(frame, rows[0], state);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(42), Constraint::Min(0)])
        .split(rows[1]);
    render_operations_list(frame, cols[0], state);
    render_detail_panel(frame, cols[1], state, &state.monitor_panel_title());
}

pub(crate) fn render_setup_menu(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let items = state
        .setup_items
        .iter()
        .map(|item| ListItem::new(state.setup_item_label(item.clone())))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().title("Setup").borders(Borders::ALL))
        .highlight_symbol(">> ")
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut state.setup_list);
}

pub(crate) fn render_operations_list(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState<'_>) {
    let items = state
        .operation_items()
        .into_iter()
        .map(|item| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", item.status),
                    Style::default().fg(item.color),
                ),
                Span::raw(item.label),
            ]))
        })
        .collect::<Vec<_>>();
    let block = Block::default().title("Operations").borders(Borders::ALL);
    let list = List::new(items)
        .block(block)
        .highlight_symbol(">> ")
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut state.operation_list);
}

pub(crate) fn render_detail_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut TuiState<'_>,
    title: &str,
) {
    let block = match state.screen {
        Screen::Monitor => Block::default()
            .title(state.monitor_tabs_title())
            .borders(Borders::ALL),
        _ => Block::default().title(title).borders(Borders::ALL),
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = state.detail_lines();
    if state.should_tail_detail() {
        let visible_lines = inner.height as usize;
        let scroll = lines.len().saturating_sub(visible_lines);
        state.detail_scroll = scroll.min(u16::MAX as usize) as u16;
    }

    let panel = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((state.detail_scroll, 0));
    frame.render_widget(
        panel,
        Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: inner.height,
        },
    );
}

pub(crate) fn render_monitor_progress(frame: &mut Frame<'_>, area: Rect, state: &TuiState<'_>) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(2)])
        .split(area);
    let gauge = Gauge::default()
        .block(
            Block::default()
                .title(format!(
                    "Progress  build={}  status={}  elapsed={}",
                    state
                        .spec
                        .as_ref()
                        .map(|spec| spec.identity.display_name.clone())
                        .unwrap_or_else(|| state.build.clone()),
                    state.run_status_label(),
                    state.run_elapsed_label(),
                ))
                .borders(Borders::ALL),
        )
        .gauge_style(Style::default().fg(Color::Green))
        .percent(state.run_progress_percent())
        .label(format!("{}%", state.run_progress_percent()));
    frame.render_widget(gauge, rows[0]);

    let summary = Paragraph::new(Text::from(vec![Line::from(state.monitor_summary_line())]))
        .block(Block::default().borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM));
    frame.render_widget(summary, rows[1]);
}
