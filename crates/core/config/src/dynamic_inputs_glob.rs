pub(super) fn glob_matches(pattern: &str, value: &str) -> bool {
    glob_matches_bytes(pattern.as_bytes(), value.as_bytes())
}

fn glob_matches_bytes(pattern: &[u8], value: &[u8]) -> bool {
    let mut pattern_index = 0usize;
    let mut value_index = 0usize;
    let mut star_index = None;
    let mut star_value_index = 0usize;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    pattern[pattern_index..].iter().all(|byte| *byte == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matching_keeps_star_question_and_exact_semantics() {
        assert!(glob_matches("release/*", "release/v2026.4.0"));
        assert!(glob_matches("v2026.?.*", "v2026.4.0"));
        assert!(glob_matches("exact", "exact"));
        assert!(glob_matches("*rc*", "v2026.4.0-rc1"));
        assert!(!glob_matches("release/?", "release/v2026"));
        assert!(!glob_matches("exact", "exactly"));
    }

    #[test]
    fn glob_matching_handles_many_wildcards_without_recursion() {
        let pattern = "*a".repeat(512) + "*z";
        let value = "a".repeat(512) + "z";

        assert!(glob_matches(&pattern, &value));
    }
}
