use std::collections::HashSet;

use gaia_spec::{InputKindSpec, ResolvedBuildSpec};

use crate::diagnostics::error;
use crate::{DiagnosticSeverity, ValidationDiagnostic};

pub(crate) fn validate_inputs(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut declared_inputs = HashSet::new();
    for input in &spec.inputs.declared {
        if !declared_inputs.insert(input.name.clone()) {
            diagnostics.push(error(
                "duplicate_input_name",
                format!("duplicate input name '{}'", input.name),
                Some(format!("input:{}", input.name)),
            ));
        }
        if input.required
            && !spec
                .inputs
                .selected
                .iter()
                .any(|(name, _)| name == &input.name)
        {
            diagnostics.push(error(
                "required_input_missing",
                format!("required input '{}' was not selected", input.name),
                Some(format!("input:{}", input.name)),
            ));
        }
    }
    for (name, value) in &spec.inputs.selected {
        let Some(input) = spec
            .inputs
            .declared
            .iter()
            .find(|declared| &declared.name == name)
        else {
            diagnostics.push(error(
                "unknown_selected_input",
                format!("selected input '{}' was not declared", name),
                Some(format!("input:{name}")),
            ));
            continue;
        };
        match input.kind {
            InputKindSpec::Integer if value.parse::<i64>().is_err() => diagnostics.push(error(
                "input_integer_invalid",
                format!("input '{}' expects an integer value, got '{}'", name, value),
                Some(format!("input:{name}")),
            )),
            InputKindSpec::Boolean
                if !matches!(
                    value.as_str(),
                    "1" | "0" | "true" | "false" | "yes" | "no" | "on" | "off"
                ) =>
            {
                diagnostics.push(error(
                    "input_boolean_invalid",
                    format!("input '{}' expects a boolean value, got '{}'", name, value),
                    Some(format!("input:{name}")),
                ))
            }
            InputKindSpec::Enum if !input.choices.iter().any(|choice| choice == value) => {
                diagnostics.push(error(
                    "input_enum_invalid",
                    format!(
                        "input '{}' expects one of [{}], got '{}'",
                        name,
                        input.choices.join(", "),
                        value
                    ),
                    Some(format!("input:{name}")),
                ))
            }
            _ => {}
        }
    }

    for unresolved in &spec.policy.interpolation.unresolved {
        let severity = if spec.policy.interpolation.allow_unresolved {
            DiagnosticSeverity::Warning
        } else {
            DiagnosticSeverity::Error
        };
        diagnostics.push(ValidationDiagnostic {
            severity,
            code: if spec.policy.interpolation.allow_unresolved {
                "interpolation_unresolved_allowed"
            } else {
                "interpolation_unresolved"
            },
            message: format!(
                "unresolved interpolation token '${{{}}}' remains in '{}'",
                unresolved.token, unresolved.location
            ),
            location: Some(unresolved.location.clone()),
        });
    }
}
