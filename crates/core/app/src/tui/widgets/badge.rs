//! Status badges: an icon and a word, coloured by what it means. Operations
//! use [`op_badge`]; the whole run uses [`phase_badge`].

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::commands::live_status::{LiveOperation, OpState};
use crate::tui::run_view::RunPhase;
use crate::tui::theme;

/// The icon, word and style of one operation's state.
pub(crate) fn op_badge(op: &LiveOperation) -> (&'static str, &'static str, Style) {
    match op.state {
        OpState::Pending if op.planned_reuse => (theme::ICON_REUSED, "REUSE", theme::muted()),
        OpState::Pending => (theme::ICON_PENDING, "PEND", theme::muted()),
        OpState::Running => (theme::ICON_RUNNING, "RUN", theme::running()),
        OpState::Done => (theme::ICON_OK, "OK", theme::ok()),
        OpState::Reused => (theme::ICON_REUSED, "REUSE", theme::reused()),
        OpState::Failed => (theme::ICON_FAILED, "FAIL", theme::failed()),
        OpState::Cancelled => (theme::ICON_CANCELLED, "CANCEL", theme::warn()),
        OpState::Skipped => (theme::ICON_SKIPPED, "SKIP", theme::muted()),
        OpState::Unknown => ("?", "?", theme::muted()),
    }
}

/// The header badge of a run: a filled label with its state.
pub(crate) fn phase_badge(phase: RunPhase) -> Span<'static> {
    let (text, background) = match phase {
        RunPhase::Starting => (
            format!(" {} STARTING ", theme::ICON_PENDING),
            Color::DarkGray,
        ),
        RunPhase::Running => (format!(" {} RUNNING ", theme::ICON_PLAY), Color::Cyan),
        RunPhase::Paused => (format!(" {} PAUSED ", theme::ICON_PAUSED), Color::Magenta),
        RunPhase::Cancelling => (
            format!(" {} CANCELLING ", theme::ICON_CANCELLED),
            Color::Yellow,
        ),
        RunPhase::Completed => (format!(" {} DONE ", theme::ICON_OK), Color::Green),
        RunPhase::Failed => (format!(" {} FAILED ", theme::ICON_FAILED), Color::Red),
        RunPhase::Cancelled => (
            format!(" {} CANCELLED ", theme::ICON_CANCELLED),
            Color::Yellow,
        ),
        RunPhase::Ended => (" ENDED ".to_string(), Color::DarkGray),
    };
    let foreground = match phase {
        RunPhase::Failed => Color::White,
        RunPhase::Starting | RunPhase::Ended => Color::White,
        _ => Color::Black,
    };
    Span::styled(
        text,
        Style::default()
            .fg(foreground)
            .bg(background)
            .add_modifier(Modifier::BOLD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_reuse_reads_as_reuse_until_it_runs() {
        let planned = LiveOperation {
            planned_reuse: true,
            ..LiveOperation::default()
        };
        assert_eq!(op_badge(&planned).1, "REUSE");
        assert_eq!(op_badge(&LiveOperation::default()).1, "PEND");
        let failed = LiveOperation {
            state: OpState::Failed,
            ..LiveOperation::default()
        };
        assert_eq!(op_badge(&failed).1, "FAIL");
    }

    #[test]
    fn phase_badges_carry_their_word() {
        assert!(phase_badge(RunPhase::Paused).content.contains("PAUSED"));
        assert!(phase_badge(RunPhase::Failed).content.contains("FAILED"));
    }
}
