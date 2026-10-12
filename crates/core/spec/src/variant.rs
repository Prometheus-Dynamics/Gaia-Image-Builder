//! Build variants: one build file, several images that share a base.
//!
//! A `[[variants]]` entry names overrides applied on top of the base config
//! (the same `key = value` form as `--set`). Every resolved spec carries its
//! own variant id in `selection.variant`; state and runtime records of a
//! variant are keyed by it so variants never read each other's records.

/// The id of the base config when variants are declared.
pub const DEFAULT_VARIANT_ID: &str = "default";

/// A declared variant, as written in `[[variants]]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariantSpec {
    pub id: String,
    pub description: Option<String>,
    /// Presets this variant applies in place of the selected preset, in order.
    pub presets: Vec<String>,
    /// Overrides applied after the base config and presets, before `--set`.
    pub overrides: Vec<(String, String)>,
}

/// `[a-z0-9][a-z0-9-]*`: the id appears in file names and on the command line.
pub fn is_valid_variant_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `@<variant>` for a variant spec, empty for a build with no variants.
pub fn variant_name_suffix(variant: Option<&str>) -> String {
    variant
        .map(|variant| format!("@{variant}"))
        .unwrap_or_default()
}

/// The runtime records dir (install, stage and checkpoint state) of a build
/// or variant, relative to the out dir. The image feed reads every record in
/// it, so each variant has its own.
pub fn runtime_state_dir_name(variant: Option<&str>) -> String {
    format!(".gaia/runtime{}", variant_name_suffix(variant))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_ids_are_lowercase_words_with_dashes() {
        for valid in ["pv2026", "a", "0", "pv-2026", "x-1-y"] {
            assert!(is_valid_variant_id(valid), "{valid} should be valid");
        }
        for invalid in ["", "-a", "PV", "pv_2026", "pv 2026", "pv/2026", "é"] {
            assert!(!is_valid_variant_id(invalid), "{invalid} should be invalid");
        }
    }

    #[test]
    fn variant_scoped_names_only_change_when_a_variant_is_set() {
        assert_eq!(runtime_state_dir_name(None), ".gaia/runtime");
        assert_eq!(
            runtime_state_dir_name(Some("pv2026")),
            ".gaia/runtime@pv2026"
        );
        assert_eq!(variant_name_suffix(None), "");
        assert_eq!(variant_name_suffix(Some("default")), "@default");
    }
}
