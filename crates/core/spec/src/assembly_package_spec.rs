use crate::{AssemblyPathTemplate, AssemblyTreeId};

/// The format `[[image.assembly.packages]]` writes: an opkg `ipk`. `deb` is
/// reserved for the Debian writer that follows.
pub const PACKAGE_FORMAT_IPK: &str = "ipk";

/// A package file built from an assembly tree: its data is the tree as
/// installed, its control and maintainer scripts come from the spec.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyPackageSpec {
    pub id: String,
    /// `ipk`; checked by the validator.
    pub format: String,
    /// The tree whose files, as installed, make the package's data.
    pub tree: AssemblyTreeId,
    /// `dest` as written: a file, or a directory (ending in `/`) that the
    /// package file is named in.
    pub dest: AssemblyPathTemplate,
    /// The file the package is written to: `dest` itself, or `dest` with the
    /// `<Package>_<Version>_<Architecture>.ipk` name appended when it is a
    /// directory. Derived from `dest` and `control` when the spec is built.
    pub output: AssemblyPathTemplate,
    /// Whether the archive starts with a `debian-binary` member (`2.0`).
    /// False gives the layout Limelight's examples use.
    pub debian_binary: bool,
    /// Absolute paths; every non-directory entry of the data must be under
    /// one of them. Empty means no restriction.
    pub allowed_prefixes: Vec<String>,
    pub control: AssemblyPackageControlSpec,
    pub scripts: AssemblyPackageScriptsSpec,
    /// `[image.assembly.packages.device]`: checks the package's ELF files
    /// against the device's sysroot before it is written. `None` skips them.
    /// Not serialized when unset, so packages without it keep their fingerprints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<AssemblyPackageDeviceSpec>,
}

/// The device a package runs on, as checks of its ELF files against the
/// device's sysroot (the libraries and the C library of the device OS).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyPackageDeviceSpec {
    /// The `[[toolchains]]` id whose sysroot is the device's. Exactly one of
    /// this and `sysroot` is set (the validator reports otherwise).
    pub toolchain: Option<String>,
    /// A directory, as a workspace path, that is the device's sysroot.
    pub sysroot: Option<String>,
    /// The highest `GLIBC_x.y` a file may need, as written (`2.17`). `None`
    /// takes the highest version the sysroot's C library defines.
    pub max_glibc: Option<String>,
    /// The smallest `PT_LOAD` alignment a file may have, in bytes. `None`
    /// takes the device toolchain's `page_size`, or
    /// `DEFAULT_DEVICE_MIN_PAGE_SIZE` when there is none.
    pub min_page_size: Option<u64>,
    /// Whether every `DT_NEEDED` library (and the program interpreter) must be
    /// found in the sysroot or in the package.
    pub check_needed: bool,
}

/// The smallest `PT_LOAD` alignment checked when neither the device table nor
/// its toolchain names one: 4 KiB, the smallest page size a kernel uses.
pub const DEFAULT_DEVICE_MIN_PAGE_SIZE: u64 = 4096;

/// Parses a `GLIBC_x.y` version (or `x.y`) into its `(major, minor)` pair.
/// `None` for anything else, such as `GLIBC_PRIVATE` or `2`.
pub fn parse_glibc_version(text: &str) -> Option<(u32, u32)> {
    let text = text.strip_prefix("GLIBC_").unwrap_or(text);
    let (major, minor) = text.split_once('.')?;
    let major = major.parse().ok()?;
    let minor = minor.parse().ok()?;
    Some((major, minor))
}

impl AssemblyPackageSpec {
    /// The `output` a `dest` and `control` give: `dest` when it names a file,
    /// `dest` plus `<Package>_<Version>_<Architecture>.ipk` when it ends in `/`.
    pub fn output_for(dest: &str, control: &AssemblyPackageControlSpec) -> String {
        if dest.ends_with('/') {
            format!(
                "{dest}{}_{}_{}.ipk",
                control.package, control.version, control.architecture
            )
        } else {
            dest.to_string()
        }
    }
}

/// The control fields of the package (the `control` file of its
/// `control.tar.gz`), in the spelling opkg and dpkg use.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyPackageControlSpec {
    pub package: String,
    pub version: String,
    /// Required; the validator reports a missing one. Never defaulted.
    pub architecture: String,
    pub maintainer: String,
    /// The synopsis, then any further lines (a blank line is written as ` .`).
    pub description: String,
    pub depends: Vec<String>,
    pub section: Option<String>,
    pub priority: Option<String>,
    pub source: Option<String>,
    pub homepage: Option<String>,
    /// Further fields, sorted by name.
    pub extra: Vec<(String, String)>,
}

/// The maintainer scripts and the files the package lists as conffiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyPackageScriptsSpec {
    /// systemd units the generated scripts enable, start, stop, disable and
    /// reset. Each must be in the tree.
    pub services: Vec<String>,
    /// Scripts that replace the generated ones, as host paths.
    pub postinst: Option<AssemblyPathTemplate>,
    pub preinst: Option<AssemblyPathTemplate>,
    pub prerm: Option<AssemblyPathTemplate>,
    pub postrm: Option<AssemblyPathTemplate>,
    /// Absolute guest paths, each a file of the tree.
    pub conffiles: Vec<String>,
}
