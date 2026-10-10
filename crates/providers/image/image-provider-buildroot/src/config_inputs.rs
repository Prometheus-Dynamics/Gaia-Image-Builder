//! Whether the Buildroot config steps (defconfig, fragments, config
//! overrides, cache settings) must run again.
//!
//! Those steps rebuild `.config` from their inputs. When the inputs are the
//! ones the tree's `.config` was last produced from, and the `.config` is
//! still exactly what those steps wrote, they are skipped: the build
//! operation repeats the prepare operation's configuration otherwise.
//!
//! The state file holds the inputs digest and the sha256 of the `.config`
//! the steps left. Any input this module does not hash is one whose change
//! must still reach the digest, so it covers: the Buildroot version (its
//! Makefile), every Kconfig file of the Buildroot tree and of the external
//! tree (their contents, not mtimes, so re-materialized trees do not look
//! changed), the defconfig and fragment files, the config overrides, the
//! cache settings Gaia writes into `.config`, and the package replacements.
use super::*;
use sha2::{Digest, Sha256};

/// The state file next to `.config`.
pub(crate) const CONFIG_INPUTS_STATE: &str = ".gaia-buildroot-config-inputs";

/// Bumped when the digest's inputs change, so trees configured by an older
/// Gaia run the steps once.
const CONFIG_INPUTS_VERSION: &str = "gaia-buildroot-config-inputs-v1";

/// Buildroot directories whose Config.in files make up the configuration
/// language (`package/` and the others the top-level `Config.in` sources).
const BUILDROOT_KCONFIG_DIRS: &[&str] = &[
    "arch",
    "boot",
    "fs",
    "linux",
    "package",
    "system",
    "toolchain",
];

/// What the config steps read, for [`config_inputs_digest`].
pub(crate) struct ConfigInputs<'a> {
    pub(crate) buildroot_dir: &'a Path,
    /// The `BR2_EXTERNAL` tree, when there is one.
    pub(crate) external_tree: Option<&'a Path>,
    /// The resolved defconfig file, when `defconfig_path` is set.
    pub(crate) defconfig_file: Option<&'a Path>,
    /// The defconfig name, when given by name instead of by path.
    pub(crate) defconfig_name: Option<&'a str>,
    pub(crate) fragments: &'a [PathBuf],
    /// The normalized `config_overrides`.
    pub(crate) overrides: &'a [(String, String)],
    /// The settings the cache step writes (download, compiler cache, jobs).
    pub(crate) cache_overrides: &'a [(String, String)],
    /// Digest of the generated package replacements, when there are any.
    pub(crate) package_replacements: Option<&'a str>,
}

/// The digest of [`ConfigInputs`].
pub(crate) fn config_inputs_digest(inputs: &ConfigInputs<'_>) -> String {
    let mut hasher = Sha256::new();
    let mut feed = |label: &str, text: &str| {
        hasher.update(label.as_bytes());
        hasher.update(b"\0");
        hasher.update(text.as_bytes());
        hasher.update(b"\0");
    };
    feed("version", CONFIG_INPUTS_VERSION);
    feed(
        "buildroot-makefile",
        &file_digest(&inputs.buildroot_dir.join("Makefile")),
    );
    let mut kconfig = BTreeMap::new();
    for name in fs::read_dir(inputs.buildroot_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = name.path();
        if path.is_file() && is_kconfig_name(&name.file_name().to_string_lossy()) {
            kconfig.insert(name.file_name().to_string_lossy().into_owned(), path);
        }
    }
    for dir in BUILDROOT_KCONFIG_DIRS {
        collect_kconfig(&inputs.buildroot_dir.join(dir), dir, &mut kconfig);
    }
    if let Some(external) = inputs.external_tree {
        collect_kconfig(external, "external", &mut kconfig);
    }
    for (relative, path) in &kconfig {
        feed(&format!("kconfig:{relative}"), &file_digest(path));
    }
    if let Some(path) = inputs.defconfig_file {
        feed("defconfig", &file_digest(path));
    }
    if let Some(name) = inputs.defconfig_name {
        feed(
            &format!("defconfig-name:{name}"),
            &named_defconfig_digest(inputs, name),
        );
    }
    for fragment in inputs.fragments {
        feed(
            &format!("fragment:{}", fragment.display()),
            &file_digest(fragment),
        );
    }
    for (key, value) in inputs.overrides {
        feed(&format!("override:{key}"), value);
    }
    for (key, value) in inputs.cache_overrides {
        feed(&format!("cache:{key}"), value);
    }
    feed(
        "package-replacements",
        inputs.package_replacements.unwrap_or("none"),
    );
    hex(&hasher.finalize())
}

/// A defconfig given by name: the file Buildroot would read it from, the
/// external tree's first.
fn named_defconfig_digest(inputs: &ConfigInputs<'_>, name: &str) -> String {
    let candidates = inputs
        .external_tree
        .into_iter()
        .map(|external| external.join("configs"))
        .chain(std::iter::once(inputs.buildroot_dir.join("configs")))
        .flat_map(|dir| [dir.join(name), dir.join(format!("{name}_defconfig"))])
        .collect::<Vec<_>>();
    candidates
        .iter()
        .find(|path| path.is_file())
        .map(|path| file_digest(path))
        .unwrap_or_else(|| "missing".to_string())
}

fn is_kconfig_name(name: &str) -> bool {
    name.starts_with("Config.in") || name == "external.desc"
}

/// Adds every Kconfig file under `dir` to `out`, keyed by its path relative to
/// `label`.
fn collect_kconfig(dir: &Path, label: &str, out: &mut BTreeMap<String, PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let relative = format!("{label}/{name}");
        if kind.is_dir() {
            collect_kconfig(&path, &relative, out);
        } else if kind.is_file() && is_kconfig_name(&name) {
            out.insert(relative, path);
        }
    }
}

/// The sha256 of a file's contents, or a marker naming it when it is missing.
fn file_digest(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => hex(&Sha256::digest(&bytes)),
        Err(_) => format!("missing:{}", path.display()),
    }
}

/// Whether `output_dir` holds the `.config` the config steps wrote from
/// `digest`: the state records the digest and that file's sha256.
pub(crate) fn config_steps_current(output_dir: &Path, digest: &str) -> bool {
    let Ok(state) = fs::read_to_string(output_dir.join(CONFIG_INPUTS_STATE)) else {
        return false;
    };
    let mut lines = state.lines();
    let (Some(recorded_digest), Some(recorded_config)) = (lines.next(), lines.next()) else {
        return false;
    };
    recorded_digest == digest && config_file_digest(output_dir).as_deref() == Some(recorded_config)
}

/// Records that the config steps wrote `.config` from `digest`.
pub(crate) fn record_config_steps(
    output_dir: &Path,
    digest: &str,
) -> Result<(), ImageProviderError> {
    let Some(config) = config_file_digest(output_dir) else {
        return Ok(());
    };
    fs::write(
        output_dir.join(CONFIG_INPUTS_STATE),
        format!("{digest}\n{config}\n"),
    )
    .map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to write Buildroot config inputs state in '{}': {error}",
            output_dir.display()
        ))
    })
}

fn config_file_digest(output_dir: &Path) -> Option<String> {
    fs::read(output_dir.join(".config"))
        .ok()
        .map(|bytes| hex(&Sha256::digest(&bytes)))
}
