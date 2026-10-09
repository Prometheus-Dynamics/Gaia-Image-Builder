//! What reads a `.config` setting that belongs to no package, so a change to
//! it rebuilds only what it can affect:
//! - Buildroot's infrastructure (the top `Makefile`, `package/Makefile.in`,
//!   `package/pkg-*.mk`, `toolchain/`, `arch/`, `system/system.mk`), or an
//!   unknown use in an external tree's `external.mk`: every package may
//!   change, so a full clean.
//! - Packages' `.mk` files, or `<PKG>_*` variables and hooks an
//!   `external.mk` sets from it: those packages are rebuilt.
//! - Only the target finalize step (overlays, post-build scripts, hostname,
//!   `PACKAGES_USERS` and the other tables, `TARGET_FINALIZE_HOOKS`), or
//!   nothing at all: no package is rebuilt; `target/` is reassembled from
//!   the per-package directories.
use super::*;

/// Settings only target finalization reads (some through hooks that
/// packages such as `skeleton-init-common` register).
const FINALIZE_SETTINGS: &[&str] = &[
    "BR2_ROOTFS_OVERLAY",
    "BR2_ROOTFS_POST_BUILD_SCRIPT",
    "BR2_ROOTFS_POST_SCRIPT_ARGS",
    "BR2_TARGET_GENERIC_HOSTNAME",
    "BR2_TARGET_GENERIC_ISSUE",
    "BR2_TARGET_GENERIC_ROOT_PASSWD",
    "BR2_TARGET_ENABLE_ROOT_LOGIN",
];

/// Variables Buildroot expands only while finalizing the target and writing
/// the root filesystem.
const FINALIZE_VARIABLES: &[&str] = &[
    "PACKAGES_USERS",
    "PACKAGES_PERMISSIONS_TABLE",
    "PACKAGES_DEVICES_TABLE",
    "TARGET_FINALIZE_HOOKS",
    "ROOTFS_PRE_CMD_HOOKS",
    "ROOTFS_POST_CMD_HOOKS",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SymbolUse {
    /// Why every package may change, when one may.
    pub global: Option<String>,
    /// Packages whose build reads it.
    pub packages: BTreeSet<String>,
    /// Target finalization reads it.
    pub finalize: bool,
}

/// The makefiles a setting's use is looked up in.
pub(crate) struct SymbolIndex {
    /// Buildroot infrastructure makefiles, by path relative to Buildroot.
    infra: Vec<(String, String)>,
    /// Each package's `.mk` contents.
    packages: Vec<(String, String)>,
    /// The external trees' `external.mk` files, parsed together.
    external: ExternalMakefiles,
    /// Upper-case package names, longest first, for `<PKG>_*` variables.
    package_prefixes: Vec<(String, String)>,
}

impl SymbolIndex {
    /// `br2_external` is the `BR2_EXTERNAL` value (trees separated by
    /// spaces or colons); `graphs` the current and previous package graphs.
    pub(crate) fn load(
        buildroot_dir: &Path,
        br2_external: Option<&str>,
        graphs: &[Option<&PackageGraph>],
    ) -> Self {
        let external_dirs = br2_external
            .unwrap_or_default()
            .split([' ', ':'])
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let mut infra_files = vec![
            buildroot_dir.join("Makefile"),
            buildroot_dir.join("package/Makefile.in"),
            buildroot_dir.join("system/system.mk"),
        ];
        for (dir, prefix, recursive) in [
            ("package", "pkg-", false),
            ("toolchain", "", true),
            ("arch", "", false),
        ] {
            collect_makefiles(
                &buildroot_dir.join(dir),
                prefix,
                recursive,
                &mut infra_files,
            );
        }
        let infra = infra_files
            .into_iter()
            .filter_map(|file| {
                let contents = fs::read_to_string(&file).ok()?;
                let label = file
                    .strip_prefix(buildroot_dir)
                    .unwrap_or(&file)
                    .display()
                    .to_string();
                Some((label, contents))
            })
            .collect();

        let mut names = BTreeMap::new();
        for graph in graphs.iter().flatten() {
            for (name, package) in &graph.packages {
                names
                    .entry(name.clone())
                    .or_insert_with(|| package.package_dir.clone());
            }
        }
        let packages = names
            .iter()
            .filter_map(|(name, dir)| {
                let dir = Path::new(dir.as_deref()?);
                let dir = if dir.is_absolute() {
                    dir.to_path_buf()
                } else {
                    buildroot_dir.join(dir)
                };
                let mut makefiles = Vec::new();
                collect_makefiles(&dir, "", false, &mut makefiles);
                makefiles.sort();
                let contents = makefiles
                    .iter()
                    .filter_map(|file| fs::read_to_string(file).ok())
                    .collect::<Vec<_>>()
                    .join("\n");
                Some((name.clone(), contents))
            })
            .collect();
        let mut package_prefixes = names
            .keys()
            .map(|name| (upper(name), name.clone()))
            .collect::<Vec<_>>();
        package_prefixes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        let external = ExternalMakefiles::parse(
            &external_dirs
                .iter()
                .filter_map(|dir| fs::read_to_string(dir.join("external.mk")).ok())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        Self {
            infra,
            packages,
            external,
            package_prefixes,
        }
    }

    pub(crate) fn symbol_use(&self, key: &str) -> SymbolUse {
        let mut found = SymbolUse::default();
        // Architecture and CPU choices (`BR2_aarch64`, `BR2_cortex_a76`).
        if key
            .strip_prefix("BR2_")
            .and_then(|rest| rest.chars().next())
            .is_some_and(|first| first.is_ascii_lowercase())
        {
            found.global = Some("architecture or CPU setting".to_string());
            return found;
        }
        if FINALIZE_SETTINGS.contains(&key) {
            found.finalize = true;
            return found;
        }
        if let Some((file, _)) = self
            .infra
            .iter()
            .find(|(_, contents)| has_word(contents, key))
        {
            found.global = Some(format!("read by Buildroot's {file}"));
            return found;
        }
        found.packages = self.packages_reading(key);

        // `external.mk`: follow the variables it sets from the setting.
        let affected = self.external.affected_by(key);
        if self.external.used_outside_assignments(key, &affected) {
            found.global = Some("used by an external tree's external.mk".to_string());
            return found;
        }
        for variable in &affected {
            if FINALIZE_VARIABLES.contains(&variable.as_str()) {
                found.finalize = true;
            } else if let Some(package) = self.package_of(variable) {
                // A hook macro counts through the hook lists naming it.
                if !self.external.macros.contains(variable) {
                    found.packages.insert(package);
                }
            } else if self.external.local.contains(variable) {
                // The tree's own variable: what else reads it.
                if let Some((file, _)) = self
                    .infra
                    .iter()
                    .find(|(_, contents)| has_word(contents, variable))
                {
                    found.global = Some(format!(
                        "external.mk sets {variable}, which Buildroot's {file} reads"
                    ));
                    return found;
                }
                found.packages.extend(self.packages_reading(variable));
            } else {
                found.global = Some(format!("external.mk changes {variable} with it"));
                return found;
            }
        }
        found
    }

    fn packages_reading(&self, word: &str) -> BTreeSet<String> {
        self.packages
            .iter()
            .filter(|(_, contents)| has_word(contents, word))
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn package_of(&self, variable: &str) -> Option<String> {
        self.package_prefixes
            .iter()
            .find(|(prefix, _)| {
                variable
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with('_'))
            })
            .map(|(_, name)| name.clone())
    }
}

/// The variables (and `define` macros) external trees' `external.mk` files
/// set, with every name their value or enclosing conditionals mention.
#[derive(Debug, Default)]
pub(crate) struct ExternalMakefiles {
    assignments: Vec<(String, BTreeSet<String>)>,
    /// Names mentioned by statements that are not assignments (includes,
    /// rules, `$(eval ...)`), with their enclosing conditionals.
    other: Vec<BTreeSet<String>>,
    /// Variables these files define (`=`, `:=`, `?=`, `define`) rather than
    /// append to.
    local: BTreeSet<String>,
    macros: BTreeSet<String>,
}

impl ExternalMakefiles {
    pub(crate) fn parse(contents: &str) -> Self {
        let mut parsed = Self::default();
        let mut conditions: Vec<BTreeSet<String>> = Vec::new();
        let mut macro_body: Option<(String, BTreeSet<String>)> = None;
        for line in logical_lines(contents) {
            let trimmed = line.trim();
            let keyword = trimmed.split_whitespace().next().unwrap_or("");
            if let Some((name, body)) = &mut macro_body {
                if keyword == "endef" {
                    let mut inputs = std::mem::take(body);
                    inputs.extend(conditions.iter().flatten().cloned());
                    parsed.macros.insert(name.clone());
                    parsed.local.insert(name.clone());
                    parsed.assignments.push((name.clone(), inputs));
                    macro_body = None;
                } else {
                    body.extend(words(&line));
                }
                continue;
            }
            let code = trimmed.split('#').next().unwrap_or("").trim();
            if code.is_empty() {
                continue;
            }
            match keyword {
                "ifeq" | "ifneq" | "ifdef" | "ifndef" => conditions.push(words(code)),
                "else" => {
                    // `else ifeq (...)` adds its own condition to the block.
                    if let Some(condition) = conditions.last_mut() {
                        condition.extend(words(code));
                    }
                }
                "endif" => {
                    conditions.pop();
                }
                "define" => {
                    let name = code
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    macro_body = Some((name, BTreeSet::new()));
                }
                _ => match assignment(code) {
                    Some((name, operator, value)) if !line.starts_with('\t') => {
                        let mut inputs = words(value);
                        inputs.extend(conditions.iter().flatten().cloned());
                        if operator != "+=" {
                            parsed.local.insert(name.to_string());
                        }
                        parsed.assignments.push((name.to_string(), inputs));
                    }
                    _ => {
                        let mut mentioned = words(code);
                        mentioned.extend(conditions.iter().flatten().cloned());
                        parsed.other.push(mentioned);
                    }
                },
            }
        }
        parsed
    }

    /// Every variable whose value can change with `key`, transitively.
    pub(crate) fn affected_by(&self, key: &str) -> BTreeSet<String> {
        let mut affected = BTreeSet::new();
        let mut changed = true;
        while changed {
            changed = false;
            for (name, inputs) in &self.assignments {
                if !affected.contains(name)
                    && (inputs.contains(key) || inputs.iter().any(|input| affected.contains(input)))
                {
                    affected.insert(name.clone());
                    changed = true;
                }
            }
        }
        affected
    }

    fn used_outside_assignments(&self, key: &str, affected: &BTreeSet<String>) -> bool {
        self.other.iter().any(|mentioned| {
            mentioned.contains(key) || mentioned.iter().any(|name| affected.contains(name))
        })
    }
}

/// `NAME op value` of a make assignment line.
fn assignment(line: &str) -> Option<(&str, &str, &str)> {
    let line = line
        .strip_prefix("override ")
        .or_else(|| line.strip_prefix("export "))
        .unwrap_or(line)
        .trim_start();
    let end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(line.len());
    let (name, rest) = line.split_at(end);
    let rest = rest.trim_start();
    if name.is_empty() {
        return None;
    }
    ["+=", "::=", ":=", "?=", "="].iter().find_map(|operator| {
        rest.strip_prefix(operator)
            .map(|value| (name, *operator, value))
    })
}

/// Lines with `\` continuations joined.
fn logical_lines(contents: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in contents.lines() {
        match line.strip_suffix('\\') {
            Some(continued) => {
                current.push_str(continued);
                current.push(' ');
            }
            None => {
                current.push_str(line);
                lines.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Every identifier in `text` (a superset of the variables it expands).
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

fn has_word(contents: &str, word: &str) -> bool {
    contents.match_indices(word).any(|(start, _)| {
        let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
        !contents[..start]
            .chars()
            .next_back()
            .is_some_and(identifier)
            && !contents[start + word.len()..]
                .chars()
                .next()
                .is_some_and(identifier)
    })
}

fn upper(name: &str) -> String {
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

fn collect_makefiles(dir: &Path, prefix: &str, recursive: bool, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                collect_makefiles(&path, prefix, recursive, files);
            }
        } else if path.extension().is_some_and(|extension| extension == "mk")
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTERNAL_MK: &str = r#"
LINUX_HASH_FILES += $(BR2_EXTERNAL_RAZE_DEVICE_PATH)/linux/linux.hash

ifeq ($(BR2_PACKAGE_OPENJDK),y)
OPENJDK_CONF_OPTS += \
	OBJCOPY=$(TARGET_OBJCOPY) \
	STRIP=$(TARGET_STRIP)
endif

RAZE_KERNEL_OVERLAYS = dwc2 i2c1-pi5

ifeq ($(BR2_LINUX_KERNEL_EXT_OV9782),y)
define RAZE_BUILD_KERNEL_OVERLAYS
	$(LINUX_MAKE_ENV) $(BR2_MAKE) -C $(LINUX_DIR) $(RAZE_KERNEL_OVERLAYS)
endef
LINUX_POST_BUILD_HOOKS += RAZE_BUILD_KERNEL_OVERLAYS
endif

# Lemnos's users table.
RAZE_LEMNOS_USERS_TABLE = $(call qstrip,$(BR2_RAZE_LEMNOS_USERS_TABLE))
ifneq ($(RAZE_LEMNOS_USERS_TABLE),)
PACKAGES_USERS += $(file <$(RAZE_LEMNOS_USERS_TABLE))$(sep)
endif

ifeq ($(BR2_RAZE_FAST_CFLAGS),y)
TARGET_CFLAGS += -O3
endif

ifeq ($(BR2_RAZE_EXTRA),y)
include $(BR2_EXTERNAL_RAZE_DEVICE_PATH)/extra.mk
endif
"#;

    fn index(packages: &[(&str, &str)]) -> SymbolIndex {
        let mut package_prefixes = packages
            .iter()
            .map(|(name, _)| (upper(name), name.to_string()))
            .collect::<Vec<_>>();
        package_prefixes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        SymbolIndex {
            infra: vec![(
                "package/Makefile.in".to_string(),
                "TARGET_CFLAGS = $(TARGET_ABI)\nifeq ($(BR2_SHARED_LIBS),y)\nendif\n".to_string(),
            )],
            packages: packages
                .iter()
                .map(|(name, contents)| (name.to_string(), contents.to_string()))
                .collect(),
            external: ExternalMakefiles::parse(EXTERNAL_MK),
            package_prefixes,
        }
    }

    fn sample() -> SymbolIndex {
        index(&[
            ("linux", "LINUX_VERSION = 7.2\n"),
            ("openjdk", "OPENJDK_VERSION = 21\n"),
            (
                "rpi-firmware",
                "ifeq ($(BR2_RAZE_FIRMWARE_TRIM),y)\nRPI_FIRMWARE_X = 1\nendif\n",
            ),
        ])
    }

    #[test]
    fn a_setting_feeding_only_the_users_table_rebuilds_nothing() {
        let found = sample().symbol_use("BR2_RAZE_LEMNOS_USERS_TABLE");
        assert_eq!(
            found,
            SymbolUse {
                global: None,
                packages: BTreeSet::new(),
                finalize: true,
            }
        );
    }

    #[test]
    fn settings_feeding_package_variables_or_hooks_rebuild_that_package() {
        let found = sample().symbol_use("BR2_LINUX_KERNEL_EXT_OV9782");
        assert_eq!(found.global, None);
        assert_eq!(found.packages, BTreeSet::from(["linux".to_string()]));
        let found = sample().symbol_use("BR2_RAZE_FIRMWARE_TRIM");
        assert_eq!(found.packages, BTreeSet::from(["rpi-firmware".to_string()]));
    }

    #[test]
    fn global_variables_includes_infrastructure_and_architecture_clean_everything() {
        let index = sample();
        assert!(index.symbol_use("BR2_RAZE_FAST_CFLAGS").global.is_some());
        assert!(index.symbol_use("BR2_RAZE_EXTRA").global.is_some());
        assert!(index.symbol_use("BR2_SHARED_LIBS").global.is_some());
        assert!(index.symbol_use("BR2_cortex_a76").global.is_some());
    }

    #[test]
    fn overlays_and_unread_settings_rebuild_nothing() {
        let index = sample();
        assert_eq!(
            index.symbol_use("BR2_ROOTFS_OVERLAY"),
            SymbolUse {
                finalize: true,
                ..SymbolUse::default()
            }
        );
        assert_eq!(index.symbol_use("BR2_RAZE_UNUSED"), SymbolUse::default());
    }
}
