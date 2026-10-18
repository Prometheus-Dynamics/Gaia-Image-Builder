//! The external tools the assembly runs: where a tool is found (the provider's
//! host tools, then the host PATH) and how its version is probed.

use super::*;

const TOOL_VERSION_TIMEOUT_SECONDS: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedTool {
    pub(super) program: PathBuf,
    pub(super) display: String,
}

pub(super) fn resolve_assembly_tool(
    roots: &AssemblyRoots,
    name: &str,
) -> Result<ResolvedTool, String> {
    if let Some(provider_host) = &roots.provider_host {
        // Buildroot installs system tools (veritysetup, mkfs.ext4, debugfs,
        // depmod) into host/sbin.
        for relative in [
            PathBuf::from("bin").join(name),
            PathBuf::from("sbin").join(name),
            PathBuf::from("usr/bin").join(name),
            PathBuf::from("usr/sbin").join(name),
            PathBuf::from(name),
        ] {
            let candidate = provider_host.join(relative);
            if candidate.is_file() {
                return Ok(ResolvedTool {
                    display: candidate.display().to_string(),
                    program: candidate,
                });
            }
        }
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(ResolvedTool {
                    display: candidate.display().to_string(),
                    program: candidate,
                });
            }
        }
    }

    Err(format!(
        "assembly tool '{name}' was not found in provider host tools or host PATH"
    ))
}

/// `--version` probes already run in this process, keyed by program path and
/// arguments. A probe can take up to `TOOL_VERSION_TIMEOUT_SECONDS`, and every
/// step that records a tool would otherwise repeat it.
type ToolVersionCache =
    std::sync::Mutex<std::collections::HashMap<(PathBuf, String), Option<String>>>;
static TOOL_VERSIONS: std::sync::OnceLock<ToolVersionCache> = std::sync::OnceLock::new();

pub(super) fn tool_version<const N: usize>(tool: &ResolvedTool, args: [&str; N]) -> Option<String> {
    let key = (PathBuf::from(&tool.program), args.join(" "));
    let cache = TOOL_VERSIONS.get_or_init(Default::default);
    if let Ok(cached) = cache.lock()
        && let Some(version) = cached.get(&key)
    {
        return version.clone();
    }
    let version = probe_tool_version(tool, args);
    if let Ok(mut cached) = cache.lock() {
        cached.insert(key, version.clone());
    }
    version
}

fn probe_tool_version<const N: usize>(tool: &ResolvedTool, args: [&str; N]) -> Option<String> {
    let mut command = Command::new(&tool.program);
    command.args(args);
    let result = gaia_process::run_command_with_timeout_and_retention(
        &mut command,
        Duration::from_secs(TOOL_VERSION_TIMEOUT_SECONDS),
        "assembly tool version",
        gaia_process::ProcessOutputRetention {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            stdout_lines: 4,
            stderr_lines: 4,
        },
        None,
        None,
    )
    .ok()?;
    if !result.output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&result.output.stdout);
    let stderr = String::from_utf8_lossy(&result.output.stderr);
    stdout
        .lines()
        .chain(stderr.lines())
        .next()
        .map(str::to_string)
}
