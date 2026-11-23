//! Which Rust component builds share one cargo invocation.
//!
//! Components built from the same source tree for the same target, profile,
//! features and toolchain compile the same dependency graph. Built one
//! cargo run each, every shared crate is compiled once per component. The
//! builds that agree on everything but their package are one group: one
//! cargo run with each package as `--package`, so each crate is compiled
//! once. Anything that changes what cargo compiles keeps builds apart: a
//! different source, target, profile, feature set, CPU, C toolchain, root
//! image or tool tree. A host helper and a target binary differ in target,
//! so they never share a run.
//!
//! The group's key is the one its members had, with the packages in place
//! of each member's package: [`Group::packages`] (sorted, without repeats).

use std::collections::BTreeMap;

/// One Rust component build, as the graph states it. Every field but
/// `component` and `package` is part of what cargo compiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustBuild {
    /// The component's name, for the group's members and errors.
    pub component: String,
    /// The package cargo builds (`--package`).
    pub package: String,
    /// The source tree: its locked commit, or its tree digest.
    pub source: String,
    /// The root image's digest.
    pub root: String,
    /// The target triple.
    pub triple: String,
    /// The cargo profile name, `release` when the component names none.
    pub profile: String,
    /// The feature names, as the build passes them (a set: sorted, no repeats).
    pub features: Vec<String>,
    /// The CPU the code is tuned for, empty for the default.
    pub cpu: String,
    /// The C compiler directory, empty when the build has none.
    pub cc: String,
    /// The component's own environment, as the build gives it.
    pub env: String,
    /// The tools tree's digest, empty when the build has none.
    pub tools: String,
    /// The Rust release and the target stds its toolchain source pins.
    pub toolchain: String,
}

impl RustBuild {
    /// What must be equal for two builds to share one cargo run.
    fn shared(&self) -> SharedKey<'_> {
        SharedKey {
            source: &self.source,
            root: &self.root,
            triple: &self.triple,
            profile: &self.profile,
            features: &self.features,
            cpu: &self.cpu,
            cc: &self.cc,
            env: &self.env,
            tools: &self.tools,
            toolchain: &self.toolchain,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SharedKey<'a> {
    source: &'a str,
    root: &'a str,
    triple: &'a str,
    profile: &'a str,
    features: &'a [String],
    cpu: &'a str,
    cc: &'a str,
    env: &'a str,
    tools: &'a str,
    toolchain: &'a str,
}

/// Builds that run one cargo invocation together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The members' indexes into the builds given, in the order given.
    pub members: Vec<usize>,
    /// The members' packages, sorted, without repeats: the `--package`s.
    pub packages: Vec<String>,
}

impl Group {
    /// The packages as the step's key takes them: sorted, comma-separated.
    pub fn packages_key(&self) -> String {
        self.packages.join(",")
    }
}

/// Groups `builds` by what cargo compiles. Groups come in the order of
/// their first member; each build is in exactly one group.
pub fn group(builds: &[RustBuild]) -> Vec<Group> {
    let mut by_key: BTreeMap<SharedKey<'_>, usize> = BTreeMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for (index, build) in builds.iter().enumerate() {
        let slot = *by_key.entry(build.shared()).or_insert_with(|| {
            groups.push(Group {
                members: Vec::new(),
                packages: Vec::new(),
            });
            groups.len() - 1
        });
        let group = &mut groups[slot];
        group.members.push(index);
        if !group.packages.contains(&build.package) {
            group.packages.push(build.package.clone());
        }
    }
    for group in &mut groups {
        group.packages.sort();
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(component: &str, package: &str) -> RustBuild {
        RustBuild {
            component: component.into(),
            package: package.into(),
            source: "d6b9361d".into(),
            root: "sha256:rust-slim".into(),
            triple: "aarch64-unknown-linux-musl".into(),
            profile: "service".into(),
            features: Vec::new(),
            cpu: "".into(),
            cc: "".into(),
            env: "".into(),
            tools: "".into(),
            toolchain: "1.99.0 aarch64-unknown-linux-musl".into(),
        }
    }

    /// Four packages of one workspace: one source, one target, one profile.
    fn four_packages() -> Vec<RustBuild> {
        vec![
            build("lemnosd", "lemnosd"),
            build("lemnos-ctl", "lemnos-ctl"),
            build("lemnos-orion", "lemnos-orion"),
            build("lemnos-slot-init", "lemnos-slot-init"),
        ]
    }

    #[test]
    fn one_source_target_and_profile_is_one_run() {
        let groups = group(&four_packages());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members, [0, 1, 2, 3]);
        assert_eq!(
            groups[0].packages,
            ["lemnos-ctl", "lemnos-orion", "lemnos-slot-init", "lemnosd"]
        );
        assert_eq!(
            groups[0].packages_key(),
            "lemnos-ctl,lemnos-orion,lemnos-slot-init,lemnosd"
        );
    }

    #[test]
    fn a_different_rev_is_its_own_run() {
        let mut builds = four_packages();
        builds[2].source = "455a956e".into();
        let groups = group(&builds);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].members, [0, 1, 3]);
        assert_eq!(groups[1].members, [2]);
        assert_eq!(groups[1].packages, ["lemnos-orion"]);
    }

    #[test]
    fn a_host_helper_is_not_merged_with_the_target_binaries() {
        let mut builds = four_packages();
        builds.push(RustBuild {
            triple: "x86_64-unknown-linux-gnu".into(),
            profile: "release".into(),
            cpu: "".into(),
            ..build("lemnos-ctl-tool", "lemnos-ctl")
        });
        let groups = group(&builds);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].members, [0, 1, 2, 3]);
        assert_eq!(groups[1].members, [4]);
    }

    #[test]
    fn a_different_profile_triple_features_cpu_or_toolchain_is_its_own_run() {
        let base = build("a", "a");
        let variants = [
            RustBuild {
                profile: "release".into(),
                ..base.clone()
            },
            RustBuild {
                triple: "x86_64-unknown-linux-musl".into(),
                ..base.clone()
            },
            RustBuild {
                features: vec!["fake-devices".into()],
                ..base.clone()
            },
            RustBuild {
                cpu: "cortex-a53".into(),
                ..base.clone()
            },
            RustBuild {
                cc: "/src/scripts/zigcc".into(),
                ..base.clone()
            },
            RustBuild {
                env: "ZIG_GLOBAL_CACHE_DIR=/build/zig-cache".into(),
                ..base.clone()
            },
            RustBuild {
                toolchain: "1.98.0 aarch64-unknown-linux-musl".into(),
                ..base.clone()
            },
            RustBuild {
                tools: "sha256:zig".into(),
                ..base.clone()
            },
            RustBuild {
                root: "sha256:other-image".into(),
                ..base.clone()
            },
        ];
        for variant in variants {
            let builds = vec![
                base.clone(),
                RustBuild {
                    package: "b".into(),
                    ..variant
                },
            ];
            let groups = group(&builds);
            assert_eq!(groups.len(), 2, "{:?}", builds[1]);
        }
    }

    #[test]
    fn the_same_package_twice_is_one_package_in_one_run() {
        let mut builds = four_packages();
        builds.push(build("lemnosd-again", "lemnosd"));
        let groups = group(&builds);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members, [0, 1, 2, 3, 4]);
        assert_eq!(
            groups[0]
                .packages
                .iter()
                .filter(|p| *p == "lemnosd")
                .count(),
            1
        );
    }

    #[test]
    fn groups_keep_their_first_member_order_and_cover_every_build_once() {
        let mut builds = four_packages();
        builds[1].profile = "release".into();
        builds[3].cpu = "cortex-a53".into();
        let groups = group(&builds);
        let order: Vec<&[usize]> = groups.iter().map(|g| g.members.as_slice()).collect();
        assert_eq!(order, [&[0, 2][..], &[1][..], &[3][..]]);
        let mut seen: Vec<usize> = groups.iter().flat_map(|g| g.members.clone()).collect();
        seen.sort();
        assert_eq!(seen, [0, 1, 2, 3]);
    }

    #[test]
    fn no_builds_no_groups() {
        assert!(group(&[]).is_empty());
    }
}
