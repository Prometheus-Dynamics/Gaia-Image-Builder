//! Detects `config_overrides` entries that Buildroot silently dropped or
//! changed.
//!
//! `olddefconfig` quietly resets every symbol whose `depends on` is not met,
//! and drops symbols that no longer exist. A build then "succeeds" without,
//! say, OpenJDK. After every config step, each requested override is
//! compared against the final `.config`; `[providers.buildroot]
//! override_check` decides whether a mismatch fails the operation (before
//! the long `make`), is reported as a warning, or is ignored.
use super::*;
use gaia_spec::BuildrootOverrideCheckSpec;

/// Prefix of the run messages that carry override check warnings; the
/// provider moves them into [`ImageExecutionResult::warnings`].
pub(crate) const OVERRIDE_CHECK_WARNING_PREFIX: &str = "warning: buildroot config_overrides: ";

/// Settings Gaia itself rewrites after the user's overrides (download and
/// compiler cache locations), so a difference there is intended.
const GAIA_MANAGED_SETTINGS: &[&str] = &["BR2_DL_DIR", "BR2_CCACHE_DIR"];

/// One requested `config_overrides` entry the final `.config` does not honor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OverrideMismatch {
    pub(crate) key: String,
    pub(crate) requested: String,
    /// `None` when the symbol is absent from `.config`; `Some("n")` for a
    /// `# KEY is not set` line.
    pub(crate) actual: Option<String>,
}

impl OverrideMismatch {
    fn dropped(&self) -> bool {
        matches!(self.actual.as_deref(), None | Some("n"))
    }

    fn describe(&self) -> String {
        let actual = match self.actual.as_deref() {
            None => "missing from .config (unknown symbol or unmet dependency)".to_string(),
            Some("n") => "is not set".to_string(),
            Some(value) => value.to_string(),
        };
        let verb = if self.dropped() { "dropped" } else { "changed" };
        format!(
            "{} {verb}: requested {}={}, final {actual}",
            self.key, self.key, self.requested
        )
    }
}

/// Compares the requested overrides with the final `.config` in
/// `output_dir` and applies `policy`. Returns run messages (warnings carry
/// [`OVERRIDE_CHECK_WARNING_PREFIX`]).
pub(crate) fn check_buildroot_config_overrides(
    spec: &ResolvedBuildSpec,
    output_dir: &Path,
    overrides: &[(String, String)],
    policy: BuildrootOverrideCheckSpec,
) -> Result<Vec<String>, ImageProviderError> {
    if policy == BuildrootOverrideCheckSpec::Off || overrides.is_empty() {
        return Ok(Vec::new());
    }
    let config_path = output_dir.join(".config");
    let Ok(config) = fs::read_to_string(&config_path) else {
        return Ok(Vec::new());
    };
    let requested = normalize_buildroot_config_overrides(spec, overrides);
    let mismatches = find_override_mismatches(&config, &requested);
    if mismatches.is_empty() {
        return Ok(vec![format!(
            "verified {} buildroot config_overrides entr{} against the final .config",
            requested.len(),
            if requested.len() == 1 { "y" } else { "ies" }
        )]);
    }
    let keys = mismatches
        .iter()
        .map(|mismatch| mismatch.key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    for mismatch in &mismatches {
        tracing::warn!(
            key = %mismatch.key,
            requested = %mismatch.requested,
            actual = mismatch.actual.as_deref().unwrap_or("<missing>"),
            config = %config_path.display(),
            "buildroot config override not applied"
        );
    }
    if policy == BuildrootOverrideCheckSpec::Error {
        let mut message = format!(
            "buildroot dropped or changed {} config_overrides entr{} in '{}' after olddefconfig:\n",
            mismatches.len(),
            if mismatches.len() == 1 { "y" } else { "ies" },
            config_path.display()
        );
        for mismatch in &mismatches {
            message.push_str(&format!("  - {}\n", mismatch.describe()));
        }
        message.push_str(&format!(
            "hint: usually an unmet `depends on`; check menuconfig for {keys} \
             (search with `/` to see each symbol's dependencies)\n\
             set [providers.buildroot] override_check = \"warn\" to build anyway"
        ));
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            message,
        ));
    }
    Ok(mismatches
        .iter()
        .map(|mismatch| {
            format!(
                "{OVERRIDE_CHECK_WARNING_PREFIX}{}; usually an unmet `depends on`; \
                 check menuconfig for {}",
                mismatch.describe(),
                mismatch.key
            )
        })
        .collect())
}

/// Splits override check warnings out of a provider's run messages.
pub(crate) fn override_check_warnings(messages: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    for message in messages {
        if let Some(warning) = message.strip_prefix("warning: ")
            && message.starts_with(OVERRIDE_CHECK_WARNING_PREFIX)
            && !warnings.iter().any(|existing| existing == warning)
        {
            warnings.push(warning.to_string());
        }
    }
    warnings
}

/// Every requested entry (the last one wins for repeated keys) whose value
/// the final `.config` does not hold.
pub(crate) fn find_override_mismatches(
    config: &str,
    requested: &[(String, String)],
) -> Vec<OverrideMismatch> {
    let settings = parse_kconfig_settings(config);
    let mut effective = BTreeMap::new();
    let mut order = Vec::new();
    for (key, value) in requested {
        let key = key.trim();
        if GAIA_MANAGED_SETTINGS.contains(&key) {
            continue;
        }
        if effective.insert(key, value.trim()).is_none() {
            order.push(key);
        }
    }
    order
        .into_iter()
        .filter_map(|key| {
            let requested = effective[key];
            let actual = settings.get(key).map(String::as_str);
            (!override_satisfied(requested, actual)).then(|| OverrideMismatch {
                key: key.to_string(),
                requested: requested.to_string(),
                actual: actual.map(str::to_string),
            })
        })
        .collect()
}

/// `KEY=value` lines and `# KEY is not set` lines (as `n`).
fn parse_kconfig_settings(config: &str) -> BTreeMap<String, String> {
    let mut settings = BTreeMap::new();
    for line in config.lines() {
        let line = line.trim();
        if let Some(key) = line
            .strip_prefix("# ")
            .and_then(|rest| rest.strip_suffix(" is not set"))
        {
            settings.insert(key.trim().to_string(), "n".to_string());
        } else if !line.starts_with('#')
            && let Some((key, value)) = line.split_once('=')
        {
            settings.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    settings
}

fn override_satisfied(requested: &str, actual: Option<&str>) -> bool {
    // An absent symbol is effectively `n` (or an empty string): Kconfig does
    // not write symbols whose dependencies are unmet.
    if requested == "n" || unquote(requested).is_empty() {
        return match actual {
            None | Some("n") => true,
            Some(actual) => unquote(actual).is_empty(),
        };
    }
    let Some(actual) = actual else {
        return false;
    };
    if actual == "n" {
        return false;
    }
    let (requested, actual) = (unquote(requested), unquote(actual));
    if requested == actual {
        return true;
    }
    matches!(
        (parse_kconfig_number(requested), parse_kconfig_number(actual)),
        (Some(requested), Some(actual)) if requested == actual
    )
}

/// Kconfig writes string values quoted; overrides may omit the quotes.
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

/// `int` and `hex` symbols: `0x1F` equals `0x1f`, `010` equals `10`.
fn parse_kconfig_number(value: &str) -> Option<i128> {
    match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => i128::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    }
}
