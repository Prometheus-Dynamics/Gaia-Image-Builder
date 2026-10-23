//! Warnings for config layers that contribute nothing.

use super::*;

/// Warnings for loaded files (the entrypoint, its `extends` chain and its
/// imports) that set nothing at all: a layer that should provide something
/// but holds only comments.
pub(crate) fn collect_empty_layer_warnings(raw: &RawBuildConfig) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut visit = vec![raw];
    while let Some(config) = visit.pop() {
        if config.sets_nothing {
            let path = config
                .source_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<build config>".into());
            let warning = format!("config file '{path}' sets nothing (only comments)");
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        visit.extend(config.extends_config.as_deref());
        visit.extend(
            config
                .imported_configs
                .iter()
                .map(|imported| &imported.config),
        );
    }
    warnings
}
