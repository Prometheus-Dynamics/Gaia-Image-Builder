//! Content keys of Buildroot packages for the package cache.
//!
//! A package's key covers everything its build output depends on, so two
//! builds (of any project) with the same key produce the same files:
//! - Buildroot's package infrastructure (`Makefile`, `package/Makefile.in`,
//!   `package/pkg-*.mk`, the toolchain helpers, `support/scripts`, the
//!   skeleton) and the values of the settings it references (architecture,
//!   toolchain, optimisation, hardening, init system, ...),
//! - the package's own `.mk` directory, `.hash` files and patches, its
//!   version and download names,
//! - the values of every setting its `.mk` files reference (a referenced
//!   file or directory by content, not path),
//! - where it runs (Docker image or host compiler),
//! - the keys of its dependencies.
//!
//! Packages built from a local directory (`SITE_METHOD = local`,
//! `<PKG>_OVERRIDE_SRCDIR`) have no key, and neither does anything depending
//! on them.
use super::*;
use sha2::{Digest, Sha256};

const KEY_FORMAT: &str = "gaia-buildroot-package-v1";

/// Infrastructure directories every package build uses, by content.
const INFRA_DIRS: &[&str] = &["support/scripts", "system/skeleton", "toolchain"];

pub(crate) struct KeyInputs<'a> {
    pub buildroot_dir: &'a Path,
    pub output_dir: &'a Path,
    pub graph: &'a PackageGraph,
    /// The Docker image or host compiler packages are built with.
    pub execution_identity: &'a str,
}

/// Every package's key, or `None` when it cannot be cached.
pub(crate) fn package_keys(inputs: &KeyInputs<'_>) -> BTreeMap<String, Option<String>> {
    let config = fs::read_to_string(inputs.output_dir.join(".config")).unwrap_or_default();
    let mut keys = KeyContext {
        inputs,
        config: config
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        local: locally_built_packages(inputs.output_dir),
        path_digests: BTreeMap::new(),
        keys: BTreeMap::new(),
        global: String::new(),
    };
    keys.global = keys.global_digest();
    let names = inputs
        .graph
        .package_names()
        .map(str::to_string)
        .collect::<Vec<_>>();
    for name in &names {
        keys.key(name, &mut BTreeSet::new());
    }
    keys.keys
}

struct KeyContext<'a> {
    inputs: &'a KeyInputs<'a>,
    config: BTreeMap<String, String>,
    /// Upper-case names of packages with an `_OVERRIDE_SRCDIR`.
    local: BTreeSet<String>,
    path_digests: BTreeMap<PathBuf, String>,
    keys: BTreeMap<String, Option<String>>,
    global: String,
}

impl KeyContext<'_> {
    fn global_digest(&mut self) -> String {
        let buildroot_dir = self.inputs.buildroot_dir;
        let mut files = vec![
            buildroot_dir.join("Makefile"),
            buildroot_dir.join("package/Makefile.in"),
        ];
        for (dir, prefix) in [("package", "pkg-"), ("toolchain", "")] {
            if let Ok(entries) = fs::read_dir(buildroot_dir.join(dir)) {
                files.extend(
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.is_file()
                                && path.extension().is_some_and(|extension| extension == "mk")
                                && path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .is_some_and(|name| name.starts_with(prefix))
                        }),
                );
            }
        }
        files.sort();
        let mut hasher = Sha256::new();
        hasher.update(KEY_FORMAT);
        hasher.update(b"\0");
        hasher.update(self.inputs.execution_identity);
        let mut symbols = BTreeSet::new();
        for file in &files {
            let contents = fs::read_to_string(file).unwrap_or_default();
            hasher.update(
                file.strip_prefix(buildroot_dir)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .as_bytes(),
            );
            hasher.update(b"\0");
            hasher.update(contents.as_bytes());
            symbols.extend(referenced_symbols(&contents).filter(|key| infra_setting(key)));
        }
        for dir in INFRA_DIRS {
            hasher.update(dir);
            hasher.update(self.path_digest(&buildroot_dir.join(dir)));
        }
        for symbol in symbols {
            self.update_setting(&mut hasher, &symbol);
        }
        hex(&hasher.finalize())
    }

    fn key(&mut self, name: &str, visiting: &mut BTreeSet<String>) -> Option<String> {
        if let Some(key) = self.keys.get(name) {
            return key.clone();
        }
        if !visiting.insert(name.to_string()) {
            return None;
        }
        let key = self.compute(name, visiting);
        visiting.remove(name);
        self.keys.insert(name.to_string(), key.clone());
        key
    }

    fn compute(&mut self, name: &str, visiting: &mut BTreeSet<String>) -> Option<String> {
        let package = self.inputs.graph.get(name)?.clone();
        let upper = upper_name(name);
        if self.local.contains(&upper) {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(self.global.as_bytes());
        for field in [
            name,
            &package.kind,
            package.version.as_deref().unwrap_or("-"),
            if package.is_virtual {
                "virtual"
            } else {
                "real"
            },
        ] {
            hasher.update(field);
            hasher.update(b"\0");
        }
        if let Some(package_dir) = &package.package_dir {
            let dir = self.resolve(package_dir)?;
            let mut makefiles = fs::read_dir(&dir)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().is_some_and(|extension| extension == "mk"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            makefiles.sort();
            let mut symbols = BTreeSet::new();
            for makefile in makefiles {
                let contents = fs::read_to_string(makefile).unwrap_or_default();
                let compact = contents.replace([' ', '\t'], "");
                if compact.contains("_SITE_METHOD=local") || compact.contains("_SITE_METHOD:=local")
                {
                    return None;
                }
                symbols.extend(referenced_symbols(&contents).filter(|key| package_setting(key)));
            }
            hasher.update(b"dir\0");
            hasher.update(self.path_digest(&dir));
            for symbol in symbols {
                self.update_setting(&mut hasher, &symbol);
            }
        }
        for file in package.hash_files.iter().chain(&package.patches) {
            let path = self.resolve(file)?;
            hasher.update(b"file\0");
            hasher.update(self.path_digest(&path));
        }
        for source in &package.sources {
            hasher.update(b"source\0");
            hasher.update(source);
        }
        for dependency in &package.dependencies {
            let dependency_key = self.key(dependency, visiting)?;
            hasher.update(b"dep\0");
            hasher.update(dependency);
            hasher.update(dependency_key);
        }
        Some(hex(&hasher.finalize()))
    }

    /// A setting's value, with files and directories it names replaced by
    /// their content digests so the path does not matter.
    fn update_setting(&mut self, hasher: &mut Sha256, key: &str) {
        hasher.update(key);
        hasher.update(b"=");
        let Some(value) = self.config.get(key).cloned() else {
            hasher.update(b"\0unset\0");
            return;
        };
        for token in value.trim_matches('"').split_whitespace() {
            let resolved = (token.contains('/') && !token.contains("$("))
                .then(|| self.resolve(token))
                .flatten();
            match resolved {
                Some(path) => {
                    hasher.update(b"path:");
                    hasher.update(self.path_digest(&path));
                }
                None => hasher.update(token),
            }
            hasher.update(b" ");
        }
        hasher.update(b"\0");
    }

    /// An existing path, absolute or relative to the Buildroot source or the
    /// output directory.
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let raw = Path::new(path);
        let candidates = if raw.is_absolute() {
            vec![raw.to_path_buf()]
        } else {
            vec![
                self.inputs.buildroot_dir.join(raw),
                self.inputs.output_dir.join(raw),
            ]
        };
        candidates.into_iter().find(|candidate| candidate.exists())
    }

    fn path_digest(&mut self, path: &Path) -> String {
        if let Some(digest) = self.path_digests.get(path) {
            return digest.clone();
        }
        let digest = path_content_digest(path);
        self.path_digests.insert(path.to_path_buf(), digest.clone());
        digest
    }
}

/// `BR2_*` names a makefile references.
fn referenced_symbols(contents: &str) -> impl Iterator<Item = String> + '_ {
    contents.match_indices("BR2_").filter_map(|(start, _)| {
        let preceded_by_identifier = contents[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let name = contents[start..]
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .next()?;
        (!preceded_by_identifier && name.len() > 4).then(|| name.to_string())
    })
}

/// Settings outside any package's own that can change a package build.
/// Package selections, kernel, bootloader and filesystem image settings,
/// paths that only say where things are, and settings that never change a
/// build are left to the packages that reference them.
fn infra_setting(key: &str) -> bool {
    let package_or_image = key.starts_with("BR2_PACKAGE_")
        || key.starts_with("BR2_LINUX_KERNEL")
        || key.starts_with("BR2_ROOTFS_")
        || (key.starts_with("BR2_TARGET_")
            && !matches!(key, "BR2_TARGET_OPTIMIZATION" | "BR2_TARGET_LDFLAGS"));
    !package_or_image && package_setting(key)
}

fn package_setting(key: &str) -> bool {
    !key.starts_with("BR2_EXTERNAL")
        && !matches!(
            key,
            "BR2_DEFCONFIG"
                | "BR2_GLOBAL_PATCH_DIR"
                | "BR2_PACKAGE_OVERRIDE_FILE"
                | "BR2_HOST_DIR"
                | "BR2_CCACHE"
                | "BR2_CCACHE_USE_BASEDIR"
        )
        && setting_requires_clean(key)
}

fn upper_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Packages `local.mk` builds from a local source directory.
fn locally_built_packages(output_dir: &Path) -> BTreeSet<String> {
    fs::read_to_string(output_dir.join("local.mk"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (variable, _) = line.split_once('=')?;
            let variable = variable.trim().trim_end_matches([':', '?', '+']).trim();
            variable
                .strip_suffix("_OVERRIDE_SRCDIR")
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        buildroot: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "gaia-package-keys-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("time")
                    .as_nanos()
            ));
            let buildroot = root.join("buildroot");
            for (path, contents) in [
                ("Makefile", "ifeq ($(BR2_OPTIMIZE_2),y)\nendif\n"),
                (
                    "package/Makefile.in",
                    "TARGET_CFLAGS += $(BR2_TARGET_OPTIMIZATION)\n",
                ),
                ("package/pkg-generic.mk", "# generic\n"),
                ("package/zlib/zlib.mk", "ZLIB_VERSION = 1.3\n"),
                ("package/zlib/zlib.hash", "sha256 abc zlib-1.3.tar.xz\n"),
                (
                    "package/libpng/libpng.mk",
                    "LIBPNG_VERSION = 1.6\nifeq ($(BR2_PACKAGE_LIBPNG_TOOLS),y)\nendif\n\
                     LIBPNG_CONF = $(BR2_PACKAGE_LIBPNG_CONFIG_FILE)\n",
                ),
                (
                    "package/local-app/local-app.mk",
                    "LOCAL_APP_SITE_METHOD = local\n",
                ),
                ("package/app/app.mk", "APP_VERSION = 1\n"),
                ("files/libpng.conf", "a\n"),
            ] {
                let path = buildroot.join(path);
                fs::create_dir_all(path.parent().expect("parent")).expect("dir");
                fs::write(path, contents).expect("file");
            }
            Self { root, buildroot }
        }

        fn output(&self, name: &str, config: &str) -> PathBuf {
            let output = self.root.join(name);
            fs::create_dir_all(&output).expect("output");
            fs::write(output.join(".config"), config).expect("config");
            output
        }

        fn keys(&self, output: &Path, graph: &PackageGraph) -> BTreeMap<String, Option<String>> {
            package_keys(&KeyInputs {
                buildroot_dir: &self.buildroot,
                output_dir: output,
                graph,
                execution_identity: "host:test",
            })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn graph() -> PackageGraph {
        let mut graph = PackageGraph::default();
        for (name, dependencies) in [
            ("zlib", &[][..]),
            ("libpng", &["zlib"][..]),
            ("local-app", &["zlib"][..]),
            ("app", &["local-app"][..]),
        ] {
            graph.packages.insert(
                name.to_string(),
                PackageInfo {
                    kind: "target".to_string(),
                    version: Some("1".to_string()),
                    stamp_dir: Some(format!("build/{name}-1")),
                    package_dir: Some(format!("package/{name}")),
                    sources: vec![format!("{name}-1.tar.xz")],
                    hash_files: if name == "zlib" {
                        vec!["package/zlib/zlib.hash".to_string()]
                    } else {
                        Vec::new()
                    },
                    dependencies: dependencies.iter().map(|d| d.to_string()).collect(),
                    ..PackageInfo::default()
                },
            );
        }
        graph
    }

    const CONFIG: &str = "BR2_aarch64=y\nBR2_OPTIMIZE_2=y\nBR2_PACKAGE_ZLIB=y\nBR2_PACKAGE_LIBPNG=y\n\
                          BR2_PACKAGE_LIBPNG_CONFIG_FILE=\"files/libpng.conf\"\n\
                          BR2_DEFCONFIG=\"/work/a/defconfig\"\nBR2_PACKAGE_HTOP=y\n";

    #[test]
    fn keys_do_not_depend_on_where_the_tree_is() {
        let fixture = Fixture::new("location");
        let first = fixture.output("first", CONFIG);
        let second = fixture.output(
            "elsewhere",
            &CONFIG.replace("/work/a/defconfig", "/work/b/other.defconfig"),
        );
        let keys = fixture.keys(&first, &graph());
        assert!(keys["zlib"].is_some() && keys["libpng"].is_some());
        // An unrelated package selection (htop) does not matter either.
        assert_eq!(keys, fixture.keys(&second, &graph()));
    }

    #[test]
    fn local_packages_and_their_dependents_have_no_key() {
        let fixture = Fixture::new("local");
        let keys = fixture.keys(&fixture.output("out", CONFIG), &graph());
        assert_eq!(keys["local-app"], None);
        assert_eq!(keys["app"], None);
    }

    #[test]
    fn keys_follow_package_files_settings_and_dependencies() {
        let fixture = Fixture::new("inputs");
        let output = fixture.output("out", CONFIG);
        let before = fixture.keys(&output, &graph());

        // A referenced file's content (not its path).
        fs::write(fixture.buildroot.join("files/libpng.conf"), "b\n").expect("conf");
        let after = fixture.keys(&output, &graph());
        assert_eq!(before["zlib"], after["zlib"]);
        assert_ne!(before["libpng"], after["libpng"]);

        // A dependency's definition changes its dependents' keys.
        fs::write(
            fixture.buildroot.join("package/zlib/zlib.hash"),
            "sha256 def\n",
        )
        .expect("hash");
        let changed = fixture.keys(&output, &graph());
        assert_ne!(after["zlib"], changed["zlib"]);
        assert_ne!(after["libpng"], changed["libpng"]);

        // A setting the package references, and one the infrastructure does.
        let tools = fixture.output("tools", &format!("{CONFIG}BR2_PACKAGE_LIBPNG_TOOLS=y\n"));
        let with_tools = fixture.keys(&tools, &graph());
        assert_eq!(changed["zlib"], with_tools["zlib"]);
        assert_ne!(changed["libpng"], with_tools["libpng"]);
        let optimized = fixture.output("o3", &CONFIG.replace("BR2_OPTIMIZE_2", "BR2_OPTIMIZE_3"));
        assert_ne!(changed["zlib"], fixture.keys(&optimized, &graph())["zlib"]);
    }
}
