//! "Did you mean" hints for misspelled keys and ids.

/// The Levenshtein distance between `a` and `b`, in characters.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b = b.chars().collect::<Vec<_>>();
    let mut previous = (0..=b.len()).collect::<Vec<_>>();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

/// `" (did you mean 'a', 'b'?)"` for the three closest of `scored`
/// (`(distance, name)` pairs), or nothing when it is empty.
pub fn did_you_mean_hint(mut scored: Vec<(usize, &str)>) -> String {
    scored.sort();
    scored.truncate(3);
    if scored.is_empty() {
        return String::new();
    }
    let names = scored
        .iter()
        .map(|(_, name)| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" (did you mean {names}?)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("abc", "ab"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn the_hint_names_the_three_closest() {
        assert_eq!(did_you_mean_hint(Vec::new()), "");
        assert_eq!(
            did_you_mean_hint(vec![(2, "d"), (1, "b"), (1, "a"), (3, "c")]),
            " (did you mean 'a', 'b', 'd'?)"
        );
    }
}
