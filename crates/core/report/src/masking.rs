pub fn mask_pairs(
    pairs: &[(String, String)],
    reporting: &gaia_spec::ReportingSpec,
) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.clone(), mask_value(key, value, reporting)))
        .collect()
}

pub fn mask_value(key: &str, value: &str, reporting: &gaia_spec::ReportingSpec) -> String {
    if !reporting.masking.enabled {
        return value.to_string();
    }
    let key_upper = key.to_ascii_uppercase();
    if reporting
        .masking
        .patterns
        .iter()
        .any(|pattern| !pattern.is_empty() && key_upper.contains(&pattern.to_ascii_uppercase()))
    {
        reporting.masking.replacement.clone()
    } else {
        value.to_string()
    }
}
