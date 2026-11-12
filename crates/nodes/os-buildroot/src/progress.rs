//! Progress of a Buildroot `make`, from its own output.
//!
//! Buildroot prints one `>>> <package> <version> <step>` line per package
//! step. A package has reached the target when its `Installing to target`
//! line is printed. The total is the packages `make show-build-order` lists,
//! one name a line, host packages left out. (`show-info` is one long JSON
//! line, and the step kit hands a tool's lines to its sink cut at 1 KiB.)

use std::collections::BTreeSet;

const TARGET_STEP: &str = "Installing to target";

/// The package named by a line that says it is installing to the target.
pub fn installed_to_target(line: &str) -> Option<&str> {
    let rest = line.strip_prefix(">>> ")?.trim_end();
    let rest = rest.strip_suffix(TARGET_STEP)?;
    rest.split_whitespace().next()
}

/// The target packages in `make show-build-order`'s output: every name that
/// is not `host-...`.
pub fn target_packages(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.starts_with("host-"))
        .map(str::to_string)
        .collect()
}

/// How many of `targets` have been installed to the target.
pub fn done(installed: &BTreeSet<String>, targets: &BTreeSet<String>) -> u32 {
    u32::try_from(installed.intersection(targets).count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_of_an_installing_to_target_line() {
        assert_eq!(
            installed_to_target(">>> busybox 1.38.0 Installing to target"),
            Some("busybox")
        );
        assert_eq!(installed_to_target(">>> busybox 1.38.0 Building"), None);
        assert_eq!(
            installed_to_target("busybox 1.38.0 Installing to target"),
            None
        );
    }

    #[test]
    fn target_packages_leave_out_host_packages() {
        let output = "host-skeleton\nskeleton\nbusybox\nhost-m4\n";
        assert_eq!(
            target_packages(output).into_iter().collect::<Vec<_>>(),
            vec!["busybox".to_string(), "skeleton".to_string()]
        );
    }

    #[test]
    fn done_counts_only_target_packages_that_are_installed() {
        let targets: BTreeSet<String> = ["busybox", "skeleton"].map(String::from).into();
        let installed: BTreeSet<String> =
            ["busybox", "host-m4", "toolchain"].map(String::from).into();
        assert_eq!(done(&installed, &targets), 1);
    }
}
