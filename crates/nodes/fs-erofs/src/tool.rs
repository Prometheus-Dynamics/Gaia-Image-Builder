//! Which `mkfs.erofs` makes an image.
//!
//! The build machine's tool makes it when it meets the floor: erofs-utils
//! 1.8 or later, with each compressor the image names in its `--version`
//! text, `--tar=f` accepted, each `-E` option the image asks for accepted,
//! and `fsck.erofs` beside it to read the image back. Each of those is asked
//! of the tool itself: the version and compressors from `--version`, and
//! `--tar=f` and each `-E` option by making an empty image with it.
//!
//! When the machine's tool does not meet the floor, or the setting
//! [`ENV`] is `buildroot`, the image's own Buildroot build makes it with
//! its host erofs-utils, which Gaia asks for in the OS build's config
//! ([`BUILDROOT_LINE`]). That tool writes only [`BUILDROOT_COMPRESSIONS`].
//!
//! The decision is made at plan time, where the graph says which tool
//! makes the image and why, and the step checks the same floor when it runs.
//! The image's key holds the tool's full `--version` text, so an image made
//! with another erofs-utils has another key. The bytes of an image depend on
//! the version of the machine's erofs-utils that made it; the key records
//! which one.

use std::path::Path;
use std::process::Command;

/// The oldest erofs-utils the build machine's tool may be.
pub const FLOOR: (u32, u32) = (1, 8);

/// The compressions the host `mkfs.erofs` of a Buildroot build writes:
/// Buildroot configures the host erofs-utils with lz4 and lzma, and without
/// zstd, deflate or zlib.
pub const BUILDROOT_COMPRESSIONS: [&str; 4] = ["none", "lz4", "lz4hc", "lzma"];

/// The line of an OS fragment that has Buildroot build the host erofs-utils.
/// Gaia adds it to the OS config when the image's tool is Buildroot's.
pub const BUILDROOT_LINE: &str = "BR2_PACKAGE_HOST_EROFS_UTILS=y";

/// The name of the fragment that holds [`BUILDROOT_LINE`].
pub const FRAGMENT: &str = "gaia.erofs-host";

/// The environment variable that forces Buildroot's tool: `buildroot`.
/// `machine`, or unset, is the default: the build machine's tool when it
/// meets the floor. Any other value fails the plan.
pub const ENV: &str = "GAIA_EROFS_TOOL";

/// What an image asks of its `mkfs.erofs`, for the floor.
#[derive(Debug, Clone, Copy)]
pub struct Asked<'a> {
    /// The compression (`none`, or a compressor's name).
    pub compression: &'a str,
    /// The `-E` options: `fragments`, `ztailpacking`, `dedupe`.
    pub extended: &'a [&'a str],
}

/// What a `mkfs.erofs` was found to be, from its own text and its probes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The program, as it was given.
    pub program: String,
    /// The first line of `--version`, for the plan's words.
    pub version: String,
    /// `(major, minor)` of the version, when the text has one.
    pub number: Option<(u32, u32)>,
    /// The compressors the `--version` text lists.
    pub compressors: Vec<String>,
    /// Whether `fsck.erofs` is beside the program.
    pub fsck: bool,
    /// The options the probes were refused: `--tar=f`, or an `-E` name.
    pub refused: Vec<String>,
}

/// Which tool makes an image, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// The build machine's tool, with what it was found to be.
    Machine(Facts),
    /// Buildroot's host tool, and the floor item that failed (or the setting).
    Buildroot(String),
}

/// Whether the setting `value` (the environment's [`ENV`]) forces Buildroot's
/// tool.
///
/// # Errors
///
/// A value that is neither `machine`, `buildroot` nor unset.
pub fn forced_by_setting(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("") | Some("machine") => Ok(false),
        Some("buildroot") => Ok(true),
        Some(other) => Err(format!(
            "{ENV} is `{other}`: it is `buildroot` (Buildroot's erofs-utils makes the images) or `machine` (or unset: the build machine's mkfs.erofs makes them when it meets the floor)"
        )),
    }
}

/// The version `(major, minor)` in the first line of `--version`
/// (`mkfs.erofs (erofs-utils) 1.9.4`).
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    let line = text.lines().next()?;
    line.split_whitespace().find_map(|token| {
        let mut parts = token.split('.');
        let major = parts.next()?.parse::<u32>().ok()?;
        let minor = parts.next()?.parse::<u32>().ok()?;
        Some((major, minor))
    })
}

/// The compressors the `--version` text lists (`available compressors: lz4,
/// zstd`).
pub fn parse_compressors(text: &str) -> Vec<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("available compressors:"))
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The first floor item the tool with `facts` does not meet for `asked`, as
/// a short reason; `None` when it meets all of them.
pub fn falls_short(facts: &Facts, asked: &Asked<'_>) -> Option<String> {
    let Some((major, minor)) = facts.number else {
        return Some(format!(
            "{} --version gives no erofs-utils version",
            facts.program
        ));
    };
    if (major, minor) < FLOOR {
        return Some(format!(
            "{} is erofs-utils {major}.{minor}, and the floor is {}.{}",
            facts.program, FLOOR.0, FLOOR.1
        ));
    }
    if asked.compression != "none" && !facts.compressors.iter().any(|c| c == asked.compression) {
        return Some(format!(
            "{} has no {} compressor (it has {})",
            facts.program,
            asked.compression,
            facts.compressors.join(", ")
        ));
    }
    if facts.refused.iter().any(|r| r == "--tar=f") {
        return Some(format!("{} refuses --tar=f", facts.program));
    }
    for option in asked.extended {
        if facts.refused.iter().any(|r| r == option) {
            return Some(format!("{} refuses -E{option}", facts.program));
        }
    }
    if !facts.fsck {
        return Some(format!(
            "no fsck.erofs beside {}, to read the image back",
            facts.program
        ));
    }
    None
}

/// The facts of the program at `program` (`None` or empty: no tool on this
/// machine), by running it.
///
/// # Errors
///
/// No program, or one that does not run `--version`.
pub fn facts(program: Option<&str>, asked: &Asked<'_>) -> Result<Facts, String> {
    let program = program.filter(|p| !p.is_empty()).ok_or_else(|| {
        "no mkfs.erofs on this machine (none on PATH, none given with --tool)".to_string()
    })?;
    let version = run(program, &["--version"]).map_err(|e| format!("{program} --version: {e}"))?;
    // One directory per call: calls in one process (the tests, say) must not
    // share the probe files one of them removes.
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch =
        std::env::temp_dir().join(format!("gaia-erofs-probe-{}-{call}", std::process::id()));
    let refused = probe_refusals(program, asked, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(Facts {
        program: program.to_string(),
        version: version.lines().next().unwrap_or_default().to_string(),
        number: parse_version(&version),
        compressors: parse_compressors(&version),
        fsck: Path::new(program).with_file_name("fsck.erofs").is_file(),
        refused,
    })
}

/// The tool with the floor's facts, for the step that makes an image with
/// it.
///
/// # Errors
///
/// The floor item it does not meet, or why it cannot be asked.
pub fn check(program: &str, asked: &Asked<'_>) -> Result<Facts, String> {
    let facts = facts(Some(program), asked)?;
    match falls_short(&facts, asked) {
        Some(short) => Err(format!(
            "the build machine's mkfs.erofs does not meet the floor: {short}"
        )),
        None => Ok(facts),
    }
}

/// The plan's choice: the build machine's tool when `program` meets the
/// floor for `asked`, Buildroot's otherwise (or when `forced`).
pub fn choose(program: Option<&str>, forced: bool, asked: &Asked<'_>) -> Choice {
    if forced {
        return Choice::Buildroot(format!("{ENV}=buildroot"));
    }
    match facts(program, asked) {
        Err(reason) => Choice::Buildroot(reason),
        Ok(facts) => match falls_short(&facts, asked) {
            Some(short) => Choice::Buildroot(short),
            None => Choice::Machine(facts),
        },
    }
}

/// The options the tool refuses, by making an empty image with each one
/// alone: `--tar=f` first, then each `-E` option asked for.
fn probe_refusals(program: &str, asked: &Asked<'_>, scratch: &Path) -> Vec<String> {
    if std::fs::create_dir_all(scratch).is_err() {
        return vec!["--tar=f".to_string()];
    }
    let tar = scratch.join("empty.tar");
    // An archive of two empty blocks: no entries.
    let image = scratch.join("probe.erofs");
    if std::fs::write(&tar, [0u8; 1024]).is_err() {
        return vec!["--tar=f".to_string()];
    }
    let mut refused = Vec::new();
    let mut options: Vec<(String, Vec<String>)> = vec![("--tar=f".into(), vec!["--tar=f".into()])];
    for option in asked.extended {
        options.push((
            (*option).to_string(),
            vec!["--tar=f".into(), format!("-E{option}")],
        ));
    }
    for (name, flags) in options {
        let mut command = Command::new(program);
        command
            .env_clear()
            .arg("--quiet")
            .args(&flags)
            .arg(&image)
            .arg(&tar);
        let ok = command.output().is_ok_and(|out| out.status.success());
        let _ = std::fs::remove_file(&image);
        if !ok {
            refused.push(name);
        }
    }
    refused
}

/// The standard output and error of `program args`, when it exits 0.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .env_clear()
        .output()
        .map_err(|e| e.to_string())?;
    let text = [out.stdout, out.stderr].concat();
    if !out.status.success() {
        return Err(format!("exit {}", out.status.code().unwrap_or(-1)));
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_with(version: &str, compressors: &[&str], refused: &[&str], fsck: bool) -> Facts {
        Facts {
            program: "/usr/bin/mkfs.erofs".into(),
            version: format!("mkfs.erofs (erofs-utils) {version}"),
            number: parse_version(&format!("mkfs.erofs (erofs-utils) {version}")),
            compressors: compressors.iter().map(|c| c.to_string()).collect(),
            fsck,
            refused: refused.iter().map(|c| c.to_string()).collect(),
        }
    }

    const ASKED: Asked<'static> = Asked {
        compression: "zstd",
        extended: &["fragments"],
    };
    const ALL: &[&str] = &["lz4", "lz4hc", "lzma", "deflate", "libdeflate", "zstd"];

    /// A tool at 1.8 or later that writes the compressor and takes the
    /// options meets the floor.
    #[test]
    fn a_new_enough_tool_meets_the_floor() {
        assert_eq!(
            parse_version("mkfs.erofs (erofs-utils) 1.9.4\n"),
            Some((1, 9))
        );
        assert_eq!(
            parse_version("mkfs.erofs (erofs-utils) 1.8.10\n"),
            Some((1, 8))
        );
        assert_eq!(parse_version("no version here"), None);
        let facts = facts_with("1.9.4", ALL, &[], true);
        assert_eq!(falls_short(&facts, &ASKED), None);
        // The floor is 1.8: a tool exactly there meets it.
        let floor = facts_with("1.8.0", ALL, &[], true);
        assert_eq!(falls_short(&floor, &ASKED), None);
    }

    /// Older than 1.8 is the first item short.
    #[test]
    fn a_tool_older_than_the_floor_falls_short() {
        let old = facts_with("1.7.1", ALL, &[], true);
        let short = falls_short(&old, &ASKED).expect("short");
        assert!(
            short.contains("erofs-utils 1.7") && short.contains("floor is 1.8"),
            "{short}"
        );
    }

    /// A compression the tool does not list is short; `none` needs none.
    #[test]
    fn a_missing_compressor_falls_short_and_none_needs_none() {
        let no_zstd = facts_with("1.9.4", &["lz4", "lz4hc", "lzma"], &[], true);
        let short = falls_short(&no_zstd, &ASKED).expect("short");
        assert!(
            short.contains("no zstd compressor") && short.contains("lz4, lz4hc, lzma"),
            "{short}"
        );
        let none = Asked {
            compression: "none",
            extended: &[],
        };
        assert_eq!(falls_short(&no_zstd, &none), None);
        let parsed = parse_compressors(
            "mkfs.erofs (erofs-utils) 1.9.4\navailable compressors: lz4, lz4hc, lzma, deflate, libdeflate, zstd\n",
        );
        assert_eq!(
            parsed,
            ["lz4", "lz4hc", "lzma", "deflate", "libdeflate", "zstd"]
        );
    }

    /// A tool that refuses an option the image asks for is short, by name.
    #[test]
    fn a_refused_option_falls_short() {
        let refuses_tar = facts_with("1.9.4", ALL, &["--tar=f"], true);
        assert!(
            falls_short(&refuses_tar, &ASKED)
                .expect("short")
                .contains("--tar=f")
        );
        let refuses_fragments = facts_with("1.9.4", ALL, &["fragments"], true);
        assert!(
            falls_short(&refuses_fragments, &ASKED)
                .expect("short")
                .contains("-Efragments")
        );
        // An option not asked for is not needed.
        let refuses_dedupe = facts_with("1.9.4", ALL, &["dedupe"], true);
        assert_eq!(falls_short(&refuses_dedupe, &ASKED), None);
    }

    /// No fsck.erofs beside the tool is short: the image could not be read
    /// back.
    #[test]
    fn a_missing_fsck_falls_short() {
        let no_fsck = facts_with("1.9.4", ALL, &[], false);
        let short = falls_short(&no_fsck, &ASKED).expect("short");
        assert!(short.contains("no fsck.erofs beside"), "{short}");
    }

    /// No tool on the machine, and no program given, is a choice for
    /// Buildroot's tool, with the reason.
    #[test]
    fn no_tool_on_the_machine_is_buildroots() {
        let none = choose(None, false, &ASKED);
        assert!(
            matches!(&none, Choice::Buildroot(r) if r.contains("no mkfs.erofs on this machine")),
            "{none:?}"
        );
        let empty = choose(Some(""), false, &ASKED);
        assert!(matches!(empty, Choice::Buildroot(_)));
        let missing = choose(Some("/nonexistent/mkfs.erofs"), false, &ASKED);
        assert!(
            matches!(&missing, Choice::Buildroot(r) if r.contains("--version")),
            "{missing:?}"
        );
    }

    /// The setting forces Buildroot's tool whatever the machine has, and
    /// takes only its three values.
    #[test]
    fn the_setting_forces_buildroot_and_takes_three_values() {
        assert_eq!(forced_by_setting(None), Ok(false));
        assert_eq!(forced_by_setting(Some("machine")), Ok(false));
        assert_eq!(forced_by_setting(Some("buildroot")), Ok(true));
        assert!(forced_by_setting(Some("host")).is_err());
        assert_eq!(
            choose(Some("/usr/bin/mkfs.erofs"), true, &ASKED),
            Choice::Buildroot("GAIA_EROFS_TOOL=buildroot".into())
        );
    }

    /// The Buildroot fragment line is the one Buildroot's own option names.
    #[test]
    fn the_buildroot_line_is_the_host_option() {
        assert_eq!(BUILDROOT_LINE, "BR2_PACKAGE_HOST_EROFS_UTILS=y");
        assert_eq!(BUILDROOT_COMPRESSIONS, ["none", "lz4", "lz4hc", "lzma"]);
    }
}
