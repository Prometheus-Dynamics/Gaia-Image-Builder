//! The parts of the kernel-module node that need no tool or store: module
//! names, the release a kernel build is for, vermagic and placement.

/// The directory under `lib/modules/<release>/` that the new modules go in.
/// It is the one `depmod` searches after the kernel's own tree.
pub const UPDATES_DIR: &str = "updates";

/// A module name, the stem of the `.ko` Kbuild builds: letters, digits, `-`
/// and `_`, starting with a letter or digit, and no path in it.
pub fn valid_name(name: &str) -> Result<(), String> {
    let first_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if first_ok && rest_ok {
        Ok(())
    } else {
        Err(format!(
            "module name {name:?} must be letters, digits, '-' or '_', starting with a letter or digit"
        ))
    }
}

/// The release a kernel build is for: the `UTS_RELEASE` in its
/// `include/generated/utsrelease.h`. That is the string the module's vermagic
/// is built from, so it is the one to compare.
pub fn release_of_header(header: &str) -> Result<String, String> {
    for line in header.lines() {
        let Some(rest) = line.trim().strip_prefix("#define UTS_RELEASE") else {
            continue;
        };
        let value = rest
            .trim()
            .strip_prefix('"')
            .and_then(|text| text.split('"').next());
        if let Some(release) = value.filter(|release| !release.is_empty()) {
            return Ok(release.to_string());
        }
    }
    Err("the kernel build's include/generated/utsrelease.h has no UTS_RELEASE".into())
}

/// The release a `modinfo -F vermagic` line names: its first word.
pub fn vermagic_release(vermagic: &str) -> Option<&str> {
    vermagic.split_whitespace().next()
}

/// The one release directory in a `lib/modules` tree, given its entry names.
pub fn single_release(names: &[String]) -> Result<String, String> {
    match names {
        [only] => Ok(only.clone()),
        [] => Err("the kernel modules have no release directory".into()),
        many => Err(format!(
            "the kernel modules have {} release directories ({}), expected one",
            many.len(),
            many.join(", ")
        )),
    }
}

/// Where a built module is placed, relative to `lib/modules`.
pub fn placed_path(release: &str, name: &str) -> String {
    format!("{release}/{UPDATES_DIR}/{name}.ko")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names_are_plain_stems() {
        assert!(valid_name("hello").is_ok());
        assert!(valid_name("wifi_8821-x").is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("-x").is_err());
        assert!(valid_name("a/b").is_err());
        assert!(valid_name("a.ko").is_err());
    }

    #[test]
    fn the_release_is_read_from_the_header() {
        let header = "/* comment */\n#define UTS_RELEASE \"7.2.9-200.nobara.fc44.x86_64\"\n";
        assert_eq!(
            release_of_header(header).expect("release"),
            "7.2.9-200.nobara.fc44.x86_64"
        );
        assert!(release_of_header("#define LINUX_VERSION_CODE 1\n").is_err());
        assert!(release_of_header("#define UTS_RELEASE \"\"\n").is_err());
    }

    #[test]
    fn vermagic_names_the_release_first() {
        assert_eq!(
            vermagic_release("7.2.9-200.nobara.fc44.x86_64 SMP preempt mod_unload "),
            Some("7.2.9-200.nobara.fc44.x86_64")
        );
        assert_eq!(vermagic_release("  \n"), None);
    }

    #[test]
    fn one_release_directory_is_required() {
        assert_eq!(single_release(&["6.1.0".into()]).expect("one"), "6.1.0");
        assert!(single_release(&[]).is_err());
        assert!(single_release(&["a".into(), "b".into()]).is_err());
    }

    #[test]
    fn modules_go_under_updates() {
        assert_eq!(placed_path("6.1.0", "hello"), "6.1.0/updates/hello.ko");
    }
}
