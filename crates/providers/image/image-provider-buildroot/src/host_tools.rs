//! Where Buildroot's host tools come from (`[providers.buildroot.host_tools]`):
//! the build environment's own ("system"), Buildroot's build ("build"), or
//! an error ("fail"), tried in order per tool.
//!
//! A tool taken from the system keeps its `host-<tool>` package, so the
//! package graph, build order and the paths dependents use stay the same,
//! but turns it into a stub: no dependencies, nothing to configure or build,
//! and an install step that puts a wrapper for the system tool in
//! `HOST_DIR/bin`. The stubs are `override` assignments in a Gaia-managed
//! block of the output tree's `local.mk` (Buildroot's package override file,
//! read before every package's `.mk`, whose ordinary assignments then do not
//! apply).
//!
//! Which tools come from the system, and their versions, are part of every
//! package cache key, so a tree built with a system tool never shares cache
//! entries with one built with Buildroot's.
use super::*;
use gaia_spec::HostToolStepSpec;

const BLOCK_BEGIN: &str = "# BEGIN gaia host tools (generated; do not edit)";
const BLOCK_END: &str = "# END gaia host tools";
/// The decisions of the last build, to rebuild a tool's package when they
/// change.
pub(crate) const DECISIONS_FILE: &str = ".gaia-host-tools";

/// What was decided for this build.
#[derive(Debug, Default)]
pub(crate) struct HostTools {
    /// Packages whose source (system or built) changed since the last build.
    pub(crate) changed_packages: BTreeSet<String>,
    pub(crate) messages: Vec<String>,
}

/// A host tool Gaia can take from the system.
struct Tool {
    name: &'static str,
    package: &'static str,
    /// Whether the config uses it at all.
    enabled: fn(&str) -> bool,
    /// The lowest version accepted, `major.minor`.
    min_version: &'static str,
    /// The `local.mk` overrides making `package` a stub for the tool at
    /// `path`.
    stub: fn(&str) -> String,
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "ccache",
        package: "host-ccache",
        enabled: |config| config.lines().any(|line| line.trim() == "BR2_CCACHE=y"),
        min_version: "4.0",
        stub: ccache_stub,
    },
    Tool {
        name: "pkgconf",
        package: "host-pkgconf",
        enabled: |config| {
            config
                .lines()
                .any(|line| line.trim() == "BR2_PACKAGE_HOST_PKGCONF=y")
        },
        // The floor is the oldest pkgconf whose options and environment
        // variables Buildroot's pkg-config wrapper uses (`--keep-system-libs`,
        // `--static`, `PKG_CONFIG_SYSTEM_*_PATH`); the build image's 1.8.1
        // has them. Buildroot's own 2.3.0 also carries a sysroot patch
        // (package/pkgconf/0001-*.patch) no system pkgconf has; see the docs.
        min_version: "1.8",
        stub: pkgconf_stub,
    },
];

/// Buildroot's own ccache reads its cache dir from `BR_CACHE_DIR` (it is
/// patched to); a system ccache reads `CCACHE_DIR`, so the wrapper sets it.
fn ccache_stub(path: &str) -> String {
    format!(
        "override HOST_CCACHE_DEPENDENCIES =\n\
         override HOST_CCACHE_POST_PATCH_HOOKS =\n\
         override define HOST_CCACHE_CONFIGURE_CMDS\n\ttrue\nendef\n\
         override define HOST_CCACHE_BUILD_CMDS\n\ttrue\nendef\n\
         override define HOST_CCACHE_INSTALL_CMDS\n\
         \tmkdir -p $(HOST_DIR)/bin $(BR_CACHE_DIR)\n\
         \tprintf '#!/bin/sh\\nCCACHE_DIR=\"$${{BR_CACHE_DIR:-$$CCACHE_DIR}}\" exec {path} \"$$@\"\\n' > $(HOST_DIR)/bin/ccache\n\
         \tchmod 0755 $(HOST_DIR)/bin/ccache\n\
         endef\n"
    )
}

/// Buildroot's pkg-config wrapper (installed by `HOST_PKGCONF_POST_INSTALL_HOOKS`,
/// which this keeps) runs `HOST_DIR/bin/pkgconf`; that path becomes a wrapper
/// for the system binary. Buildroot's own pkgconf also installs `pkg.m4` into
/// `HOST_DIR/share/aclocal`, which `PKG_CHECK_MODULES` in host autotools
/// packages needs, so the system's copy is copied there when present.
fn pkgconf_stub(path: &str) -> String {
    format!(
        "override HOST_PKGCONF_DEPENDENCIES =\n\
         override define HOST_PKGCONF_CONFIGURE_CMDS\n\ttrue\nendef\n\
         override define HOST_PKGCONF_BUILD_CMDS\n\ttrue\nendef\n\
         override define HOST_PKGCONF_INSTALL_CMDS\n\
         \tmkdir -p $(HOST_DIR)/bin $(HOST_DIR)/share/aclocal\n\
         \tprintf '#!/bin/sh\\nexec {path} \"$$@\"\\n' > $(HOST_DIR)/bin/pkgconf\n\
         \tchmod 0755 $(HOST_DIR)/bin/pkgconf\n\
         \tif [ -f \"$$(dirname {path})/../share/aclocal/pkg.m4\" ]; then cp -f \"$$(dirname {path})/../share/aclocal/pkg.m4\" $(HOST_DIR)/share/aclocal/; fi\n\
         endef\n"
    )
}

/// The build environment's probe of a tool: its path and version, when it is
/// there and new enough. Arguments: the tool name and its minimum version.
pub(crate) type ProbeFn<'a> =
    dyn FnMut(&str, &str) -> Result<Option<(String, String)>, ImageProviderError> + 'a;

/// What the host tools come to for one config: each tool's decision, the
/// `local.mk` stubs for the system ones, and what to rebuild.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HostToolsDecision {
    /// `ccache=system:4.8:/usr/bin/ccache`, `pkgconf=build`, ...
    pub(crate) decisions: String,
    /// The `local.mk` stubs of the tools taken from the system.
    pub(crate) stubs: String,
    pub(crate) messages: Vec<String>,
    /// Packages whose source (system or built) changed since the last build.
    pub(crate) changed_packages: BTreeSet<String>,
}

/// Decides each tool's source for `config`, given the decisions of the last
/// build (`previous`) and a probe of the build environment. Changes nothing.
pub(crate) fn decide_host_tools(
    config: &str,
    policy: &gaia_spec::BuildrootHostToolsPolicySpec,
    previous: &str,
    probe: &mut ProbeFn<'_>,
) -> Result<HostToolsDecision, ImageProviderError> {
    let mut decisions = Vec::new();
    let mut stubs = String::new();
    let mut messages = Vec::new();
    for tool in TOOLS {
        if !(tool.enabled)(config) {
            continue;
        }
        let mut decision = None;
        for step in policy.steps_for(tool.name) {
            match step {
                HostToolStepSpec::System => {
                    if let Some((path, version)) = probe(tool.name, tool.min_version)? {
                        stubs.push_str(&(tool.stub)(&path));
                        messages.push(format!(
                            "host tool {}: system {version} ({path}) instead of {}",
                            tool.name, tool.package
                        ));
                        decision = Some(format!("{}=system:{version}:{path}", tool.name));
                        break;
                    }
                }
                HostToolStepSpec::Build => {
                    decision = Some(format!("{}=build", tool.name));
                    break;
                }
                HostToolStepSpec::Fail => {
                    return Err(ImageProviderError::new(
                        ImageProviderErrorKind::PolicyBlocked,
                        format!(
                            "host tool {}: the build environment has no {} {} or newer, \
                             and providers.buildroot.host_tools.{} does not allow building \
                             it",
                            tool.name, tool.name, tool.min_version, tool.name
                        ),
                    ));
                }
            }
        }
        decisions.push(decision.unwrap_or_else(|| format!("{}=build", tool.name)));
    }
    let identity = decisions.join(" ");
    let changed_packages = TOOLS
        .iter()
        .filter(|tool| decision_of(previous, tool.name) != decision_of(&identity, tool.name))
        // A tree never built has nothing to rebuild.
        .filter(|_| !previous.is_empty())
        .map(|tool| tool.package.to_string())
        .collect();
    Ok(HostToolsDecision {
        decisions: identity,
        stubs,
        messages,
        changed_packages,
    })
}

/// [`decide_host_tools`] with the build environment probed, for the config
/// `config` and the decisions `previous` of the last build.
pub(crate) fn decide_host_tools_probed(
    config: &str,
    previous: &str,
    command_context: &ImageCommandContext<'_>,
) -> Result<HostToolsDecision, ImageProviderError> {
    decide_host_tools(
        config,
        &command_context.policy.host_tools,
        previous,
        &mut |name, min_version| {
            let tool = TOOLS
                .iter()
                .find(|tool| tool.name == name)
                .expect("probed tools are known");
            probe(tool, min_version, command_context)
        },
    )
}

/// Writes a decision into the tree: the stubs into `local.mk`, the decisions
/// into the tree's state.
pub(crate) fn write_host_tools(
    output_dir: &Path,
    decision: &HostToolsDecision,
) -> Result<(), ImageProviderError> {
    write_local_mk_block(output_dir, &decision.stubs)?;
    fs::write(output_dir.join(DECISIONS_FILE), &decision.decisions).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write '{}': {error}",
            output_dir.join(DECISIONS_FILE).display()
        ))
    })
}

/// Decides the host tools of the output tree and writes them. Runs after the
/// config is final and before the package graph is read.
pub(crate) fn apply_host_tools(
    output_dir: &Path,
    command_context: &ImageCommandContext<'_>,
) -> Result<HostTools, ImageProviderError> {
    let config = fs::read_to_string(output_dir.join(".config")).unwrap_or_default();
    let previous = fs::read_to_string(output_dir.join(DECISIONS_FILE)).unwrap_or_default();
    let decision = decide_host_tools_probed(&config, &previous, command_context)?;
    write_host_tools(output_dir, &decision)?;
    Ok(HostTools {
        changed_packages: decision.changed_packages,
        messages: decision.messages,
    })
}

/// The system tools recorded for the last build, for the package keys
/// (tools Buildroot builds add nothing, so existing keys stay valid).
pub(crate) fn recorded_host_tools(output_dir: &Path) -> String {
    system_host_tools(&fs::read_to_string(output_dir.join(DECISIONS_FILE)).unwrap_or_default())
}

/// The system tools among `decisions` (see [`recorded_host_tools`]).
pub(crate) fn system_host_tools(decisions: &str) -> String {
    decisions
        .split_whitespace()
        .filter(|decision| decision.contains("=system:"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn decision_of<'a>(decisions: &'a str, tool: &str) -> Option<&'a str> {
    decisions
        .split_whitespace()
        .find_map(|decision| decision.strip_prefix(tool)?.strip_prefix('='))
}

/// The tool's path and version in the build environment, when it is there
/// and new enough.
fn probe(
    tool: &Tool,
    min_version: &str,
    command_context: &ImageCommandContext<'_>,
) -> Result<Option<(String, String)>, ImageProviderError> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(format!(
        "p=$(command -v {0}) && echo \"$p\" && \"$p\" --version | head -n 1",
        tool.name
    ));
    let output = command_output_with_timeout(
        &mut command,
        command_context.execution,
        Duration::from_secs(120),
        &format!("host tool probe {}", tool.name),
        command_context.policy.output_retention,
        None,
        command_context.cancel_check.clone(),
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse_probe(
        &String::from_utf8_lossy(&output.stdout),
        min_version,
    ))
}

/// `<path>\n<name> version X.Y.Z...` (or a bare `X.Y.Z`, as pkgconf prints) to
/// (path, version) when the version is at least `min_version`.
fn parse_probe(stdout: &str, min_version: &str) -> Option<(String, String)> {
    let mut lines = stdout.lines();
    let path = lines.next()?.trim().to_string();
    let version = lines
        .next()?
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))?
        .to_string();
    (path.starts_with('/') && version_at_least(&version, min_version)).then_some((path, version))
}

/// Compares dotted versions component by component (`1.10` is newer than
/// `1.8`); a non-numeric suffix on a component is ignored (`3rc1` is 3).
fn version_at_least(version: &str, min_version: &str) -> bool {
    let parts = |text: &str| -> Vec<u64> {
        text.split('.')
            .map(|part| {
                part.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    parts(version) >= parts(min_version)
}

/// Replaces Gaia's host tools block in `<output>/local.mk`.
fn write_local_mk_block(output_dir: &Path, stubs: &str) -> Result<(), ImageProviderError> {
    write_local_mk_section(output_dir, BLOCK_BEGIN, BLOCK_END, stubs)
}

/// Replaces the block between `begin` and `end` lines in `<output>/local.mk`,
/// keeping the rest of the file; an empty `content` removes the block.
pub(crate) fn write_local_mk_section(
    output_dir: &Path,
    begin: &str,
    end: &str,
    content: &str,
) -> Result<(), ImageProviderError> {
    let path = output_dir.join("local.mk");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut kept = String::new();
    let mut inside = false;
    for line in existing.lines() {
        if line == begin {
            inside = true;
        } else if line == end {
            inside = false;
        } else if !inside {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    let contents = if content.is_empty() {
        kept
    } else {
        format!("{kept}{begin}\n{content}{end}\n")
    };
    if contents == existing {
        return Ok(());
    }
    if contents.is_empty() {
        let _ = fs::remove_file(&path);
        return Ok(());
    }
    fs::write(&path, contents).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write '{}': {error}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_accept_new_enough_tools() {
        assert_eq!(
            parse_probe("/usr/bin/ccache\nccache version 4.10.2\n", "4.0"),
            Some(("/usr/bin/ccache".to_string(), "4.10.2".to_string()))
        );
        assert_eq!(
            parse_probe("/usr/bin/ccache\nccache version 3.7.12\n", "4.0"),
            None
        );
        assert_eq!(parse_probe("", "4.0"), None);
    }

    #[test]
    fn pkgconf_probe_parses_the_bare_version_line() {
        assert_eq!(
            parse_probe("/usr/bin/pkgconf\n1.8.1\n", "1.8"),
            Some(("/usr/bin/pkgconf".to_string(), "1.8.1".to_string()))
        );
        assert_eq!(
            parse_probe("/usr/bin/pkgconf\n2.3.0\n", "1.8"),
            Some(("/usr/bin/pkgconf".to_string(), "2.3.0".to_string()))
        );
        // Below the floor, and no path: no system tool.
        assert_eq!(parse_probe("/usr/bin/pkgconf\n1.7.4\n", "1.8"), None);
        assert_eq!(parse_probe("pkgconf\n2.3.0\n", "1.8"), None);
    }

    #[test]
    fn minimum_versions_compare_component_by_component() {
        assert!(version_at_least("1.8", "1.8"));
        assert!(version_at_least("1.8.1", "1.8"));
        assert!(version_at_least("1.10", "1.8"));
        assert!(version_at_least("10.0", "4.0"));
        assert!(version_at_least("4.0rc1", "4.0"));
        assert!(!version_at_least("1.7.9", "1.8"));
        assert!(!version_at_least("3.7.12", "4.0"));
    }

    #[test]
    fn pkgconf_is_enabled_by_its_config_symbol() {
        let tool = TOOLS
            .iter()
            .find(|tool| tool.name == "pkgconf")
            .expect("pkgconf");
        assert!((tool.enabled)("BR2_PACKAGE_HOST_PKGCONF=y\nBR2_CCACHE=y\n"));
        assert!(!(tool.enabled)("# BR2_PACKAGE_HOST_PKGCONF is not set\n"));
    }

    #[test]
    fn pkgconf_stub_only_swaps_the_binary() {
        let stub = pkgconf_stub("/usr/bin/pkgconf");
        assert!(stub.contains("override HOST_PKGCONF_DEPENDENCIES =\n"));
        assert!(stub.contains("override define HOST_PKGCONF_INSTALL_CMDS\n"));
        assert!(stub.contains("exec /usr/bin/pkgconf \"$$@\"\\n"));
        assert!(stub.contains("cp -f \"$$(dirname /usr/bin/pkgconf)/../share/aclocal/pkg.m4\""));
        // The pkg-config wrapper is Buildroot's post-install hook, not stubbed.
        assert!(!stub.contains("pkg-config"));
        assert!(!stub.contains("HOST_PKGCONF_POST_INSTALL_HOOKS"));
    }

    #[test]
    fn the_local_mk_block_is_replaced_and_user_lines_kept() {
        let dir = std::env::temp_dir().join(format!("gaia-host-tools-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("local.mk"), "FOO_OVERRIDE_SRCDIR = /src/foo\n").expect("local.mk");
        write_local_mk_block(&dir, &ccache_stub("/usr/bin/ccache")).expect("write");
        let written = fs::read_to_string(dir.join("local.mk")).expect("read");
        assert!(written.starts_with("FOO_OVERRIDE_SRCDIR = /src/foo\n"));
        assert!(written.contains("override HOST_CCACHE_DEPENDENCIES =\n"));
        assert!(written.contains("exec /usr/bin/ccache"));
        assert!(written.contains("$${BR_CACHE_DIR:-$$CCACHE_DIR}"));
        // Rewritten, not appended twice; then removed.
        write_local_mk_block(&dir, &ccache_stub("/usr/bin/ccache")).expect("again");
        let again = fs::read_to_string(dir.join("local.mk")).expect("read");
        assert_eq!(again.matches(BLOCK_BEGIN).count(), 1);
        write_local_mk_block(&dir, "").expect("remove");
        assert_eq!(
            fs::read_to_string(dir.join("local.mk")).expect("read"),
            "FOO_OVERRIDE_SRCDIR = /src/foo\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn decisions_are_read_per_tool() {
        let decisions = "ccache=system:4.10.2:/usr/bin/ccache pkgconf=build";
        assert_eq!(
            decision_of(decisions, "ccache"),
            Some("system:4.10.2:/usr/bin/ccache")
        );
        assert_eq!(decision_of(decisions, "pkgconf"), Some("build"));
        assert_eq!(decision_of("", "ccache"), None);
    }

    fn probe_found(name: &str, _min: &str) -> Result<Option<(String, String)>, ImageProviderError> {
        Ok(match name {
            "ccache" => Some(("/usr/bin/ccache".to_string(), "4.10.2".to_string())),
            _ => None,
        })
    }

    /// The policy `ccache = [System, Build]`, `pkgconf = [Build]`.
    fn system_first_policy() -> gaia_spec::BuildrootHostToolsPolicySpec {
        let mut policy = gaia_spec::BuildrootHostToolsPolicySpec::default();
        policy.tools.insert(
            "ccache".to_string(),
            vec![HostToolStepSpec::System, HostToolStepSpec::Build],
        );
        policy
    }

    #[test]
    fn host_tool_decisions_take_the_system_tool_when_found_and_build_otherwise() {
        let config = "BR2_CCACHE=y\nBR2_PACKAGE_HOST_PKGCONF=y\n";
        let policy = system_first_policy();
        let decision = decide_host_tools(config, &policy, "", &mut probe_found).expect("decision");
        assert!(
            decision
                .decisions
                .contains("ccache=system:4.10.2:/usr/bin/ccache")
        );
        assert!(decision.decisions.contains("pkgconf=build"));
        assert!(decision.stubs.contains("HOST_CCACHE_INSTALL_CMDS"));
        assert!(!decision.stubs.contains("HOST_PKGCONF"));
        // A tree never built has nothing to rebuild.
        assert!(decision.changed_packages.is_empty());
        assert_eq!(
            system_host_tools(&decision.decisions),
            "ccache=system:4.10.2:/usr/bin/ccache"
        );
    }

    #[test]
    fn host_tool_changes_rebuild_only_the_tool_whose_decision_changed() {
        let config = "BR2_CCACHE=y\nBR2_PACKAGE_HOST_PKGCONF=y\n";
        let policy = system_first_policy();
        let previous = "ccache=build pkgconf=build";
        let decision =
            decide_host_tools(config, &policy, previous, &mut probe_found).expect("decision");
        assert_eq!(
            decision.changed_packages,
            BTreeSet::from(["host-ccache".to_string()])
        );
        // Nothing probed, nothing changed: an unchanged decision is no change.
        let same = decide_host_tools(
            config,
            &policy,
            "ccache=system:4.10.2:/usr/bin/ccache pkgconf=build",
            &mut probe_found,
        )
        .expect("decision");
        assert!(same.changed_packages.is_empty());
    }

    #[test]
    fn host_tools_the_build_environment_lacks_are_built() {
        let config = "BR2_CCACHE=y\n";
        let policy = gaia_spec::BuildrootHostToolsPolicySpec::default();
        let mut nothing_found = |_: &str, _: &str| Ok(None);
        let decision =
            decide_host_tools(config, &policy, "", &mut nothing_found).expect("decision");
        assert_eq!(decision.decisions, "ccache=build");
        assert!(decision.stubs.is_empty());
        assert!(decision.messages.is_empty());
    }
}
