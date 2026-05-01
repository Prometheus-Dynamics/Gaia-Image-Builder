use super::*;

pub(crate) fn selection_precedence_order(raw: &RawBuildConfig) -> Vec<String> {
    precedence_layers(raw)
        .into_iter()
        .map(|layer| format!("{:?}", layer.source))
        .collect()
}

pub(crate) fn precedence_layers(raw: &RawBuildConfig) -> Vec<PrecedenceLayerSpec> {
    let mut layers = vec![PrecedenceLayerSpec {
        source: PrecedenceSource::ConfigDefaults,
        applies_to: vec![
            PrecedenceTarget::PresetSelection,
            PrecedenceTarget::Environment,
            PrecedenceTarget::Interpolation,
            PrecedenceTarget::Metadata,
            PrecedenceTarget::Provenance,
            PrecedenceTarget::Workspace,
            PrecedenceTarget::ImageOutput,
            PrecedenceTarget::Selection,
        ],
    }];

    if raw.preset.is_some() {
        layers.push(PrecedenceLayerSpec {
            source: PrecedenceSource::SelectedPreset,
            applies_to: vec![
                PrecedenceTarget::PresetSelection,
                PrecedenceTarget::Environment,
                PrecedenceTarget::Interpolation,
                PrecedenceTarget::Metadata,
                PrecedenceTarget::Provenance,
                PrecedenceTarget::Workspace,
                PrecedenceTarget::ImageOutput,
                PrecedenceTarget::Selection,
            ],
        });
    }
    if !raw.env_files.is_empty() {
        layers.push(PrecedenceLayerSpec {
            source: PrecedenceSource::EnvFiles,
            applies_to: vec![PrecedenceTarget::Environment, PrecedenceTarget::Selection],
        });
    }
    if !raw.env.is_empty() {
        layers.push(PrecedenceLayerSpec {
            source: PrecedenceSource::InlineEnv,
            applies_to: vec![PrecedenceTarget::Environment],
        });
    }
    layers.push(PrecedenceLayerSpec {
        source: PrecedenceSource::ProcessEnv,
        applies_to: vec![PrecedenceTarget::Environment],
    });
    if !raw.env_overrides.is_empty() {
        layers.push(PrecedenceLayerSpec {
            source: PrecedenceSource::CliEnvOverrides,
            applies_to: vec![PrecedenceTarget::Environment, PrecedenceTarget::Selection],
        });
    }
    if !raw.explicit_overrides.is_empty() {
        layers.push(PrecedenceLayerSpec {
            source: PrecedenceSource::CliSetOverrides,
            applies_to: vec![
                PrecedenceTarget::Interpolation,
                PrecedenceTarget::Metadata,
                PrecedenceTarget::Provenance,
                PrecedenceTarget::Workspace,
                PrecedenceTarget::ImageOutput,
                PrecedenceTarget::Selection,
            ],
        });
    }

    layers
}
