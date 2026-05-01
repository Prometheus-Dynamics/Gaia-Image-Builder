use gaia_spec::ResolvedBuildSpec;

use crate::masking::{mask_pairs, mask_value};
use crate::model::{PrecedenceLayerReport, SelectionReport};
use crate::state::rollback_domains;

pub fn render_selection(spec: &ResolvedBuildSpec) -> SelectionReport {
    SelectionReport {
        requested_build: spec.selection.requested_build.clone(),
        selected_build_file: spec.selection.selected_build_file.clone(),
        selected_preset: spec.selection.selected_preset.clone(),
        selected_inputs: mask_pairs(&spec.selection.selected_inputs, &spec.reporting),
        selected_env_files: spec.selection.env_files.clone(),
        selected_env_overrides: mask_pairs(&spec.selection.env_overrides, &spec.reporting),
        explicit_overrides: spec
            .selection
            .explicit_overrides
            .iter()
            .map(|(key, value)| {
                if let Some(env_key) = key.strip_prefix("env.") {
                    (key.clone(), mask_value(env_key, value, &spec.reporting))
                } else {
                    (key.clone(), value.clone())
                }
            })
            .collect(),
        rollback_on_error: spec.policy.failure.rollback_on_error,
        preserve_failed_outputs: spec.policy.failure.preserve_failed_outputs,
        rollback_domains: rollback_domains(spec),
        precedence_order: spec.selection.precedence_order.clone(),
        precedence_layers: spec
            .policy
            .precedence
            .layers
            .iter()
            .map(|layer| PrecedenceLayerReport {
                source: format!("{:?}", layer.source),
                applies_to: layer
                    .applies_to
                    .iter()
                    .map(|target| format!("{:?}", target))
                    .collect(),
            })
            .collect(),
    }
}
