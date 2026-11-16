//! The packages one `gaia.build.rust` step builds, and what each one gets.
//!
//! A step takes `package` as one name, or as several names separated by
//! commas (the sorted list a graph's group gives). Several packages are built
//! for one cargo invocation when cargo would compile exactly what each
//! package's own build compiles: that is, when no shared crate gets a
//! different feature set. Otherwise each package is built by its own cargo
//! run, in the same target directory, so a crate both need is compiled once
//! and built with each package's own features.
//!
//! This module holds the pure parts: the names, cargo's arguments, which
//! files of the profile directory belong to which package (`cargo metadata`),
//! and the feature comparison (`cargo tree`).

use std::collections::{BTreeMap, BTreeSet};

/// A crate as `cargo tree` names it: its name and its version without `v`.
pub(crate) type Crate = (String, String);

/// The feature names each crate has in one resolved build.
pub(crate) type Features = BTreeMap<Crate, BTreeSet<String>>;

/// One file a package's build leaves in the profile directory, and the
/// directory it goes to in the output (`bin` or `lib`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Built {
    pub(crate) file: String,
    pub(crate) sub: &'static str,
}

/// The package names a `package` setting gives, in the order given.
///
/// One name is the single-package build. Names are cargo package names:
/// letters, digits, `-` and `_`, none repeated.
pub(crate) fn names(package: &str) -> Result<Vec<&str>, String> {
    let names: Vec<&str> = package.split(',').collect();
    let mut seen = BTreeSet::new();
    for name in &names {
        let valid = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !valid {
            return Err(format!(
                "`{package}` is not a package name, nor a list of them"
            ));
        }
        if !seen.insert(*name) {
            return Err(format!("package `{name}` is in the list twice"));
        }
    }
    Ok(names)
}

/// The arguments of one cargo build: `--package` for each of `packages`,
/// `--features` (joined) when there are any. For one package this is the
/// argument list the single-package build has always had, in the same order.
pub(crate) fn build_args(
    profile: &str,
    triple: &str,
    packages: &[&str],
    features: &[&str],
    manifest: &str,
    target_dir: &str,
) -> Vec<String> {
    let mut args = vec![
        "build".to_string(),
        "--profile".into(),
        profile.into(),
        "--locked".into(),
        "--offline".into(),
    ];
    for package in packages {
        args.push("--package".into());
        args.push((*package).into());
    }
    args.extend(["--target".into(), triple.into()]);
    args.extend(["--manifest-path".into(), manifest.into()]);
    args.extend(["--target-dir".into(), target_dir.into()]);
    if !features.is_empty() {
        args.extend(["--features".into(), features.join(",")]);
    }
    args
}

/// The arguments of `cargo tree` for the features of `packages` (one build's
/// resolution), with the edges a build compiles: normal and build
/// dependencies, not dev dependencies.
pub(crate) fn tree_args(triple: &str, packages: &[&str], manifest: &str) -> Vec<String> {
    let mut args = vec![
        "tree".to_string(),
        "--locked".into(),
        "--offline".into(),
        "--target".into(),
        triple.into(),
        "--edges".into(),
        "normal,build,features".into(),
        "--prefix".into(),
        "none".into(),
        "--format".into(),
        "{p} {f}".into(),
        "--manifest-path".into(),
        manifest.into(),
    ];
    for package in packages {
        args.push("--package".into());
        args.push((*package).into());
    }
    args
}

/// The files `package` leaves in the profile directory, from `cargo metadata
/// --no-deps` (its JSON text): each binary target is a file of its own name;
/// each shared or static library target is `lib<crate>.so` or `.a`.
pub(crate) fn built_files(metadata: &str, package: &str) -> Result<Vec<Built>, String> {
    let value: serde_json::Value =
        serde_json::from_str(metadata).map_err(|e| format!("cargo metadata: {e}"))?;
    let packages = value
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("cargo metadata: no packages")?;
    let found = packages
        .iter()
        .find(|p| p.get("name").and_then(serde_json::Value::as_str) == Some(package))
        .ok_or_else(|| format!("cargo metadata: no package {package} in the workspace"))?;
    let mut files = Vec::new();
    for target in found
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = target
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let kinds: Vec<&str> = target
            .get("kind")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect();
        let crate_name = name.replace('-', "_");
        for kind in kinds {
            let built = match kind {
                "bin" => Built {
                    file: name.to_string(),
                    sub: "bin",
                },
                "cdylib" | "dylib" => Built {
                    file: format!("lib{crate_name}.so"),
                    sub: "lib",
                },
                "staticlib" => Built {
                    file: format!("lib{crate_name}.a"),
                    sub: "lib",
                },
                _ => continue,
            };
            if !files.contains(&built) {
                files.push(built);
            }
        }
    }
    files.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(files)
}

/// The features each crate has in a `cargo tree` listing made with the
/// format `{p} {f}` and `--prefix none`. Feature-edge lines (`name feature
/// "x"`) are skipped; a crate listed again (`(*)`) has the same features.
pub(crate) fn features_of(listing: &str) -> Features {
    let mut out: Features = BTreeMap::new();
    for line in listing.lines() {
        let Some((name, rest)) = line.split_once(' ') else {
            continue;
        };
        let (version, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        // A feature-edge line has `feature` where the version is.
        let Some(version) = version
            .strip_prefix('v')
            .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
        else {
            continue;
        };
        let mut rest = rest.trim();
        if let Some(stripped) = rest.strip_suffix("(*)") {
            rest = stripped.trim_end();
        }
        // The source in parentheses (a path, `proc-macro`) comes before the
        // features.
        if let Some(close) = rest.find(')').filter(|_| rest.starts_with('(')) {
            rest = rest[close + 1..].trim_start();
        }
        let features = out
            .entry((name.to_string(), version.to_string()))
            .or_default();
        features.extend(rest.split(',').filter(|f| !f.is_empty()).map(String::from));
    }
    out
}

/// The first crate whose features differ between a package's own build
/// (`solo`) and the build with others (`merged`), or `None` when none does.
/// A solo build with no crates gives a difference: nothing to compare.
pub(crate) fn first_difference(solo: &Features, merged: &Features) -> Option<String> {
    if solo.is_empty() {
        return Some("its build lists no crates".into());
    }
    for ((name, version), features) in solo {
        let shown = |set: &BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>().join(",");
        match merged.get(&(name.clone(), version.clone())) {
            None => return Some(format!("{name} {version} is not in the merged build")),
            Some(other) if other != features => {
                return Some(format!(
                    "{name} {version} has features [{}] alone and [{}] merged",
                    shown(features),
                    shown(other)
                ));
            }
            Some(_) => {}
        }
    }
    None
}

/// Why a group of packages cannot share one cargo run, or `None` when it
/// can. Cargo takes a feature list for one package only, so any feature
/// refuses the merge. Otherwise every crate each package's own build (`solo`,
/// by package) compiles must have the same features in `merged`, the build
/// of all of them together.
pub(crate) fn refusal(
    features: &[&str],
    solo: &[(&str, Features)],
    merged: &Features,
) -> Option<String> {
    if !features.is_empty() {
        return Some("the component sets features".into());
    }
    for (package, own) in solo {
        if let Some(why) = first_difference(own, merged) {
            return Some(format!("{package}: {why}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_name_is_one_package_and_a_list_is_many() {
        assert_eq!(names("lemnosd").expect("names"), ["lemnosd"]);
        assert_eq!(
            names("lemnosd,lemnos-ctl,lemnos_orion").expect("names"),
            ["lemnosd", "lemnos-ctl", "lemnos_orion"]
        );
    }

    #[test]
    fn a_bad_list_is_refused() {
        for bad in ["", "a,", ",a", "a,,b", "a b", "a/b", "a,a"] {
            assert!(names(bad).is_err(), "{bad:?} is accepted");
        }
    }

    #[test]
    fn one_package_gives_the_arguments_it_always_had() {
        let args = build_args(
            "service",
            "aarch64-unknown-linux-musl",
            &["lemnosd"],
            &[],
            "/src/Cargo.toml",
            "/build/target",
        );
        assert_eq!(
            args,
            [
                "build",
                "--profile",
                "service",
                "--locked",
                "--offline",
                "--package",
                "lemnosd",
                "--target",
                "aarch64-unknown-linux-musl",
                "--manifest-path",
                "/src/Cargo.toml",
                "--target-dir",
                "/build/target",
            ]
        );
    }

    #[test]
    fn features_come_last_and_as_one_flag() {
        let args = build_args("release", "t", &["a"], &["x", "y"], "/m", "/d");
        assert_eq!(&args[args.len() - 2..], ["--features", "x,y"]);
    }

    #[test]
    fn several_packages_are_one_package_flag_each() {
        let args = build_args("service", "t", &["lemnos-ctl", "lemnosd"], &[], "/m", "/d");
        let packages: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "--package")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(packages, ["lemnos-ctl", "lemnosd"]);
    }

    const METADATA: &str = r#"{"packages":[
      {"name":"lemnosd","targets":[{"name":"lemnosd","kind":["bin"]},{"name":"lemnosd_lib","kind":["lib"]}]},
      {"name":"lemnos-orion","targets":[{"name":"lemnos-orion","kind":["bin"]},{"name":"orion-ffi","kind":["cdylib"]},{"name":"orion-static","kind":["staticlib","rlib"]}]},
      {"name":"lemnos-slot-init","targets":[{"name":"lemnos-slot-init","kind":["bin"]}]}
    ],"workspace_members":[]}"#;

    fn file(file: &str, sub: &'static str) -> Built {
        Built {
            file: file.into(),
            sub,
        }
    }

    #[test]
    fn a_package_has_its_own_binaries_and_libraries_only() {
        assert_eq!(
            built_files(METADATA, "lemnosd").expect("metadata"),
            [file("lemnosd", "bin")]
        );
        assert_eq!(
            built_files(METADATA, "lemnos-orion").expect("metadata"),
            [
                file("lemnos-orion", "bin"),
                file("liborion_ffi.so", "lib"),
                file("liborion_static.a", "lib"),
            ]
        );
        assert!(built_files(METADATA, "nope").is_err());
        assert!(built_files("not json", "lemnosd").is_err());
    }

    const LISTING: &str = "lemnosd v2.0.1-dev (/src/service/lemnosd) default
lemnos-client feature \"default\"
lemnos-client v2.0.1-dev (/src/engine/client) default
serde v1.0.229 derive,serde_derive,std
serde_derive v1.0.229 (proc-macro) default
typenum v1.20.1 (*)
subtle v2.6.1
lemnos-slot-store v2.0.1-dev (/src/drivers/slot store) fake,linux (*)
";

    #[test]
    fn the_listing_gives_each_crates_features() {
        let features = features_of(LISTING);
        let key = |n: &str, v: &str| (n.to_string(), v.to_string());
        assert_eq!(features[&key("lemnosd", "2.0.1-dev")], set(&["default"]));
        assert_eq!(
            features[&key("serde", "1.0.229")],
            set(&["derive", "serde_derive", "std"])
        );
        assert_eq!(features[&key("serde_derive", "1.0.229")], set(&["default"]));
        assert_eq!(features[&key("typenum", "1.20.1")], set(&[]));
        assert_eq!(features[&key("subtle", "2.6.1")], set(&[]));
        assert_eq!(
            features[&key("lemnos-slot-store", "2.0.1-dev")],
            set(&["fake", "linux"])
        );
        assert_eq!(features.len(), 7, "feature lines add no crate");
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_same_features_in_both_builds_is_no_difference() {
        let solo = features_of(LISTING);
        assert_eq!(first_difference(&solo, &solo), None);
    }

    #[test]
    fn a_feature_the_merged_build_adds_is_a_difference() {
        let solo = features_of(LISTING);
        let merged = features_of(&LISTING.replace("fake,linux", "fake,linux,mock"));
        let found = first_difference(&solo, &merged).expect("a difference");
        assert!(found.contains("lemnos-slot-store"), "{found}");
        assert!(
            found.contains("[fake,linux]") && found.contains("[fake,linux,mock]"),
            "{found}"
        );
    }

    #[test]
    fn a_crate_the_merged_build_lacks_is_a_difference() {
        let solo = features_of(LISTING);
        let merged = features_of(&LISTING.replace("subtle v2.6.1\n", ""));
        let found = first_difference(&solo, &merged).expect("a difference");
        assert!(found.contains("subtle"), "{found}");
    }

    #[test]
    fn a_build_with_no_crates_is_never_the_same() {
        assert!(first_difference(&Features::new(), &Features::new()).is_some());
    }

    #[test]
    fn the_tree_args_take_normal_and_build_edges_for_each_package() {
        let args = tree_args("t", &["a", "b"], "/src/Cargo.toml");
        assert!(
            args.windows(2)
                .any(|w| w == ["--edges", "normal,build,features"])
        );
        let packages: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "--package")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(packages, ["a", "b"]);
    }

    #[test]
    fn a_group_with_features_or_a_difference_is_refused() {
        let solo = features_of(LISTING);
        let same = solo.clone();
        let mut merged = solo.clone();
        merged
            .get_mut(&("subtle".to_string(), "2.6.1".to_string()))
            .expect("subtle")
            .insert("const-generics".into());
        assert_eq!(refusal(&[], &[("a", same.clone())], &same), None);
        let why = refusal(&[], &[("a", same.clone())], &merged).expect("refused");
        assert!(why.starts_with("a: subtle 2.6.1"), "{why}");
        let why = refusal(&["fake"], &[("a", same.clone())], &same).expect("refused");
        assert_eq!(why, "the component sets features");
    }
}
