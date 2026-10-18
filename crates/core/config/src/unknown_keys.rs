//! Keys a build file sets that this Gaia does not know.
//!
//! The raw config structs accept and silently drop unknown keys (so files
//! written for a newer Gaia still load). Validation reports them instead.
//! Known keys are taken from the serde field lists of the raw structs
//! themselves, so the check never drifts from the parser. The tables whose
//! struct is an internally tagged enum (`[[sources]]` by `kind`,
//! `[[artifacts]]` by `kind`, and the image's `kind`-specific keys) are
//! listed by kind here, next to the struct they mirror; a test checks that
//! every kind's keys still parse. Free-form maps (inputs, presets, env) are
//! not checked.
use std::path::Path;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};

use crate::locate::KeyLocations;
use crate::overrides::did_you_mean_among;
use crate::raw::{self, RawBuildConfig};
use crate::raw_assembly;

type FieldsFn = fn() -> Option<&'static [&'static str]>;

/// The keys of a table whose own keys depend on its `kind`: the keys every
/// kind takes and the keys of the named kind. `None` when the kind is not one
/// this Gaia knows (or, for a table without a kind, when it has no kind
/// rule), so no key is reported.
type KindFieldsFn = fn(Option<&str>) -> Option<Vec<&'static str>>;

/// How the keys of a table are known.
#[derive(Clone, Copy)]
pub(crate) enum Known {
    /// A plain struct with these field names.
    Fixed(FieldsFn),
    /// A table whose keys depend on its `kind`.
    ByKind(KindFieldsFn),
}

/// A table (or array of tables) and the struct it deserializes into.
pub(crate) struct Section {
    key: &'static str,
    known: Known,
    children: &'static [Section],
}

const fn section(key: &'static str, fields: FieldsFn, children: &'static [Section]) -> Section {
    Section {
        key,
        known: Known::Fixed(fields),
        children,
    }
}

const fn by_kind(key: &'static str, kinds: KindFieldsFn, children: &'static [Section]) -> Section {
    Section {
        key,
        known: Known::ByKind(kinds),
        children,
    }
}

/// The keys one table may set, given the table itself (for its `kind`).
fn fields_of(known: Known, table: &toml::Table) -> Option<Vec<&'static str>> {
    match known {
        Known::Fixed(fields) => fields().map(<[&str]>::to_vec),
        Known::ByKind(kinds) => kinds(table.get("kind").and_then(toml::Value::as_str)),
    }
}

const COMMAND_PROVIDER_CHILDREN: &[Section] = &[
    section(
        "ccache",
        struct_fields::<raw::RawBuildrootCcachePolicyConfig>,
        &[],
    ),
    section(
        "package_cache",
        struct_fields::<raw::RawBuildrootPackageCacheConfig>,
        &[],
    ),
    // Per-tool keys are flattened into a map: the tools Gaia knows.
    section("host_tools", host_tools_fields, &[]),
];

fn host_tools_fields() -> Option<&'static [&'static str]> {
    const FIELDS: &[&str] = &["default", "ccache", "pkgconf"];
    debug_assert!(
        gaia_spec::KNOWN_HOST_TOOLS
            .iter()
            .all(|tool| FIELDS.contains(tool))
    );
    Some(FIELDS)
}

const ASSEMBLY_CHILDREN: &[Section] = &[
    section(
        "trees",
        struct_fields::<raw_assembly::RawAssemblyTreeConfig>,
        &[],
    ),
    section(
        "dirs",
        struct_fields::<raw_assembly::RawAssemblyDirConfig>,
        &[],
    ),
    section(
        "symlinks",
        struct_fields::<raw_assembly::RawAssemblySymlinkConfig>,
        &[],
    ),
    section(
        "files",
        struct_fields::<raw_assembly::RawAssemblyFileConfig>,
        &[],
    ),
    section(
        "transforms",
        struct_fields::<raw_assembly::RawAssemblyTransformConfig>,
        &[],
    ),
    section(
        "filesystems",
        struct_fields::<raw_assembly::RawAssemblyFilesystemConfig>,
        &[],
    ),
    section(
        "disks",
        struct_fields::<raw_assembly::RawAssemblyDiskConfig>,
        &[section(
            "partitions",
            struct_fields::<raw_assembly::RawAssemblyDiskPartitionConfig>,
            &[],
        )],
    ),
    section(
        "archives",
        struct_fields::<raw_assembly::RawAssemblyArchiveConfig>,
        &[
            section(
                "members",
                struct_fields::<raw_assembly::RawAssemblyArchiveMemberConfig>,
                &[],
            ),
            section(
                "generated",
                struct_fields::<raw_assembly::RawAssemblyArchiveGeneratedConfig>,
                &[],
            ),
        ],
    ),
    section(
        "busybox_initramfs",
        struct_fields::<raw_assembly::RawAssemblyBusyboxInitramfsConfig>,
        &[],
    ),
    section(
        "binaries",
        struct_fields::<raw_assembly::RawAssemblyBinaryConfig>,
        &[],
    ),
    section(
        "kernel_modules",
        struct_fields::<raw_assembly::RawAssemblyKernelModulesConfig>,
        &[],
    ),
    section(
        "devices",
        struct_fields::<raw_assembly::RawAssemblyDeviceConfig>,
        &[],
    ),
];

/// `[[sources]]`: the keys every source takes, then the keys of its `kind`.
const SOURCE_COMMON_KEYS: &[&str] = &["id", "kind"];
const GIT_SOURCE_KEYS: &[&str] = &[
    "repo",
    "branch",
    "tag",
    "rev",
    "subdir",
    "update",
    "refresh",
    "pin",
    "git_backend",
];
const PATH_SOURCE_KEYS: &[&str] = &["path", "identity_ignore", "refresh", "pin"];
const ARCHIVE_SOURCE_KEYS: &[&str] = &["path", "strip_components", "refresh", "pin"];
const DOWNLOAD_SOURCE_KEYS: &[&str] = &["url", "sha256", "output_path", "refresh", "pin"];

fn source_fields(kind: Option<&str>) -> Option<Vec<&'static str>> {
    let own = match kind? {
        "git" => GIT_SOURCE_KEYS,
        "path" => PATH_SOURCE_KEYS,
        "archive" => ARCHIVE_SOURCE_KEYS,
        "download" => DOWNLOAD_SOURCE_KEYS,
        _ => return None,
    };
    Some(SOURCE_COMMON_KEYS.iter().chain(own).copied().collect())
}

/// `[[artifacts]]`: the keys every artifact takes, then the keys of its kind.
const ARTIFACT_COMMON_KEYS: &[&str] = &[
    "id",
    "kind",
    "when",
    "source",
    "sources",
    "execution",
    "target",
    "profile",
    "dependencies",
    "after_image_prepare",
    "build_group",
    "cargo_git_sources",
    "install_name",
    "install_class",
    "install_dest_hint",
    "output_path",
];
const RUST_ARTIFACT_KEYS: &[&str] = &[
    "package",
    "target_name",
    "emit_directory",
    "features",
    "no_default_features",
    "all_features",
];
const JAVA_ARTIFACT_KEYS: &[&str] = &["build_target", "build_args", "build_command", "build_env"];
const PACKAGE_DIR_ARTIFACT_KEYS: &[&str] = &["package_dir"];
const GO_ARTIFACT_KEYS: &[&str] = &["package"];

fn artifact_fields(kind: Option<&str>) -> Option<Vec<&'static str>> {
    let own = match kind? {
        "rust" => RUST_ARTIFACT_KEYS,
        "java" => JAVA_ARTIFACT_KEYS,
        "node" | "python" => PACKAGE_DIR_ARTIFACT_KEYS,
        "go" => GO_ARTIFACT_KEYS,
        _ => return None,
    };
    Some(ARTIFACT_COMMON_KEYS.iter().chain(own).copied().collect())
}

/// `[image]`: the keys every image takes, then the keys of its kind. Without
/// a `kind` (a layer that leaves it to the layer it extends) every kind's
/// keys are accepted.
const IMAGE_COMMON_KEYS: &[&str] = &["kind", "feed", "output", "assembly"];
const BUILDROOT_IMAGE_KEYS: &[&str] = &[
    "source",
    "defconfig",
    "defconfig_path",
    "allow_fallback",
    "config_fragments",
    "config_overrides",
    "external_tree",
    "external_tree_mode",
    "expected_images",
    "rootfs",
];
const STARTING_POINT_IMAGE_KEYS: &[&str] = &[
    "source",
    "source_path",
    "rootfs_path",
    "image_partition",
    "image_read_only",
    "packages",
    "rootfs_validation_mode",
    "output_mode",
];

fn image_fields(kind: Option<&str>) -> Option<Vec<&'static str>> {
    let own: Vec<&'static str> = match kind {
        Some("buildroot") => BUILDROOT_IMAGE_KEYS.to_vec(),
        Some("starting-point") => STARTING_POINT_IMAGE_KEYS.to_vec(),
        Some(_) => return None,
        None => BUILDROOT_IMAGE_KEYS
            .iter()
            .chain(STARTING_POINT_IMAGE_KEYS)
            .copied()
            .collect(),
    };
    Some(IMAGE_COMMON_KEYS.iter().chain(&own).copied().collect())
}

const ARTIFACT_CHILDREN: &[Section] = &[section(
    "execution",
    struct_fields::<raw::RawArtifactExecutionConfig>,
    &[section(
        "docker",
        struct_fields::<raw::RawArtifactDockerExecutionConfig>,
        &[],
    )],
)];

const IMAGE_CHILDREN: &[Section] = &[
    section("output", struct_fields::<raw::RawImageOutputConfig>, &[]),
    section("feed", struct_fields::<raw::RawImageFeedConfig>, &[]),
    section(
        "assembly",
        struct_fields::<raw_assembly::RawImageAssemblyConfig>,
        ASSEMBLY_CHILDREN,
    ),
    section(
        "rootfs",
        struct_fields::<raw::RawBuildrootRootfsConfig>,
        &[],
    ),
    section(
        "expected_images",
        struct_fields::<raw::RawBuildrootExpectedImageConfig>,
        &[],
    ),
    section(
        "packages",
        struct_fields::<raw::RawStartingPointPackagesConfig>,
        &[],
    ),
];

const SECTIONS: &[Section] = &[
    by_kind("image", image_fields, IMAGE_CHILDREN),
    by_kind("sources", source_fields, &[]),
    by_kind("artifacts", artifact_fields, ARTIFACT_CHILDREN),
    section("workspace", struct_fields::<raw::RawWorkspaceConfig>, &[]),
    section("expect", struct_fields::<raw::RawExpectConfig>, &[]),
    section("product", struct_fields::<raw::RawProductConfig>, &[]),
    section(
        "interpolation",
        struct_fields::<raw::RawInterpolationConfig>,
        &[],
    ),
    section("clean", struct_fields::<raw::RawCleanConfig>, &[]),
    section(
        "execution",
        struct_fields::<raw::RawExecutionPolicyConfig>,
        &[
            section(
                "docker",
                struct_fields::<raw::RawDockerExecutionConfig>,
                &[],
            ),
            section(
                "output_retention",
                struct_fields::<raw::RawOutputRetentionPolicyConfig>,
                &[],
            ),
        ],
    ),
    section("failure", struct_fields::<raw::RawFailurePolicyConfig>, &[]),
    section(
        "providers",
        struct_fields::<raw::RawProviderPoliciesConfig>,
        &[
            section(
                "rust",
                struct_fields::<raw::RawRustProviderPolicyConfig>,
                &[],
            ),
            section("git", struct_fields::<raw::RawGitProviderPolicyConfig>, &[]),
            section(
                "archive",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "download",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "go",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "java",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "node",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "python",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "buildroot",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
            section(
                "starting_point",
                struct_fields::<raw::RawCommandProviderPolicyConfig>,
                COMMAND_PROVIDER_CHILDREN,
            ),
        ],
    ),
    section(
        "provenance",
        struct_fields::<raw::RawProvenanceConfig>,
        &[section(
            "identity",
            struct_fields::<raw::RawProvenanceIdentityConfig>,
            &[],
        )],
    ),
    section(
        "reporting",
        struct_fields::<raw::RawReportingConfig>,
        &[
            section(
                "masking",
                struct_fields::<raw::RawReportingMaskingConfig>,
                &[],
            ),
            section(
                "output_hygiene",
                struct_fields::<raw::RawOutputHygieneConfig>,
                &[],
            ),
            section(
                "post_build",
                struct_fields::<raw::RawPostBuildHookConfig>,
                &[],
            ),
        ],
    ),
    section(
        "stage",
        struct_fields::<raw::RawStageConfig>,
        &[
            section("files", struct_fields::<raw::RawStageFileConfig>, &[]),
            section("env_sets", struct_fields::<raw::RawStageEnvSetConfig>, &[]),
            section("services", struct_fields::<raw::RawStageServiceConfig>, &[]),
        ],
    ),
    section("install", struct_fields::<raw::RawInstallConfig>, &[]),
    section(
        "checkpoints",
        struct_fields::<raw::RawCheckpointConfig>,
        &[],
    ),
];

/// A key a build file sets that this Gaia does not know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UnknownKey {
    /// Dotted path in the file, such as `providers.java.gradle_hme` or
    /// `stage.files[0].owner`.
    pub(crate) path: String,
    /// `" (did you mean 'x'?)"` for the closest key the table takes, or
    /// nothing.
    pub(crate) suggestion: String,
    /// `line:col` of the key in its file, when the file's text shows it.
    pub(crate) location: Option<String>,
}

/// The unknown keys of one parsed build file, sorted by path.
pub(crate) fn unknown_config_keys(value: &toml::Value) -> Vec<UnknownKey> {
    let mut unknown = Vec::new();
    let root = |_: &toml::Table| struct_fields::<RawBuildConfig>().map(<[&str]>::to_vec);
    check_table(value, "", &root, SECTIONS, &mut unknown);
    unknown.sort_by(|left, right| left.path.cmp(&right.path));
    unknown
}

/// Attaches each unknown key's `line:col` from the file's own text, where the
/// text shows the key.
pub(crate) fn locate_unknown_keys(keys: &mut [UnknownKey], path: &Path, text: &str) {
    if keys.is_empty() {
        return;
    }
    let locations = KeyLocations::scan(text);
    for key in keys {
        key.location = locations
            .offset(&key.path)
            .map(|offset| crate::locate::at_offset(path, text, offset));
    }
}

/// Warnings for loaded files (the entrypoint, its `extends` chain and its
/// imports) that set nothing at all: a layer that should provide something
/// but holds only comments.
pub(crate) fn collect_empty_layer_warnings(raw: &RawBuildConfig) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut visit = vec![raw];
    while let Some(config) = visit.pop() {
        if config.sets_nothing {
            let path = config
                .source_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<build config>".into());
            let warning = format!("config file '{path}' sets nothing (only comments)");
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        visit.extend(config.extends_config.as_deref());
        visit.extend(
            config
                .imported_configs
                .iter()
                .map(|imported| &imported.config),
        );
    }
    warnings
}

/// Warning messages for every unknown key in the loaded file, its
/// `extends` chain and its imports.
pub(crate) fn collect_unknown_key_warnings(raw: &RawBuildConfig) -> Vec<String> {
    let mut warnings = Vec::new();
    collect_into(raw, &mut warnings);
    warnings
}

fn collect_into(raw: &RawBuildConfig, warnings: &mut Vec<String>) {
    let path = raw
        .source_path
        .as_deref()
        .map(Path::display)
        .map(|path| path.to_string())
        .unwrap_or_else(|| "<build config>".into());
    for key in &raw.unknown_keys {
        let place = key.location.clone().unwrap_or_else(|| path.clone());
        let warning = format!(
            "unknown key '{}' in '{place}'{}: this gaia does not know it \
             (a typo, or a setting that needs a newer gaia)",
            key.path, key.suggestion
        );
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    if let Some(extends) = &raw.extends_config {
        collect_into(extends, warnings);
    }
    for imported in &raw.imported_configs {
        collect_into(&imported.config, warnings);
    }
}

/// Checks the keys of `value` (a table, or an array of tables) against the
/// keys `known` gives for each table, then checks the nested sections.
fn check_table(
    value: &toml::Value,
    prefix: &str,
    known: &dyn Fn(&toml::Table) -> Option<Vec<&'static str>>,
    children: &'static [Section],
    unknown: &mut Vec<UnknownKey>,
) {
    let tables: Vec<(String, &toml::Table)> = match value {
        toml::Value::Table(table) => vec![(prefix.to_string(), table)],
        toml::Value::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                item.as_table()
                    .map(|table| (format!("{prefix}[{index}]"), table))
            })
            .collect(),
        _ => return,
    };
    for (path, table) in tables {
        let fields = known(table);
        for (key, child) in table {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            // Without a field list (flattened or custom types) only the
            // listed nested tables are checked.
            if let Some(fields) = &fields
                && !fields.contains(&key.as_str())
            {
                unknown.push(UnknownKey {
                    path: child_path,
                    suggestion: did_you_mean_among(key, fields),
                    location: None,
                });
                continue;
            }
            if let Some(section) = children.iter().find(|section| section.key == key) {
                let section_known = section.known;
                check_table(
                    child,
                    &child_path,
                    &move |table| fields_of(section_known, table),
                    section.children,
                    unknown,
                );
            }
        }
    }
}

/// The field names serde's derive passes to `deserialize_struct`, or `None`
/// for types that do not deserialize as a plain struct (flatten, custom
/// impls).
fn struct_fields<T: DeserializeOwned>() -> Option<&'static [&'static str]> {
    match T::deserialize(FieldNames) {
        Err(Captured(fields)) => fields,
        Ok(_) => None,
    }
}

#[derive(Debug)]
struct Captured(Option<&'static [&'static str]>);

impl std::fmt::Display for Captured {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("field name capture")
    }
}

impl std::error::Error for Captured {}

impl de::Error for Captured {
    fn custom<T: std::fmt::Display>(_message: T) -> Self {
        Self(None)
    }
}

struct FieldNames;

impl<'de> Deserializer<'de> for FieldNames {
    type Error = Captured;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Captured> {
        Err(Captured(None))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Captured> {
        Err(Captured(Some(fields)))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// The layout the JSON schema is written from (see `schema`).
pub(crate) mod layout {
    use super::{Known, RawBuildConfig, SECTIONS, Section, struct_fields};

    /// The keys of the build file's top level.
    pub(crate) fn root_fields() -> Vec<&'static str> {
        struct_fields::<RawBuildConfig>()
            .map(<[&str]>::to_vec)
            .unwrap_or_default()
    }

    /// A top-level section: its key, how its keys are known, and its nested
    /// sections.
    pub(crate) struct Node {
        pub(crate) key: &'static str,
        pub(crate) known: Known,
        pub(crate) children: &'static [Section],
    }

    /// The sections the unknown-key check looks into.
    pub(crate) fn top_sections() -> Vec<Node> {
        SECTIONS
            .iter()
            .map(|section| Node {
                key: section.key,
                known: section.known,
                children: section.children,
            })
            .collect()
    }

    /// A nested section, for the schema: its key, how its keys are known and
    /// its own nested sections.
    pub(crate) fn nested(section: &Section) -> (&'static str, Known, &'static [Section]) {
        (section.key, section.known, section.children)
    }

    /// The keys a table of a section takes when its `kind` is `kind` (`None`
    /// for a table that names none).
    pub(crate) fn keys_for_kind(known: Known, kind: Option<&str>) -> Option<Vec<&'static str>> {
        match known {
            Known::Fixed(fields) => fields().map(<[&str]>::to_vec),
            Known::ByKind(kinds) => kinds(kind),
        }
    }

    /// The kinds a kinded section names.
    pub(crate) fn kinds_of(key: &str) -> &'static [&'static str] {
        match key {
            "sources" => &["git", "path", "archive", "download"],
            "artifacts" => &["rust", "java", "node", "python", "go"],
            "image" => &["buildroot", "starting-point"],
            _ => &[],
        }
    }
}
#[cfg(test)]
#[path = "unknown_keys_tests.rs"]
mod tests;
