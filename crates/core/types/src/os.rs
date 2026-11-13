//! An OS build's configuration, as the graph gives it to `gaia.os.buildroot`:
//! config fragments, the trees they name, and the packages whose source is
//! one of those trees.

use daedalus::prelude::Type;
use serde::{Deserialize, Serialize};

/// A block of Buildroot config: `BR2_...=value` and `# BR2_... is not set`
/// lines. A value may name one of the build's input trees as `@name@`; the
/// step replaces it with the tree's path, so the text, and the key, do not
/// change with where the store is.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:buildroot-fragment")]
pub struct ConfigFragment {
    /// Names the fragment in errors and in the key. Unique in one build.
    pub name: String,
    pub text: String,
}

/// The host side of a Buildroot run: make, an optional CPU pin, the job
/// count and the download cache.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:buildroot-host")]
pub struct BuildrootHost {
    /// Absolute path of the make program.
    pub make: String,
    /// Absolute path of `taskset`, used when `cpus` is set.
    pub taskset: String,
    /// A CPU list such as `6-31`; empty for no pinning.
    pub cpus: String,
    /// Make jobs, at least 1.
    pub jobs: u32,
    /// Absolute path of the download cache (`BR2_DL_DIR`).
    pub download_dir: String,
}

/// A package Buildroot builds from one of the input trees instead of
/// downloading it (`<PACKAGE>_OVERRIDE_SRCDIR`).
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:source-override")]
pub struct SourceOverride {
    /// Buildroot's package name, in make's form (`LINUX`, `UBOOT`).
    pub package: String,
    /// The input tree, by name.
    pub input: String,
}

/// A contract the kernel's final configuration must meet: a file in one of
/// the input trees that lists the options a boot path needs built in, read
/// for one kind of image, with the board's own storage drivers beside it.
/// The step checks it once the kernel is configured, before it compiles.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:kernel-contract")]
pub struct KernelContract {
    /// The input tree that holds the file, by name.
    pub input: String,
    /// The file in that tree.
    pub path: String,
    /// Who asks, for the failure: the component that states the contract.
    pub asked_by: String,
    /// The board the kernel is for, for the failure.
    pub board: String,
    /// The image kind the contract is read for (`disk` or `mtd`).
    pub kind: String,
    /// The board's storage drivers, as `.config` writes them
    /// (`CONFIG_...`): each must be built in.
    pub storage_drivers: Vec<String>,
}

/// What an OS build is made from besides its provider's source: fragments,
/// the names of its input trees in port order, source overrides and
/// external trees.
#[derive(Clone, Debug, Default, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:os-config")]
pub struct OsConfig {
    /// Applied in order; a later line for a symbol replaces an earlier one.
    pub fragments: Vec<ConfigFragment>,
    /// The input trees' names: the first port's tree is `inputs[0]`.
    pub inputs: Vec<String>,
    pub overrides: Vec<SourceOverride>,
    /// Input trees given to Buildroot as `BR2_EXTERNAL`, by name.
    pub externals: Vec<String>,
    /// Contracts the kernel's final configuration must meet.
    #[serde(default)]
    pub kernel_contracts: Vec<KernelContract>,
}

impl OsConfig {
    /// An error for an input named twice, or an override, an external or a
    /// kernel contract that names no input.
    pub fn check(&self) -> Result<(), String> {
        for (i, name) in self.inputs.iter().enumerate() {
            if self.inputs[..i].contains(name) {
                return Err(format!("the input {name:?} is named twice"));
            }
            if name.is_empty() || name.contains('@') {
                return Err(format!(
                    "the input name {name:?} is empty or has an @ in it"
                ));
            }
        }
        let named = |name: &str, what: &str| {
            if self.inputs.iter().any(|input| input == name) {
                Ok(())
            } else {
                Err(format!("{what} names no input {name:?}"))
            }
        };
        for o in &self.overrides {
            named(&o.input, &format!("the override of {}", o.package))?;
            let valid = !o.package.is_empty()
                && o.package
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
            if !valid {
                return Err(format!(
                    "{:?} is not a package in make's form (LINUX, UBOOT)",
                    o.package
                ));
            }
        }
        for e in &self.externals {
            named(e, "an external tree")?;
        }
        for contract in &self.kernel_contracts {
            named(
                &contract.input,
                &format!("the kernel contract of {}", contract.asked_by),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OsConfig {
        OsConfig {
            fragments: Vec::new(),
            inputs: vec!["linux".into(), "files".into()],
            overrides: vec![SourceOverride {
                package: "LINUX".into(),
                input: "linux".into(),
            }],
            externals: vec!["files".into()],
            kernel_contracts: Vec::new(),
        }
    }

    #[test]
    fn a_config_whose_names_all_resolve_is_fine() {
        assert_eq!(config().check(), Ok(()));
    }

    #[test]
    fn an_override_of_no_input_is_an_error() {
        let mut c = config();
        c.overrides[0].input = "u-boot".into();
        assert_eq!(
            c.check(),
            Err("the override of LINUX names no input \"u-boot\"".into())
        );
    }

    #[test]
    fn an_input_named_twice_is_an_error() {
        let mut c = config();
        c.inputs.push("linux".into());
        assert_eq!(c.check(), Err("the input \"linux\" is named twice".into()));
    }

    #[test]
    fn a_package_not_in_make_form_is_an_error() {
        let mut c = config();
        c.overrides[0].package = "linux".into();
        assert!(c.check().expect_err("lowercase").contains("make's form"));
    }

    #[test]
    fn an_external_of_no_input_is_an_error() {
        let mut c = config();
        c.externals = vec!["nothing".into()];
        assert!(
            c.check()
                .expect_err("external")
                .contains("an external tree")
        );
    }
}
