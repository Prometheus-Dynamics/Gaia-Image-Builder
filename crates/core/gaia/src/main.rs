use std::fmt;
use std::io::{self, IsTerminal};

use tracing::{Event, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// Filter used when `RUST_LOG` is unset or empty. WARN and above keeps the
/// console quiet for interactive runs; `RUST_LOG=info` or `RUST_LOG=debug`
/// brings the INFO/DEBUG lines back.
const DEFAULT_LOG_DIRECTIVE: &str = "warn";

fn main() {
    bootstrap_logging();
    std::process::exit(gaia_app::run());
}

fn bootstrap_logging() {
    let rust_log = std::env::var("RUST_LOG").ok();
    let filter = EnvFilter::try_new(log_filter_directive(rust_log.as_deref()))
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_DIRECTIVE));
    let ansi = io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();

    if std::env::args().nth(1).as_deref() == Some("tui") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(ansi)
            .event_format(QuietFormat { ansi })
            .with_writer(io::sink)
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(ansi)
            .event_format(QuietFormat { ansi })
            .with_writer(io::stderr)
            .try_init();
    }

    tracing::debug!("gaia bootstrap logging initialized");
}

/// Picks the filter directive: the `RUST_LOG` value when it is set and not
/// blank, otherwise the quiet default.
fn log_filter_directive(rust_log: Option<&str>) -> &str {
    match rust_log.map(str::trim) {
        Some(value) if !value.is_empty() => value,
        _ => DEFAULT_LOG_DIRECTIVE,
    }
}

/// One line per event: level, message, then the event's own fields. Span
/// scope (`run_build{build=...}:execute_plan{...}:`) is not printed, so
/// warnings stay readable next to Gaia's progress lines.
struct QuietFormat {
    ansi: bool,
}

impl<S, N> FormatEvent<S, N> for QuietFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let level = *event.metadata().level();
        let label = level.as_str();
        if self.ansi {
            let color = match level {
                tracing::Level::ERROR => "31",
                tracing::Level::WARN => "33",
                tracing::Level::INFO => "32",
                tracing::Level::DEBUG => "34",
                tracing::Level::TRACE => "35",
            };
            write!(writer, "\x1b[{color}m{label:<5}\x1b[0m ")?;
        } else {
            write!(writer, "{label:<5} ")?;
        }
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_LOG_DIRECTIVE, log_filter_directive};
    use tracing_subscriber::EnvFilter;

    #[test]
    fn unset_or_blank_rust_log_uses_quiet_default() {
        assert_eq!(log_filter_directive(None), "warn");
        assert_eq!(log_filter_directive(Some("")), DEFAULT_LOG_DIRECTIVE);
        assert_eq!(log_filter_directive(Some("   ")), "warn");
    }

    #[test]
    fn rust_log_value_overrides_default() {
        assert_eq!(log_filter_directive(Some("info")), "info");
        assert_eq!(
            log_filter_directive(Some(" gaia_exec=debug ")),
            "gaia_exec=debug"
        );
    }

    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("buffer lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn quiet_format_prints_level_message_and_fields_without_spans() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(log_filter_directive(None)))
            .with_ansi(false)
            .event_format(super::QuietFormat { ansi: false })
            .with_writer(move || writer.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("run_build", build = "build.toml");
            let _guard = span.enter();
            tracing::info!(operation_id = %"source:x", "operation reused");
            tracing::warn!(operation_id = %"source:x", "operation cancelled");
        });

        let output = String::from_utf8(captured.0.lock().expect("buffer lock").clone())
            .expect("utf-8 log output");
        assert_eq!(output, "WARN  operation cancelled operation_id=source:x\n");
    }

    #[test]
    fn quiet_default_is_a_valid_filter() {
        assert!(EnvFilter::try_new(log_filter_directive(None)).is_ok());
        assert!(EnvFilter::try_new(log_filter_directive(Some("gaia_exec=debug"))).is_ok());
    }
}
