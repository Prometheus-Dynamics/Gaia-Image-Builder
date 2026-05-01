use crate::{DiagnosticSeverity, ValidationDiagnostic};

pub(crate) fn error(
    code: &'static str,
    message: String,
    location: Option<String>,
) -> ValidationDiagnostic {
    ValidationDiagnostic {
        severity: DiagnosticSeverity::Error,
        code,
        message,
        location,
    }
}

pub(crate) fn warning(
    code: &'static str,
    message: String,
    location: Option<String>,
) -> ValidationDiagnostic {
    ValidationDiagnostic {
        severity: DiagnosticSeverity::Warning,
        code,
        message,
        location,
    }
}
