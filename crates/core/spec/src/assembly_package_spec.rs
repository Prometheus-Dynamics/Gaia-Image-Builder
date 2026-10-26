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
