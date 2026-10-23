//! Validation of `[secrets]`: the entries, the references to them, and where
//! `${secret:NAME}` may appear.
//!
//! A secret is read when it is used, so the spec holds only its reference. The
//! reference forms are `secret:NAME` (a whole value, in the secret-capable
//! fields) and `${secret:NAME}` inside a build-time process environment (an
//! artifact's `build_env`). Any other place would store the value in the spec,
//! the fingerprints or the reports, so `${secret:...}` there is an error.

use std::collections::BTreeSet;

use gaia_spec::ResolvedBuildSpec;
use serde_json::Value;

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};

/// The reference that may stand inside a string, in a build-time environment.
const INTERPOLATED_PREFIX: &str = "${secret:";
/// The whole-value reference of a secret-capable field.
const SECRET_PREFIX: &str = "secret:";
/// The whole-value reference of an environment variable.
const ENV_PREFIX: &str = "env:";

pub(crate) fn validate_secrets(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let defined: BTreeSet<&str> = spec
        .secrets
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    for entry in &spec.secrets.entries {
        validate_entry(entry, diagnostics);
    }
    if let Some(signing) = &spec.image.signing {
        check_reference(&signing.key, "[image.signing] key", &defined, diagnostics);
    }
    for checkpoint in &spec.checkpoints.points {
        let Some(oci) = checkpoint
            .backend
            .as_ref()
            .and_then(|backend| backend.oci.as_ref())
        else {
            continue;
        };
        let id = checkpoint.id.as_str();
        for (field, setting) in [
            ("sign_key", oci.sign_key.as_deref()),
            ("verify_key", oci.verify_key.as_deref()),
            ("token", oci.token.as_deref()),
            ("username", oci.username.as_deref()),
        ] {
            if let Some(setting) = setting {
                check_reference(
                    setting,
                    &format!("checkpoint '{id}' {field}"),
                    &defined,
                    diagnostics,
                );
            }
        }
        if let Some(token) = oci.token.as_deref()
            && !token.trim().starts_with(SECRET_PREFIX)
            && !token.trim().starts_with(ENV_PREFIX)
        {
            diagnostics.push(error(
                "checkpoint_oci_token_literal",
                format!(
                    "checkpoint '{id}' token must be secret:NAME or env:VAR, never the token itself; define it in [secrets]"
                ),
                Some(format!("checkpoints.{id}.token")),
            ));
        }
    }
    check_placement(spec, &defined, diagnostics);
    for name in &spec.secrets.env_like {
        diagnostics.push(warning(
            "secret_like_env",
            format!(
                "${{env:{name}}} reads a variable whose name looks like a secret, so its value is stored in the spec. Use a [secrets] entry and secret:/${{secret:}} so the value is not stored"
            ),
            None,
        ));
    }
}

fn validate_entry(entry: &gaia_spec::SecretSpec, diagnostics: &mut Vec<ValidationDiagnostic>) {
    let name = entry.name.as_str();
    let location = Some(format!("secrets.{name}"));
    let valid_name = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !valid_name {
        diagnostics.push(error(
            "secret_name_invalid",
            format!("secret name '{name}' may use only letters, digits, '_', '-' and '.'"),
            location.clone(),
        ));
    }
    let sources = [
        entry.env.is_some(),
        entry.file.is_some(),
        entry.command.is_some(),
    ]
    .iter()
    .filter(|set| **set)
    .count();
    if sources != 1 {
        diagnostics.push(error(
            "secret_source",
            format!(
                "[secrets.{name}] needs exactly one of env, file or command (it sets {sources})"
            ),
            location.clone(),
        ));
    }
    if let Some(var) = &entry.env
        && var.trim().is_empty()
    {
        diagnostics.push(error(
            "secret_source",
            format!("[secrets.{name}] env names no variable"),
            location.clone(),
        ));
    }
    if let Some(path) = &entry.file
        && path.trim().is_empty()
    {
        diagnostics.push(error(
            "secret_source",
            format!("[secrets.{name}] file names no path"),
            location.clone(),
        ));
    }
    if let Some(argv) = &entry.command
        && argv.first().is_none_or(|program| program.trim().is_empty())
    {
        diagnostics.push(error(
            "secret_source",
            format!(
                "[secrets.{name}] command must be a non-empty list whose first item is a program"
            ),
            location,
        ));
    }
}

/// A whole-value reference in a secret-capable field names a defined entry.
fn check_reference(
    setting: &str,
    location: &str,
    defined: &BTreeSet<&str>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let Some(name) = setting.trim().strip_prefix(SECRET_PREFIX) else {
        return;
    };
    if !defined.contains(name) {
        diagnostics.push(error(
            "secret_undefined",
            format!("{location} names secret '{name}', which is not defined in [secrets]"),
            Some(location.to_string()),
        ));
    }
}

/// Every `${secret:NAME}` in the spec must name a defined entry and sit in an
/// artifact's `build_env`. The walk covers the whole spec, so a reference that
/// reaches any other field (by interpolation or otherwise) is reported where
/// it lands. Stored interpolation values are skipped: they only name where a
/// reference may go, and are checked where they land.
fn check_placement(
    spec: &ResolvedBuildSpec,
    defined: &BTreeSet<&str>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let Ok(value) = serde_json::to_value(spec) else {
        return;
    };
    let mut found = Vec::new();
    collect(&value, &mut Vec::new(), &mut found);
    for (path, name) in found {
        let location = path.join(".");
        if path.starts_with(&["policy".into(), "interpolation".into(), "values".into()]) {
            continue;
        }
        if !defined.contains(name.as_str()) {
            diagnostics.push(error(
                "secret_undefined",
                format!("${{secret:{name}}} at {location} names a secret that is not defined in [secrets]"),
                Some(location.clone()),
            ));
        }
        let allowed = path.first().map(String::as_str) == Some("artifacts")
            && path.iter().any(|segment| segment == "build_env");
        if !allowed {
            diagnostics.push(error(
                "secret_placement",
                format!(
                    "${{secret:{name}}} is allowed only in an artifact's java build_env, not at {location}. Use a secret: reference (for example sign_key = \"secret:{name}\") in a secret-capable field"
                ),
                Some(location),
            ));
        }
    }
}

/// Finds every `${secret:NAME}` in the strings of `value`, with the path of
/// the string (object keys and array indexes).
fn collect(value: &Value, path: &mut Vec<String>, found: &mut Vec<(Vec<String>, String)>) {
    match value {
        Value::String(text) => {
            let mut rest = text.as_str();
            while let Some(start) = rest.find(INTERPOLATED_PREFIX) {
                let after = &rest[start + INTERPOLATED_PREFIX.len()..];
                let Some(end) = after.find('}') else {
                    break;
                };
                found.push((path.clone(), after[..end].to_string()));
                rest = &after[end + 1..];
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                collect(item, path, found);
                path.pop();
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                path.push(key.clone());
                collect(item, path, found);
                path.pop();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::{DiagnosticSeverity, ValidationReport, validate_spec};
    use gaia_testkit::TempDir;

    const BASE: &str = r#"
build_name = "secrets-validate"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "rootfs"
"#;

    fn validated(text: &str) -> ValidationReport {
        let scratch = TempDir::new("gaia-validate-secrets");
        let path = scratch.join("build.toml");
        std::fs::write(&path, text).expect("write the build file");
        let spec = gaia_config::resolve_config(path.to_str().expect("utf-8 path"));
        validate_spec(&spec)
    }

    fn codes(report: &ValidationReport, severity: DiagnosticSeverity) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == severity)
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    #[test]
    fn a_signing_key_secret_must_name_a_defined_entry() {
        let report = validated(&format!(
            "{BASE}\n[image.signing]\nkind = \"gpg\"\nkey = \"secret:missing\"\noutputs = [\"*.img.xz\"]\n"
        ));
        assert!(
            codes(&report, DiagnosticSeverity::Error).contains(&"secret_undefined"),
            "{:?}",
            report.errors
        );
    }

    #[test]
    fn an_interpolated_secret_outside_a_build_env_is_refused_where_it_lands() {
        let report = validated(&format!(
            "{BASE}\n[secrets.gpg]\nenv = \"GPG_ID\"\n[image.signing]\nkind = \"gpg\"\nkey = \"${{secret:gpg}}\"\noutputs = [\"*.img.xz\"]\n"
        ));
        let errors = codes(&report, DiagnosticSeverity::Error);
        assert!(errors.contains(&"secret_placement"), "{:?}", report.errors);
    }

    #[test]
    fn a_secret_entry_names_exactly_one_source() {
        let report = validated(&format!(
            "{BASE}\n[secrets.both]\nenv = \"A\"\nfile = \"b\"\n"
        ));
        assert!(
            codes(&report, DiagnosticSeverity::Error).contains(&"secret_source"),
            "{:?}",
            report.errors
        );
    }

    #[test]
    fn a_checkpoint_token_must_be_a_reference_not_the_token() {
        let report = validated(&format!(
            "{BASE}\n[[checkpoints]]\nid = \"registry\"\nbackend = \"oci\"\nregistry = \"ghcr.io\"\nrepository = \"owner/gaia-cache\"\nuse_policy = \"off\"\nupload_policy = \"off\"\ntoken = \"ghp_literal_value_1234\"\n"
        ));
        let errors = codes(&report, DiagnosticSeverity::Error);
        assert!(
            errors.contains(&"checkpoint_oci_token_literal"),
            "{:?}",
            report.errors
        );
        assert!(
            report
                .errors
                .iter()
                .all(|message| !message.contains("ghp_literal_value_1234")),
            "{:?}",
            report.errors
        );
    }

    #[test]
    fn an_env_reference_with_a_secret_like_name_warns_that_its_value_is_stored() {
        // `version` is interpolated at load, so the value lands in the spec.
        let report = validated(&format!(
            "version = \"${{env:GAIA_TEST_CI_TOKEN}}\"\n{BASE}\n[env]\nGAIA_TEST_CI_TOKEN = \"x-value\"\n"
        ));
        assert!(
            codes(&report, DiagnosticSeverity::Warning).contains(&"secret_like_env"),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_build_env_reference_to_a_defined_entry_is_valid() {
        let report = validated(
            r#"
build_name = "secrets-java"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "myapp-source"
kind = "path"
path = "/tmp/myapp"

[[artifacts]]
id = "myapp-jar"
kind = "java"
source = "myapp-source"
build_target = "build/libs/myapp.jar"
build_command = ["./gradlew", "jar"]
build_env = [["GAIA_BUILD_TOKEN", "${secret:tok}"]]
output_path = "out/myapp.jar"

[secrets.tok]
command = ["printf", "x"]
"#,
        );
        assert!(
            codes(&report, DiagnosticSeverity::Error)
                .iter()
                .all(|code| !code.starts_with("secret_")),
            "{:?}",
            report.errors
        );
    }
}
