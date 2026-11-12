//! `gaia.os.buildroot.rebuild-reason`: the config options that force a clean
//! build, named.
//!
//! A config change is a new key, so Buildroot's output is made again from
//! nothing (the output is a fresh directory; nothing of an earlier build is
//! reused). That is correct, but a run that rebuilds everything should say
//! why. This node compares the fragments of the earlier build with the
//! current ones and names each option whose value changed, `BR2_X "a" -> "b"`.
//!
//! The values are read the way Buildroot's merge reads them: a later line for
//! a symbol replaces an earlier one, and `# BR2_X is not set` is the value
//! `unset`. A symbol that neither build sets is `unset` in both.

use daedalus::prelude::*;
use gaia_os_buildroot::ConfigFragment;
use gaia_step::StepError;
use std::collections::{BTreeMap, BTreeSet};

const NAME: &str = "gaia.os.buildroot.rebuild-reason";
const UNSET: &str = "unset";

/// The value each symbol ends at, after `fragments` in order.
pub fn settings(fragments: &[ConfigFragment]) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for fragment in fragments {
        for (number, line) in fragment.text.lines().enumerate() {
            let line = line.trim();
            let parsed = if let Some(rest) = line.strip_prefix("# ") {
                rest.strip_suffix(" is not set").map(|name| (name, UNSET))
            } else if line.is_empty() || line.starts_with('#') {
                None
            } else {
                line.split_once('=')
            };
            let Some((name, value)) = parsed else {
                continue;
            };
            let valid = name.starts_with("BR2_")
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if !valid {
                return Err(format!(
                    "fragment {:?}, line {}: {name:?} is not a BR2_ symbol",
                    fragment.name,
                    number + 1
                ));
            }
            values.insert(name.to_string(), value.to_string());
        }
    }
    Ok(values)
}

/// The config options that differ between `previous` and `current`, one
/// line each, sorted by option. Empty when no option changed.
///
/// With no previous fragments there is no earlier build, and the one reason
/// is that. When the options are the same but a fragment's text changed,
/// the reason names that fragment: its key changed, so the build is still
/// clean.
pub fn reasons(
    previous: &[ConfigFragment],
    current: &[ConfigFragment],
) -> Result<Vec<String>, String> {
    if previous.is_empty() {
        return Ok(vec!["no earlier build of this output".into()]);
    }
    let before = settings(previous)?;
    let after = settings(current)?;
    let symbols: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let mut lines = Vec::new();
    for symbol in symbols {
        let old = before.get(symbol).map_or(UNSET, String::as_str);
        let new = after.get(symbol).map_or(UNSET, String::as_str);
        if old != new {
            lines.push(format!("{symbol} {old} -> {new}"));
        }
    }
    if lines.is_empty() {
        let text_of = |fragments: &[ConfigFragment]| {
            fragments
                .iter()
                .map(|fragment| (fragment.name.clone(), fragment.text.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let (before, after) = (text_of(previous), text_of(current));
        for (name, text) in &after {
            if before.get(name) != Some(text) {
                lines.push(format!("fragment {name} changed, no option value changed"));
            }
        }
    }
    Ok(lines)
}

/// The reasons a config change forces a clean build, as a Buildroot node
/// output: one line per option, empty when nothing forces one.
#[node(
    id = "gaia.os.buildroot.rebuild-reason",
    inputs(previous, current),
    outputs(reasons)
)]
pub fn os_buildroot_rebuild_reason(
    previous: &[ConfigFragment],
    current: &[ConfigFragment],
) -> Result<Vec<String>, StepError> {
    reasons(previous, current).map_err(|why| StepError::body(NAME, why))
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
    fn the_option_that_changed_is_named_with_its_old_and_new_value() {
        let before = [
            fragment("base", "BR2_aarch64=y\n"),
            fragment("host", "BR2_TARGET_GENERIC_HOSTNAME=\"one\"\n"),
        ];
        let after = [
            fragment("base", "BR2_aarch64=y\n"),
            fragment("host", "BR2_TARGET_GENERIC_HOSTNAME=\"two\"\n"),
        ];
        assert_eq!(
            reasons(&before, &after).expect("reasons"),
            vec!["BR2_TARGET_GENERIC_HOSTNAME \"one\" -> \"two\"".to_string()]
        );
    }

    #[test]
    fn a_later_fragment_decides_the_value_and_unset_is_a_value() {
        let before = [fragment("a", "BR2_PACKAGE_BUSYBOX=y\n")];
        let after = [
            fragment("a", "BR2_PACKAGE_BUSYBOX=y\n"),
            fragment("off", "# BR2_PACKAGE_BUSYBOX is not set\n"),
        ];
        assert_eq!(
            reasons(&before, &after).expect("reasons"),
            vec!["BR2_PACKAGE_BUSYBOX y -> unset".to_string()]
        );
    }

    #[test]
    fn a_repeated_setting_with_the_same_value_is_no_option_change() {
        let before = [fragment("a", "BR2_x=y\nBR2_x=y\n")];
        let after = [fragment("a", "BR2_x=y\n")];
        assert_eq!(
            settings(&before).expect("before"),
            settings(&after).expect("after")
        );
        // The text changed, so the key did: the fragment is named, no option is.
        assert_eq!(
            reasons(&before, &after).expect("reasons"),
            vec!["fragment a changed, no option value changed".to_string()]
        );
    }

    #[test]
    fn a_text_change_with_the_same_options_names_its_fragment() {
        let before = [fragment("note", "# a comment\nBR2_aarch64=y\n")];
        let after = [fragment("note", "# another comment\nBR2_aarch64=y\n")];
        assert_eq!(
            reasons(&before, &after).expect("reasons"),
            vec!["fragment note changed, no option value changed".to_string()]
        );
    }

    #[test]
    fn no_earlier_build_is_its_own_reason() {
        assert_eq!(
            reasons(&[], &[fragment("a", "BR2_x=y\n")]).expect("reasons"),
            vec!["no earlier build of this output".to_string()]
        );
    }

    #[test]
    fn a_line_that_is_not_a_br2_symbol_is_an_error() {
        let why = settings(&[fragment("bad", "CONFIG_X=y\n")]).expect_err("bad");
        assert!(why.contains("is not a BR2_ symbol"), "{why}");
    }
}
