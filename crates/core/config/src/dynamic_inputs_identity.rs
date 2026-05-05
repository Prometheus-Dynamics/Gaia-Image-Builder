use crate::raw::{
    RawInputChoicesErrorMode, RawInputChoicesFromConfig, RawInputChoicesFromKind,
    RawInputChoicesRefresh, RawInputChoicesSort, RawInputVersionScheme,
};

pub(crate) fn dynamic_choices_identity_key(
    config: &RawInputChoicesFromConfig,
    repo: &str,
) -> String {
    let mut identity = DynamicChoicesIdentity::default();
    identity.field("kind", choices_kind_name(config.kind));
    identity.field("repo", repo);
    identity.optional_field("source", config.source.as_deref());
    identity.optional_field("url", config.url.as_deref());
    identity.optional_field("json_path", config.json_path.as_deref());
    identity.fields("command", &config.command);
    identity.optional_field("pattern", config.pattern.as_deref());
    identity.fields("include", &config.include);
    identity.fields("exclude", &config.exclude);
    identity.optional_field("strip_prefix", config.strip_prefix.as_deref());
    identity.optional_field("display_template", config.display_template.as_deref());
    identity.optional_field(
        "selected_value_template",
        config.selected_value_template.as_deref(),
    );
    identity.optional_usize("limit", config.limit);
    identity.field("sort", choices_sort_name(config.sort));
    identity.bool_field("prefer_stable", config.prefer_stable);
    identity.field("refresh", choices_refresh_name(config.refresh));
    // TTL affects freshness, not which choices a source definition represents.
    identity.fields("fallback_choices", &config.fallback_choices);
    identity.field("version_scheme", version_scheme_name(config.version_scheme));
    identity.bool_field("allow_empty", config.allow_empty);
    identity.field("on_error", choices_error_mode_name(config.on_error));
    identity.optional_field("auth_env", config.auth_env.as_deref());
    identity.bool_field("credential_helper", config.credential_helper);
    identity.bool_field("include_prereleases", config.include_prereleases);
    identity.bool_field("include_drafts", config.include_drafts);
    format!("v2-{}", stable_hash_hex(identity.as_bytes()))
}

#[derive(Default)]
struct DynamicChoicesIdentity {
    bytes: Vec<u8>,
}

impl DynamicChoicesIdentity {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn field(&mut self, name: &str, value: &str) {
        self.write_name(name);
        self.write_value(value);
    }

    fn optional_field(&mut self, name: &str, value: Option<&str>) {
        self.write_name(name);
        match value {
            Some(value) => {
                self.bytes.push(b'1');
                self.write_value(value);
            }
            None => self.bytes.push(b'0'),
        }
    }

    fn fields(&mut self, name: &str, values: &[String]) {
        self.write_name(name);
        self.write_value(&values.len().to_string());
        for value in values {
            self.write_value(value);
        }
    }

    fn optional_usize(&mut self, name: &str, value: Option<usize>) {
        self.optional_field(name, value.map(|value| value.to_string()).as_deref());
    }

    fn bool_field(&mut self, name: &str, value: bool) {
        self.field(name, if value { "true" } else { "false" });
    }

    fn write_name(&mut self, name: &str) {
        self.write_value(name);
    }

    fn write_value(&mut self, value: &str) {
        self.bytes
            .extend_from_slice(value.len().to_string().as_bytes());
        self.bytes.push(b':');
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(b';');
    }
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    const FNV_OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const FNV_PRIME: u128 = 0x0000000001000000000000000000013b;

    let hash = bytes.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u128::from(*byte)).wrapping_mul(FNV_PRIME)
    });
    format!("{hash:032x}")
}

fn choices_kind_name(kind: RawInputChoicesFromKind) -> &'static str {
    match kind {
        RawInputChoicesFromKind::GitTags => "git-tags",
        RawInputChoicesFromKind::GitBranches => "git-branches",
        RawInputChoicesFromKind::GithubReleases => "github-releases",
        RawInputChoicesFromKind::Json => "json",
        RawInputChoicesFromKind::Command => "command",
    }
}

fn choices_sort_name(sort: RawInputChoicesSort) -> &'static str {
    match sort {
        RawInputChoicesSort::LexicalAsc => "lexical-asc",
        RawInputChoicesSort::LexicalDesc => "lexical-desc",
        RawInputChoicesSort::VersionAsc => "version-asc",
        RawInputChoicesSort::VersionDesc => "version-desc",
        RawInputChoicesSort::PublishedDesc => "published-desc",
        RawInputChoicesSort::PublishedAsc => "published-asc",
    }
}

fn choices_refresh_name(refresh: RawInputChoicesRefresh) -> &'static str {
    match refresh {
        RawInputChoicesRefresh::Auto => "auto",
        RawInputChoicesRefresh::Always => "always",
        RawInputChoicesRefresh::Never => "never",
    }
}

fn version_scheme_name(version_scheme: RawInputVersionScheme) -> &'static str {
    match version_scheme {
        RawInputVersionScheme::Semver => "semver",
        RawInputVersionScheme::Versionish => "versionish",
    }
}

fn choices_error_mode_name(on_error: RawInputChoicesErrorMode) -> &'static str {
    match on_error {
        RawInputChoicesErrorMode::Fail => "fail",
        RawInputChoicesErrorMode::Warn => "warn",
        RawInputChoicesErrorMode::Ignore => "ignore",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_choices_identity_key_is_versioned_and_stable() {
        let config = RawInputChoicesFromConfig {
            kind: RawInputChoicesFromKind::Command,
            command: vec!["printf".into(), "release\n".into()],
            display_template: Some("profile ${choice}".into()),
            selected_value_template: Some("${choice}".into()),
            sort: RawInputChoicesSort::LexicalAsc,
            ..RawInputChoicesFromConfig::default()
        };

        let key = dynamic_choices_identity_key(&config, "");

        assert!(key.starts_with("v2-"), "{key}");
        assert_eq!(key.len(), 35);
        assert_eq!(key, "v2-a8d2cd6827073bfede5469c9d8fef354");
    }
}
