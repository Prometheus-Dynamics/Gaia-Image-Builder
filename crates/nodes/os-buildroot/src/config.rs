//! Config fragments: how a Buildroot `.config` is made from descriptions.
//!
//! The base is what Buildroot itself gives for `allnoconfig`. Each fragment
//! is text of `BR2_...=value` lines and `# BR2_... is not set` lines, from a
//! description. Fragments are applied in the order given; a later line for a
//! symbol replaces an earlier one for it, `is not set` included. The merged
//! file is then resolved by Buildroot's own `olddefconfig`. No defconfig is
//! written or edited by hand.

use crate::ConfigFragment;

/// The symbol a config line sets or unsets, `None` for a blank or comment
/// line, and an error for anything else.
fn symbol(line: &str) -> Result<Option<&str>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    if let Some(rest) = line.strip_prefix("# ") {
        if let Some(name) = rest.strip_suffix(" is not set") {
            return check(name).map(Some);
        }
        return Ok(None);
    }
    match line.split_once('=') {
        Some((name, _)) => check(name).map(Some),
        None => Err(format!("{line:?} is not NAME=value or # NAME is not set")),
    }
}

/// A Buildroot symbol: the `BR2_` prefix, then letters, digits and
/// underscores (`BR2_aarch64` is one).
fn check(name: &str) -> Result<&str, String> {
    let valid =
        name.starts_with("BR2_") && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if valid {
        Ok(name)
    } else {
        Err(format!("{name:?} is not a BR2_ symbol"))
    }
}

/// `base` with every fragment applied in order. Each line of a fragment
/// drops the lines of the same symbol before it, then is added. The result
/// ends with a newline.
pub fn merge(base: &str, fragments: &[ConfigFragment]) -> Result<String, String> {
    let mut lines: Vec<String> = base.lines().map(str::to_string).collect();
    for fragment in fragments {
        for (number, line) in fragment.text.lines().enumerate() {
            let name = match symbol(line) {
                Ok(Some(name)) => name.to_string(),
                Ok(None) => continue,
                Err(why) => {
                    return Err(format!(
                        "fragment {:?}, line {}: {why}",
                        fragment.name,
                        number + 1
                    ));
                }
            };
            lines.retain(|kept| !matches_symbol(kept, &name));
            lines.push(line.trim().to_string());
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    Ok(text)
}

/// `text` with each `@name@` of an input tree replaced by its path. An
/// `@...@` of letters, digits, `.`, `-` and `_` that names no input is an
/// error, so a misspelled name is not left for Buildroot to find.
pub fn substitute(text: &str, inputs: &[(String, std::path::PathBuf)]) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('@') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name_end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
            .unwrap_or(after.len());
        if name_end > 0 && after[name_end..].starts_with('@') {
            let name = &after[..name_end];
            let Some((_, path)) = inputs.iter().find(|(n, _)| n == name) else {
                return Err(format!("@{name}@ names no input tree"));
            };
            out.push_str(&path.display().to_string());
            rest = &after[name_end + 1..];
        } else {
            out.push('@');
            rest = after;
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn matches_symbol(line: &str, name: &str) -> bool {
    matches!(symbol(line), Ok(Some(found)) if found == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(name: &str, text: &str) -> ConfigFragment {
        ConfigFragment {
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn a_later_fragment_replaces_an_earlier_line_for_the_same_symbol() {
        let base = "BR2_aarch64=y\n# BR2_PACKAGE_BUSYBOX is not set\n";
        let merged = merge(
            base,
            &[
                fragment("a", "BR2_PACKAGE_BUSYBOX=y\n"),
                fragment("b", "BR2_TARGET_GENERIC_HOSTNAME=\"one\"\n"),
                fragment("c", "BR2_TARGET_GENERIC_HOSTNAME=\"two\"\n"),
            ],
        )
        .expect("merge");
        assert_eq!(
            merged,
            "BR2_aarch64=y\nBR2_PACKAGE_BUSYBOX=y\nBR2_TARGET_GENERIC_HOSTNAME=\"two\"\n"
        );
    }

    #[test]
    fn an_unset_line_replaces_a_set_one() {
        let merged = merge(
            "BR2_PACKAGE_BUSYBOX=y\n",
            &[fragment("off", "# BR2_PACKAGE_BUSYBOX is not set\n")],
        )
        .expect("merge");
        assert_eq!(merged, "# BR2_PACKAGE_BUSYBOX is not set\n");
    }

    #[test]
    fn blank_and_comment_lines_are_dropped() {
        let merged =
            merge("", &[fragment("c", "# a comment\n\n  BR2_aarch64=y  \n")]).expect("merge");
        assert_eq!(merged, "BR2_aarch64=y\n");
    }

    #[test]
    fn an_empty_fragment_list_keeps_the_base() {
        assert_eq!(
            merge("BR2_aarch64=y", &[]).expect("merge"),
            "BR2_aarch64=y\n"
        );
    }

    #[test]
    fn an_input_name_becomes_its_path() {
        let inputs = vec![
            ("zero.linux".to_string(), std::path::PathBuf::from("/s/objects/aa")),
            ("files.0".to_string(), std::path::PathBuf::from("/s/objects/bb")),
        ];
        let text = "BR2_X=\"@files.0@/a.fragment @files.0@/b\"\nBR2_Y=\"me@host\"\n";
        assert_eq!(
            substitute(text, &inputs).expect("substituted"),
            "BR2_X=\"/s/objects/bb/a.fragment /s/objects/bb/b\"\nBR2_Y=\"me@host\"\n"
        );
        assert_eq!(
            substitute("BR2_X=\"@zero.linux@\"", &inputs).expect("one"),
            "BR2_X=\"/s/objects/aa\""
        );
    }

    #[test]
    fn a_name_that_is_no_input_is_an_error() {
        let why = substitute("BR2_X=\"@nothing@/f\"", &[]).expect_err("unknown");
        assert_eq!(why, "@nothing@ names no input tree");
    }

    #[test]
    fn a_line_that_is_not_a_symbol_is_an_error_naming_it() {
        let why =
            merge("", &[fragment("bad", "BR2_aarch64=y\nCONFIG_X=y\n")]).expect_err("bad line");
        assert_eq!(
            why,
            "fragment \"bad\", line 2: \"CONFIG_X\" is not a BR2_ symbol"
        );
        let why = merge("", &[fragment("bad", "just words\n")]).expect_err("bad line");
        assert!(why.contains("is not NAME=value"), "{why}");
    }
}
