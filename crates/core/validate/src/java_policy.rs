use gaia_spec::{CommandProviderPolicySpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};

/// `[providers.java]` settings. Compilation falls back to the default for an
/// unknown value, so the mistake is reported here.
pub(crate) fn validate_java_policy(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if let Some(text) = &spec.policy.providers.java.gradle_home_invalid {
        diagnostics.push(error(
            "java_gradle_home_invalid",
            format!("providers.java.gradle_home '{text}': expected 'workspace' or 'user-cache'"),
            Some("providers.java.gradle_home".into()),
        ));
    }

    // `gradle_home` is read only from `[providers.java]`. The other command
    // provider tables share the same struct, so the key is accepted there
    // and would do nothing; say so.
    let providers = &spec.policy.providers;
    let others: [(&str, &CommandProviderPolicySpec); 7] = [
        ("archive", &providers.archive),
        ("download", &providers.download),
        ("go", &providers.go),
        ("node", &providers.node),
        ("python", &providers.python),
        ("buildroot", &providers.buildroot),
        ("starting_point", &providers.starting_point),
    ];
    for (name, policy) in others {
        if policy.gradle_home_configured {
            diagnostics.push(warning(
                "provider_gradle_home_ignored",
                format!(
                    "providers.{name}.gradle_home is ignored: gradle_home applies only to \
                     [providers.java]"
                ),
                Some(format!("providers.{name}.gradle_home")),
            ));
        }
    }
}
