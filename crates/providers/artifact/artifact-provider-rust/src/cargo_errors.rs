//! Reading cargo's own error output: which packages failed to compile, so a
//! failed batch can be attributed to the artifacts it names.

/// The packages cargo names as failed to compile: "could not compile `name`".
pub(crate) fn cargo_failing_packages(message: &str) -> Vec<String> {
    const MARKER: &str = "could not compile `";
    message
        .match_indices(MARKER)
        .filter_map(|(index, _)| {
            message[index + MARKER.len()..]
                .split('`')
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(test)]
    #[test]
    fn cargo_names_the_packages_that_failed_to_compile() {
        let message = "cargo build for packages 'a', 'b' failed: \
            error[E0425]: cannot find value `x`\n\
            error: could not compile `b` (bin \"b\") due to 1 previous error\n\
            error: could not compile `dep-x` (lib)";
        assert_eq!(cargo_failing_packages(message), vec!["b", "dep-x"]);
        assert!(cargo_failing_packages("spurious network error").is_empty());
    }
}
