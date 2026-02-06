const MAX_LOG_CHARS: usize = 4096;

enum EscapeMode {
    Esc,
    Csi,
    Osc,
    OscEsc,
    StTerminated,
    StEsc,
}

pub fn sanitize_log_line(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(MAX_LOG_CHARS));
    let mut esc_mode: Option<EscapeMode> = None;
    let mut truncated = false;
    let mut char_count = 0usize;

    for c in input.chars() {
        if let Some(mode) = esc_mode.as_ref() {
            match mode {
                EscapeMode::Esc => match c {
                    '[' => esc_mode = Some(EscapeMode::Csi),
                    ']' => esc_mode = Some(EscapeMode::Osc),
                    'P' | 'X' | '^' | '_' => esc_mode = Some(EscapeMode::StTerminated),
                    _ => esc_mode = None,
                },
                EscapeMode::Csi => {
                    if ('@'..='~').contains(&c) {
                        esc_mode = None;
                    }
                }
                EscapeMode::Osc => {
                    if c == '\x07' {
                        esc_mode = None;
                    } else if c == '\x1b' {
                        esc_mode = Some(EscapeMode::OscEsc);
                    }
                }
                EscapeMode::OscEsc => {
                    if c == '\\' {
                        esc_mode = None;
                    } else if c != '\x1b' {
                        esc_mode = Some(EscapeMode::Osc);
                    }
                }
                EscapeMode::StTerminated => {
                    if c == '\x1b' {
                        esc_mode = Some(EscapeMode::StEsc);
                    }
                }
                EscapeMode::StEsc => {
                    if c == '\\' {
                        esc_mode = None;
                    } else if c != '\x1b' {
                        esc_mode = Some(EscapeMode::StTerminated);
                    }
                }
            }
            continue;
        }

        if c == '\x1b' {
            esc_mode = Some(EscapeMode::Esc);
            continue;
        }
        if c == '\r' || c == '\n' {
            continue;
        }
        if c == '\t' {
            out.push(' ');
            char_count += 1;
        } else if c.is_control() || is_format_control(c) {
            continue;
        } else {
            out.push(c);
            char_count += 1;
        }

        if char_count >= MAX_LOG_CHARS {
            truncated = true;
            break;
        }
    }

    if truncated {
        out.push_str(" ...[truncated]");
    }

    out
}

fn is_format_control(c: char) -> bool {
    c == '\u{061C}'
        || c == '\u{200E}'
        || c == '\u{200F}'
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

#[cfg(test)]
mod tests {
    use super::sanitize_log_line;

    #[test]
    fn strips_csi_and_osc_sequences() {
        let input = "ok \u{1b}[31mred\u{1b}[0m \u{1b}]0;title\u{7} done";
        let got = sanitize_log_line(input);
        assert_eq!(got, "ok red  done");
    }

    #[test]
    fn strips_st_terminated_sequences() {
        let input = "a\u{1b}Ppayload\u{1b}\\b";
        let got = sanitize_log_line(input);
        assert_eq!(got, "ab");
    }

    #[test]
    fn strips_newlines_and_tabs_and_bidi_controls() {
        let input = "a\tb\nc\r\u{202e}x";
        let got = sanitize_log_line(input);
        assert_eq!(got, "a bcx");
    }
}
