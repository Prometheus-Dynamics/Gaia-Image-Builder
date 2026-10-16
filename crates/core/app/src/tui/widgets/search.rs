//! Case-insensitive search in output text: the matches of a pattern, shown
//! highlighted. Shared by the dashboard's output pane and the Events tab.

use ratatui::text::Span;

use crate::tui::theme;

/// `text` split so every case-insensitive match of `pattern` is highlighted.
pub(crate) fn highlight(text: &str, pattern: &str) -> Vec<Span<'static>> {
    // ASCII lowering keeps byte positions, so matches map back onto `text`.
    let lower = text.to_ascii_lowercase();
    let needle = pattern.to_ascii_lowercase();
    let mut spans = Vec::new();
    if needle.is_empty() {
        spans.push(Span::raw(text.to_string()));
        return spans;
    }
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find(&needle) {
        let start = cursor + found;
        let end = start + needle.len();
        if start > cursor {
            spans.push(Span::raw(text[cursor..start].to_string()));
        }
        spans.push(Span::styled(
            text[start..end].to_string(),
            theme::selected(),
        ));
        cursor = end;
    }
    if cursor < text.len() {
        spans.push(Span::raw(text[cursor..].to_string()));
    }
    spans
}

/// Whether `text` contains `pattern`, ignoring ASCII case.
pub(crate) fn contains_ignore_case(text: &str, pattern: &str) -> bool {
    text.to_ascii_lowercase()
        .contains(&pattern.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_marks_every_match_without_case() {
        let spans = highlight("Compiling Linux, linux done", "linux");
        let marked = spans
            .iter()
            .filter(|span| span.style == theme::selected())
            .map(|span| span.content.to_string())
            .collect::<Vec<_>>();
        assert_eq!(marked, vec!["Linux".to_string(), "linux".to_string()]);
        let joined: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(joined, "Compiling Linux, linux done");
    }

    #[test]
    fn highlight_handles_no_match_and_unicode() {
        let spans = highlight("café ok", "zzz");
        assert_eq!(spans.len(), 1);
        let spans = highlight("café ok", "ok");
        let joined: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(joined, "café ok");
        assert!(contains_ignore_case("Linux", "LIN"));
        assert_eq!(highlight("abc", "").len(), 1);
    }
}
