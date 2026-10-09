//! The CPU architecture a declared artifact target builds for, so installed
//! binaries can be checked against it (by ELF machine) and validation can
//! refuse targets Gaia cannot check before any build runs.

/// Architectures Gaia can verify installed binaries against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetArch {
    X86_64,
    Arm,
    AArch64,
    RiscV64,
}

impl TargetArch {
    pub fn label(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Arm => "arm",
            Self::AArch64 => "aarch64",
            Self::RiscV64 => "riscv64",
        }
    }
}

/// The architecture of a Rust/LLVM target triple (`aarch64-unknown-linux-musl`,
/// `armv7-unknown-linux-gnueabihf`, `x86_64-unknown-linux-gnu`, ...) or a
/// Docker platform (`linux/arm64`, `linux/arm/v7`), by its architecture part:
/// the vendor, OS and C library (gnu, musl, ...) do not change the ELF
/// machine. `None` for architectures Gaia cannot verify.
pub fn target_arch(target: &str) -> Option<TargetArch> {
    let lowered = target.trim().to_ascii_lowercase();
    let arch = match lowered.strip_prefix("linux/") {
        Some(platform) => platform.split('/').next().unwrap_or_default(),
        None => lowered.split('-').next().unwrap_or_default(),
    };
    match arch {
        "aarch64" | "arm64" => Some(TargetArch::AArch64),
        "x86_64" | "amd64" => Some(TargetArch::X86_64),
        _ if arch.starts_with("riscv64") => Some(TargetArch::RiscV64),
        _ if arch == "arm"
            || arch.starts_with("armv")
            || arch.starts_with("thumbv")
            || arch.starts_with("armeb") =>
        {
            Some(TargetArch::Arm)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_map_by_architecture_whatever_their_libc() {
        for (target, arch) in [
            ("aarch64-unknown-linux-gnu", TargetArch::AArch64),
            ("aarch64-unknown-linux-musl", TargetArch::AArch64),
            ("linux/arm64", TargetArch::AArch64),
            ("x86_64-unknown-linux-musl", TargetArch::X86_64),
            ("linux/amd64", TargetArch::X86_64),
            ("riscv64gc-unknown-linux-gnu", TargetArch::RiscV64),
            ("riscv64gc-unknown-linux-musl", TargetArch::RiscV64),
            ("armv7-unknown-linux-gnueabihf", TargetArch::Arm),
            ("armv7-unknown-linux-musleabihf", TargetArch::Arm),
            ("arm-unknown-linux-gnueabi", TargetArch::Arm),
            ("thumbv7neon-unknown-linux-gnueabihf", TargetArch::Arm),
            ("linux/arm/v7", TargetArch::Arm),
            (" AARCH64-UNKNOWN-LINUX-MUSL ", TargetArch::AArch64),
        ] {
            assert_eq!(target_arch(target), Some(arch), "{target}");
        }
        for target in [
            "mips-unknown-linux-gnu",
            "powerpc64le-unknown-linux-gnu",
            "wasm32-wasi",
            "",
        ] {
            assert_eq!(target_arch(target), None, "{target}");
        }
    }
}
