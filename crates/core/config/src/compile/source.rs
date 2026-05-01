use super::*;

pub(crate) fn compile_source_refresh_policy(
    raw: Option<RawSourceRefreshPolicy>,
    default: SourceRefreshPolicySpec,
) -> SourceRefreshPolicySpec {
    match raw {
        Some(RawSourceRefreshPolicy::Auto) => SourceRefreshPolicySpec::Auto,
        Some(RawSourceRefreshPolicy::Always) => SourceRefreshPolicySpec::Always,
        Some(RawSourceRefreshPolicy::Never) => SourceRefreshPolicySpec::Never,
        None => default,
    }
}

pub(crate) fn compile_source_pin_policy(
    raw: Option<RawSourcePinPolicy>,
    default: SourcePinPolicySpec,
) -> SourcePinPolicySpec {
    match raw {
        Some(RawSourcePinPolicy::Floating) => SourcePinPolicySpec::Floating,
        Some(RawSourcePinPolicy::Locked) => SourcePinPolicySpec::Locked,
        None => default,
    }
}
