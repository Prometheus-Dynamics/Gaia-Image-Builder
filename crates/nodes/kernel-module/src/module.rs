//! The parts of the kernel-module node that need no tool or store: module
//! names, the release a kernel build is for, vermagic and placement.

/// The directory under `lib/modules/<release>/` that the new modules go in.
/// It is the one `depmod` searches after the kernel's own tree.
pub const UPDATES_DIR: &str = "updates";

/// A module name, the stem of the `.ko` Kbuild builds: letters, digits, `-`
/// and `_`, starting with a letter or digit, and no path in it.
pub fn valid_name(name: &str) -> Result<(), String> {
    let first_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if first_ok && rest_ok {
        Ok(())
    } else {
        Err(format!(
            "module name {name:?} must be letters, digits, '-' or '_', starting with a letter or digit"
        ))
    }
}

/// How a module is compiled: as its kernel was. `Llvm` builds with `LLVM=1`
/// from the toolchain's pinned LLVM, and with its rustc when the kernel has
/// Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compile {
    Gcc,
    Llvm { rust: bool },
}

/// The compile mode for a kernel build's `.config` and the board's declared
/// kernel toolchain (`gcc` or `llvm`). The two must agree: a kernel built
/// with clang (`CONFIG_CC_IS_CLANG=y`) for a board that declares gcc, or the
/// other way round, is refused, by both names.
pub fn compile_of(config: &str, declared: &str) -> Result<Compile, String> {
    let set = |name: &str| config.lines().any(|line| line == format!("{name}=y"));
    let clang = set("CONFIG_CC_IS_CLANG");
    let built = if clang { "llvm" } else { "gcc" };
    match declared {
        "gcc" | "llvm" if declared != built => Err(format!(
            "the board's kernel toolchain is {declared}, but the kernel build was made with {} \
             (its .config has {}): a module is built as its kernel was, and the two must agree",
            if clang { "clang" } else { "gcc" },
            if clang {
                "CONFIG_CC_IS_CLANG=y"
            } else {
                "no CONFIG_CC_IS_CLANG"
            }
        )),
        "gcc" => Ok(Compile::Gcc),
        "llvm" => Ok(Compile::Llvm {
            rust: set("CONFIG_RUST"),
        }),
        other => Err(format!(
            "kernel toolchain {other:?} is not one: it is gcc or llvm"
        )),
    }
}

/// The release a kernel build is for: the `UTS_RELEASE` in its
/// `include/generated/utsrelease.h`. That is the string the module's vermagic
/// is built from, so it is the one to compare.
pub fn release_of_header(header: &str) -> Result<String, String> {
    for line in header.lines() {
        let Some(rest) = line.trim().strip_prefix("#define UTS_RELEASE") else {
            continue;
        };
        let value = rest
            .trim()
            .strip_prefix('"')
            .and_then(|text| text.split('"').next());
        if let Some(release) = value.filter(|release| !release.is_empty()) {
            return Ok(release.to_string());
        }
    }
    Err("the kernel build's include/generated/utsrelease.h has no UTS_RELEASE".into())
}

/// The release a `modinfo -F vermagic` line names: its first word.
pub fn vermagic_release(vermagic: &str) -> Option<&str> {
    vermagic.split_whitespace().next()
}

/// The one release directory in a `lib/modules` tree, given its entry names.
pub fn single_release(names: &[String]) -> Result<String, String> {
    match names {
        [only] => Ok(only.clone()),
        [] => Err("the kernel modules have no release directory".into()),
        many => Err(format!(
            "the kernel modules have {} release directories ({}), expected one",
            many.len(),
            many.join(", ")
        )),
    }
}

/// Where a built module is placed, relative to `lib/modules`.
pub fn placed_path(release: &str, name: &str) -> String {
    format!("{release}/{UPDATES_DIR}/{name}.ko")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names_are_plain_stems() {
        assert!(valid_name("hello").is_ok());
        assert!(valid_name("wifi_8821-x").is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("-x").is_err());
        assert!(valid_name("a/b").is_err());
        assert!(valid_name("a.ko").is_err());
    }

    #[test]
    fn the_release_is_read_from_the_header() {
        let header = "/* comment */\n#define UTS_RELEASE \"7.2.9-200.nobara.fc44.x86_64\"\n";
        assert_eq!(
            release_of_header(header).expect("release"),
            "7.2.9-200.nobara.fc44.x86_64"
        );
        assert!(release_of_header("#define LINUX_VERSION_CODE 1\n").is_err());
        assert!(release_of_header("#define UTS_RELEASE \"\"\n").is_err());
    }

    #[test]
    fn vermagic_names_the_release_first() {
        assert_eq!(
            vermagic_release("7.2.9-200.nobara.fc44.x86_64 SMP preempt mod_unload "),
            Some("7.2.9-200.nobara.fc44.x86_64")
        );
        assert_eq!(vermagic_release("  \n"), None);
    }

    #[test]
    fn one_release_directory_is_required() {
        assert_eq!(single_release(&["6.1.0".into()]).expect("one"), "6.1.0");
        assert!(single_release(&[]).is_err());
        assert!(single_release(&["a".into(), "b".into()]).is_err());
    }

    #[test]
    fn modules_go_under_updates() {
        assert_eq!(placed_path("6.1.0", "hello"), "6.1.0/updates/hello.ko");
    }

    #[test]
    fn a_module_is_compiled_as_its_kernel_was() {
        let gcc = "CONFIG_CC_IS_GCC=y\nCONFIG_MODULES=y\n";
        let clang = "CONFIG_CC_IS_CLANG=y\nCONFIG_MODULES=y\n";
        let rust = "CONFIG_CC_IS_CLANG=y\nCONFIG_RUST=y\n";
        assert_eq!(compile_of(gcc, "gcc"), Ok(Compile::Gcc));
        assert_eq!(compile_of(clang, "llvm"), Ok(Compile::Llvm { rust: false }));
        assert_eq!(compile_of(rust, "llvm"), Ok(Compile::Llvm { rust: true }));
        // A line that only starts like the option is not it.
        assert_eq!(
            compile_of("CONFIG_CC_IS_CLANG=yes\n", "gcc"),
            Ok(Compile::Gcc)
        );
    }

    #[test]
    fn a_kernel_built_otherwise_than_the_board_declares_is_refused() {
        let why =
            compile_of("CONFIG_CC_IS_GCC=y\nCONFIG_RUST=y\n", "llvm").expect_err("gcc kernel");
        assert!(
            why.contains(
                "the board's kernel toolchain is llvm, but the kernel build was made with gcc"
            ),
            "{why}"
        );
        let why = compile_of("CONFIG_CC_IS_CLANG=y\n", "gcc").expect_err("clang kernel");
        assert!(why.contains("the board's kernel toolchain is gcc, but the kernel build was made with clang (its .config has CONFIG_CC_IS_CLANG=y)"), "{why}");
        let why = compile_of("CONFIG_CC_IS_CLANG=y\n", "clang").expect_err("not a toolchain");
        assert!(
            why.contains("kernel toolchain \"clang\" is not one"),
            "{why}"
        );
    }
}
