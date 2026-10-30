//! `[[variants]]` entries as written in a build file.

use serde::Deserialize;

/// `[[variants]]` entry: an `id`, an optional description and the
/// `overrides` (`key = value` pairs in `--set` form) that define it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct RawVariantConfig {
    pub id: String,
    pub description: Option<String>,
    /// `preset = "name"`: a preset applied for this variant, like `--preset`.
    pub preset: Option<String>,
    /// `presets = ["a", "b"]`: several presets, applied in this order after
    /// `preset`. They replace the selected preset for this variant.
    pub presets: Vec<String>,
    pub overrides: Vec<(String, String)>,
}
