//! Reuse of the host tools probe. A probe asks the build environment for each
//! tool (`command -v`, then `--version`): with a docker execution backend
//! that is a `docker run` per tool, seconds each. The answers depend only on
//! the inputs of [`probe_key`]; when those match the key of the last probe,
//! recorded in [`PROBE_CACHE_FILE`] next to the decisions, the answers are
//! reused. Any input that cannot be read or checked means a probe, as before.
use super::host_tools::{HostToolsDecision, ProbeFn, decide_host_tools};
use super::*;
use sha2::{Digest, Sha256};
use std::io;
use std::os::unix::fs::MetadataExt;

/// The probe answers of the last build, with their key (in the output tree).
pub(crate) const PROBE_CACHE_FILE: &str = ".gaia-host-tools-probe";
const CACHE_HEADER: &str = "gaia host tools probe 1";

/// A tool the decision may probe, and what its probe depends on.
#[derive(Debug, Clone)]
pub(crate) struct ProbeTool {
    pub(crate) name: &'static str,
    pub(crate) min_version: &'static str,
    /// Whether the config uses the tool at all.
    pub(crate) enabled: bool,
    /// Whether the policy asks for the system tool (the only probe).
    pub(crate) system: bool,
    /// The policy steps of the tool, whatever they are.
    pub(crate) steps: String,
}

/// Where the probe runs.
pub(crate) enum ProbeBackend<'a> {
    Host,
    Docker(&'a str),
}

/// The answers of one decision, by (tool, minimum version).
pub(crate) type Answers = BTreeMap<(String, String), Option<(String, String)>>;

/// The decision's answers, and whether they came from the cache alone.
pub(crate) struct CachedDecision {
    pub(crate) decision: HostToolsDecision,
    pub(crate) reused: bool,
}

/// The key of a probe run now: [`probe_key`] with the environment's `PATH`
/// and locale. None when they are unset or not UTF-8.
pub(crate) fn probe_key_now(tools: &[ProbeTool], backend: &ProbeBackend<'_>) -> Option<String> {
    let path = env::var("PATH").ok()?;
    let locale = ["LANG", "LC_ALL", "LC_MESSAGES"]
        .iter()
        .map(|name| format!("{name}={}", env::var(name).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(" ");
    probe_key(tools, backend, &path, &locale)
}

/// A digest of everything the answers depend on: Gaia's version and binary
/// (the stub text and probe code), the backend (for docker, the image's id),
/// `PATH` and locale, and for each tool its config and policy and, for a
/// host probe that asks for the system tool, every `PATH` entry up to the
/// tool's binary with its identity (device, inode, size, mode, mtime and
/// ctime in ns, and the canonical path). None when any of that cannot be
/// read, or `PATH` has a relative entry (its answer depends on the working
/// directory).
pub(crate) fn probe_key(
    tools: &[ProbeTool],
    backend: &ProbeBackend<'_>,
    path: &str,
    locale: &str,
) -> Option<String> {
    let mut lines = vec![
        CACHE_HEADER.to_string(),
        format!("gaia {}", env!("CARGO_PKG_VERSION")),
        format!("exe {}", current_exe_identity()?),
        format!("PATH {path}"),
        format!("locale {locale}"),
    ];
    match backend {
        ProbeBackend::Host => {
            lines.push("backend host".to_string());
            for tool in tools.iter().filter(|tool| tool.enabled && tool.system) {
                path_search(tool.name, path, &mut lines)?;
            }
        }
        ProbeBackend::Docker(image) => {
            lines.push(format!(
                "backend docker {image} {}",
                docker_image_id(image)?
            ));
        }
    }
    for tool in tools {
        lines.push(format!(
            "tool {} min={} enabled={} system={} steps={}",
            tool.name, tool.min_version, tool.enabled, tool.system, tool.steps
        ));
    }
    Some(digest(&lines.join("\n")))
}

/// Every `PATH` entry's candidate for `name`, in order, up to the first
/// executable regular file (what `command -v` finds).
fn path_search(name: &str, path: &str, lines: &mut Vec<String>) -> Option<()> {
    for dir in path.split(':') {
        if !dir.starts_with('/') {
            return None;
        }
        let candidate = Path::new(dir).join(name);
        match fs::metadata(&candidate) {
            Ok(meta) => {
                let canonical = fs::canonicalize(&candidate).ok()?;
                lines.push(format!(
                    "found {} {} {}",
                    candidate.display(),
                    canonical.display(),
                    identity(&meta)
                ));
                if meta.is_file() && meta.mode() & 0o111 != 0 {
                    return Some(());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                lines.push(format!("absent {}", candidate.display()));
            }
            Err(_) => return None,
        }
    }
    Some(())
}

fn current_exe_identity() -> Option<String> {
    let exe = env::current_exe().ok()?;
    let meta = fs::metadata(&exe).ok()?;
    Some(format!("{} {}", exe.display(), identity(&meta)))
}

fn identity(meta: &fs::Metadata) -> String {
    format!(
        "dev={} ino={} size={} mode={:o} mtime={}.{:09} ctime={}.{:09}",
        meta.dev(),
        meta.ino(),
        meta.size(),
        meta.mode(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec()
    )
}

/// The image's id (`sha256:...`), or None when docker cannot say.
fn docker_image_id(image: &str) -> Option<String> {
    let output = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .output()
        .ok()?;
    let id = String::from_utf8(output.stdout).ok()?;
    let id = id.trim();
    (output.status.success() && id.starts_with("sha256:")).then(|| id.to_string())
}

fn digest(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The answers recorded under `key` in `output_dir`, when the file is intact
/// and its key is `key`. Anything else (missing, corrupt, another key) is None.
pub(crate) fn read_answers(output_dir: &Path, key: &str) -> Option<Answers> {
    let text = fs::read_to_string(output_dir.join(PROBE_CACHE_FILE)).ok()?;
    let without_newline = text.strip_suffix('\n')?;
    let (body_end, check_line) = without_newline.rsplit_once('\n')?;
    let body = &text[..body_end.len() + 1];
    if check_line.strip_prefix("check ")? != digest(body) {
        return None;
    }
    let mut lines = body.lines();
    if lines.next()? != CACHE_HEADER || lines.next()? != format!("key {key}") {
        return None;
    }
    let mut answers = Answers::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        let ["answer", name, min_version, path, version] = fields[..] else {
            return None;
        };
        let answer = match (path, version) {
            ("-", "-") => None,
            (path, version) if path.starts_with('/') && version != "-" => {
                Some((path.to_string(), version.to_string()))
            }
            _ => return None,
        };
        if name.is_empty() || min_version.is_empty() {
            return None;
        }
        if answers
            .insert((name.to_string(), min_version.to_string()), answer)
            .is_some()
        {
            return None;
        }
    }
    Some(answers)
}

/// Records `answers` under `key` in `output_dir`, replacing the file.
pub(crate) fn write_answers(output_dir: &Path, key: &str, answers: &Answers) -> io::Result<()> {
    let mut body = format!("{CACHE_HEADER}\nkey {key}\n");
    for ((name, min_version), answer) in answers {
        let (path, version) = match answer {
            Some((path, version)) => (path.as_str(), version.as_str()),
            None => ("-", "-"),
        };
        if [name, min_version, path, version]
            .iter()
            .any(|field| field.contains(['\t', '\n']))
        {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "tab in a probe"));
        }
        body.push_str(&format!(
            "answer\t{name}\t{min_version}\t{path}\t{version}\n"
        ));
    }
    let contents = format!("{body}check {}\n", digest(&body));
    let file = output_dir.join(PROBE_CACHE_FILE);
    let temp = output_dir.join(format!("{PROBE_CACHE_FILE}.tmp"));
    fs::write(&temp, contents)?;
    fs::rename(temp, file)
}

/// [`decide_host_tools`] with the probes answered from the cache when it
/// holds `key`'s answers, and the answers recorded when any probe ran.
/// Without a key (an input could not be read) every probe runs.
pub(crate) fn decide_with_probe_cache(
    output_dir: &Path,
    config: &str,
    policy: &gaia_spec::BuildrootHostToolsPolicySpec,
    previous: &str,
    key: Option<&str>,
    probe: &mut ProbeFn<'_>,
) -> Result<CachedDecision, ImageProviderError> {
    let cached = key.and_then(|key| read_answers(output_dir, key));
    let mut answers = Answers::new();
    let mut probed = false;
    let decision = decide_host_tools(config, policy, previous, &mut |name, min_version| {
        let answer_key = (name.to_string(), min_version.to_string());
        if let Some(answer) = cached.as_ref().and_then(|cached| cached.get(&answer_key)) {
            answers.insert(answer_key, answer.clone());
            return Ok(answer.clone());
        }
        probed = true;
        let answer = probe(name, min_version)?;
        answers.insert(answer_key, answer.clone());
        Ok(answer)
    })?;
    if let Some(key) = key.filter(|_| probed && !answers.is_empty()) {
        // A failed write only means the next build probes again.
        let _ = write_answers(output_dir, key, &answers);
    }
    Ok(CachedDecision {
        decision,
        reused: cached.is_some() && !probed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::HostToolStepSpec;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("gaia-probe-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn tool(name: &'static str, enabled: bool, steps: &str) -> ProbeTool {
        ProbeTool {
            name,
            min_version: "4.0",
            enabled,
            system: steps.contains("System"),
            steps: steps.to_string(),
        }
    }

    fn tools(enabled: bool) -> Vec<ProbeTool> {
        vec![tool("ccache", enabled, "[System, Build]")]
    }

    fn fake_tool(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\necho {body}\n")).expect("tool");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
        path
    }

    fn key(tools: &[ProbeTool], path: &str) -> Option<String> {
        probe_key(tools, &ProbeBackend::Host, path, "LANG=C")
    }

    #[test]
    fn key_is_stable_while_nothing_changes() {
        let dir = temp_dir("stable");
        fake_tool(&dir, "ccache", "one");
        let path = dir.display().to_string();
        assert!(key(&tools(true), &path).is_some());
        assert_eq!(key(&tools(true), &path), key(&tools(true), &path));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn key_changes_when_a_tool_binary_is_replaced() {
        let dir = temp_dir("replace");
        let path = dir.display().to_string();
        fake_tool(&dir, "ccache", "one");
        let before = key(&tools(true), &path);
        // A new file under the same name: a new inode.
        let replacement = dir.join("ccache.new");
        fs::write(&replacement, "#!/bin/sh\necho two\n").expect("new");
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755)).expect("mode");
        fs::rename(&replacement, dir.join("ccache")).expect("replace");
        assert_ne!(before, key(&tools(true), &path));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn key_changes_when_a_tool_binary_is_touched() {
        let dir = temp_dir("touch");
        let path = dir.display().to_string();
        let binary = fake_tool(&dir, "ccache", "one");
        let before = key(&tools(true), &path);
        let file = fs::File::options().write(true).open(&binary).expect("open");
        file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000))
            .expect("touch");
        assert_ne!(before, key(&tools(true), &path));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn key_changes_when_path_changes_or_a_shadowing_tool_appears() {
        let first = temp_dir("path-first");
        let second = temp_dir("path-second");
        fake_tool(&second, "ccache", "one");
        let only_second = key(&tools(true), &second.display().to_string());
        let both = format!("{}:{}", first.display(), second.display());
        let shadowed_later = key(&tools(true), &both);
        assert_ne!(only_second, shadowed_later);
        // A tool appearing in an earlier PATH entry changes the answer.
        fake_tool(&first, "ccache", "one");
        assert_ne!(shadowed_later, key(&tools(true), &both));
        let _ = fs::remove_dir_all(first);
        let _ = fs::remove_dir_all(second);
    }

    #[test]
    fn key_changes_when_a_config_symbol_or_policy_changes() {
        let dir = temp_dir("config").display().to_string();
        assert_ne!(key(&tools(true), &dir), key(&tools(false), &dir));
        let build_only = vec![tool("ccache", true, "[Build]")];
        assert_ne!(key(&tools(true), &dir), key(&build_only, &dir));
    }

    #[test]
    fn relative_path_entries_and_docker_without_an_image_have_no_key() {
        let dir = temp_dir("relative");
        let relative = format!(".:{}", dir.display());
        assert_eq!(key(&tools(true), &relative), None);
        assert_eq!(key(&tools(true), ""), None);
        assert_eq!(
            probe_key(
                &tools(true),
                &ProbeBackend::Docker("gaia-test-no-such-image:none"),
                "/usr/bin",
                ""
            ),
            None
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recorded_answers_are_reused_only_under_their_key() {
        let dir = temp_dir("roundtrip");
        let mut answers = Answers::new();
        answers.insert(
            ("ccache".into(), "4.0".into()),
            Some(("/usr/bin/ccache".into(), "4.10.2".into())),
        );
        answers.insert(("pkgconf".into(), "1.8".into()), None);
        write_answers(&dir, "key-a", &answers).expect("write");
        assert_eq!(read_answers(&dir, "key-a"), Some(answers));
        assert_eq!(read_answers(&dir, "key-b"), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_cache_is_ignored() {
        let dir = temp_dir("corrupt");
        let mut answers = Answers::new();
        answers.insert(
            ("ccache".into(), "4.0".into()),
            Some(("/usr/bin/ccache".into(), "4.10.2".into())),
        );
        write_answers(&dir, "key-a", &answers).expect("write");
        let file = dir.join(PROBE_CACHE_FILE);
        let good = fs::read_to_string(&file).expect("read");
        // An edited answer with the old check line.
        fs::write(&file, good.replace("4.10.2", "9.9.9")).expect("edit");
        assert_eq!(read_answers(&dir, "key-a"), None);
        // Truncated, and garbage.
        fs::write(&file, &good[..good.len() / 2]).expect("truncate");
        assert_eq!(read_answers(&dir, "key-a"), None);
        fs::write(&file, "not a cache\n").expect("garbage");
        assert_eq!(read_answers(&dir, "key-a"), None);
        fs::write(&file, "").expect("empty");
        assert_eq!(read_answers(&dir, "key-a"), None);
        let _ = fs::remove_dir_all(dir);
    }

    fn system_ccache_policy() -> gaia_spec::BuildrootHostToolsPolicySpec {
        let mut policy = gaia_spec::BuildrootHostToolsPolicySpec::default();
        policy.tools.insert(
            "ccache".to_string(),
            vec![HostToolStepSpec::System, HostToolStepSpec::Build],
        );
        policy
    }

    #[test]
    fn decisions_reuse_the_probe_answers_until_an_input_changes() {
        let dir = temp_dir("decide");
        let config = "BR2_CCACHE=y\n";
        let policy = system_ccache_policy();
        let probes = AtomicUsize::new(0);
        let mut probe = |name: &str, _min: &str| {
            probes.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ImageProviderError>(match name {
                "ccache" => Some(("/usr/bin/ccache".to_string(), "4.10.2".to_string())),
                _ => None,
            })
        };
        let first = decide_with_probe_cache(&dir, config, &policy, "", Some("k1"), &mut probe)
            .expect("first");
        assert!(!first.reused);
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        assert!(dir.join(PROBE_CACHE_FILE).is_file());

        let second = decide_with_probe_cache(
            &dir,
            config,
            &policy,
            &first.decision.decisions,
            Some("k1"),
            &mut probe,
        )
        .expect("second");
        assert!(second.reused);
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        assert_eq!(second.decision, first.decision);

        // Another key, no key, or a corrupt file: probe as before.
        for key in [Some("k2"), None] {
            let again =
                decide_with_probe_cache(&dir, config, &policy, "", key, &mut probe).expect("again");
            assert!(!again.reused);
        }
        fs::write(dir.join(PROBE_CACHE_FILE), "junk\n").expect("corrupt");
        let corrupt = decide_with_probe_cache(&dir, config, &policy, "", Some("k1"), &mut probe)
            .expect("corrupt");
        assert!(!corrupt.reused);
        assert_eq!(corrupt.decision, first.decision);
        let _ = fs::remove_dir_all(dir);
    }
}
