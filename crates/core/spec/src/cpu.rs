//! Per-board CPU profiles: the CPU a board's binaries are compiled for, so
//! the instructions its cores have (dotprod, fp16, CRC32, crypto, RVV) are
//! used everywhere and not only where a crate detects them at run time.
//!
//! A build names a profile in `[cpu]`. Each Rust artifact built for the
//! profile's architecture gets its rustc and C compiler flags, and a
//! Buildroot image gets its CPU symbols.

use crate::TargetArch;

/// A CPU Gaia knows how to compile for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuProfile {
    /// The name `[cpu] profile` takes.
    pub name: &'static str,
    /// The boards it is for, for the docs and error messages.
    pub boards: &'static str,
    pub arch: TargetArch,
    /// rustc's `-C target-cpu`. A big.LITTLE profile names the little core:
    /// code runs on both, and rustc has no tuning for the pair.
    pub rust_target_cpu: Option<&'static str>,
    /// rustc's `-C target-feature`, added to what the CPU implies.
    pub rust_target_features: &'static [&'static str],
    /// The C and C++ compiler flags (`CFLAGS_<target>` and
    /// `CXXFLAGS_<target>`, which the `cc` crate reads).
    pub c_flags: &'static [&'static str],
    /// Buildroot symbols that select the CPU, set before the image's own
    /// `config_overrides`.
    pub buildroot: &'static [(&'static str, &'static str)],
}

pub const CPU_PROFILES: &[CpuProfile] = &[
    CpuProfile {
        name: "cortex-a76",
        boards: "CM5 (BCM2712)",
        arch: TargetArch::AArch64,
        rust_target_cpu: Some("cortex-a76"),
        rust_target_features: &[],
        c_flags: &["-mcpu=cortex-a76"],
        buildroot: &[("BR2_cortex_a76", "y")],
    },
    CpuProfile {
        name: "cortex-a76.cortex-a55",
        boards: "RK3588S, RK3582",
        arch: TargetArch::AArch64,
        rust_target_cpu: Some("cortex-a55"),
        rust_target_features: &[],
        c_flags: &["-mcpu=cortex-a76.cortex-a55"],
        buildroot: &[("BR2_cortex_a76_a55", "y")],
    },
    CpuProfile {
        name: "cortex-a72",
        boards: "CM4, Pi 4 (BCM2711)",
        arch: TargetArch::AArch64,
        rust_target_cpu: Some("cortex-a72"),
        // The BCM2711's A72s are built without the crypto extension: no
        // AES or SHA2 instructions, whatever the core model allows.
        rust_target_features: &["-aes", "-sha2"],
        c_flags: &["-mcpu=cortex-a72+nocrypto"],
        buildroot: &[("BR2_cortex_a72", "y")],
    },
    CpuProfile {
        name: "cortex-a72.cortex-a53",
        boards: "RK3576",
        arch: TargetArch::AArch64,
        rust_target_cpu: Some("cortex-a53"),
        rust_target_features: &[],
        c_flags: &["-mcpu=cortex-a72.cortex-a53"],
        buildroot: &[("BR2_cortex_a72_a53", "y")],
    },
    CpuProfile {
        name: "cortex-a55",
        boards: "RK3566",
        arch: TargetArch::AArch64,
        rust_target_cpu: Some("cortex-a55"),
        rust_target_features: &[],
        c_flags: &["-mcpu=cortex-a55"],
        buildroot: &[("BR2_cortex_a55", "y")],
    },
    CpuProfile {
        name: "cortex-a53",
        boards: "RK3562",
        arch: TargetArch::AArch64,
        rust_target_cpu: Some("cortex-a53"),
        rust_target_features: &[],
        c_flags: &["-mcpu=cortex-a53"],
        buildroot: &[("BR2_cortex_a53", "y")],
    },
    CpuProfile {
        name: "cortex-a7",
        boards: "Flame (RV1106)",
        arch: TargetArch::Arm,
        rust_target_cpu: Some("cortex-a7"),
        rust_target_features: &["+neon", "+vfp4"],
        c_flags: &["-mcpu=cortex-a7", "-mfpu=neon-vfpv4"],
        buildroot: &[("BR2_cortex_a7", "y"), ("BR2_ARM_FPU_NEON_VFPV4", "y")],
    },
    CpuProfile {
        name: "xuantie-c908",
        boards: "K230D (C908 big core)",
        arch: TargetArch::RiscV64,
        // rustc's LLVM has no C908 model: the base target plus its
        // extensions. RVV 1.0 with VLEN 128, and the bit-manipulation
        // extensions every C908 has.
        rust_target_cpu: None,
        rust_target_features: &["+v", "+zba", "+zbb", "+zbs"],
        c_flags: &["-march=rv64gcv_zba_zbb_zbs"],
        buildroot: &[
            ("BR2_riscv_custom", "y"),
            ("BR2_RISCV_ISA_RVM", "y"),
            ("BR2_RISCV_ISA_RVA", "y"),
            ("BR2_RISCV_ISA_RVF", "y"),
            ("BR2_RISCV_ISA_RVD", "y"),
            ("BR2_RISCV_ISA_RVC", "y"),
            ("BR2_RISCV_ISA_RVV", "y"),
        ],
    },
];

/// The profile named `name`.
pub fn cpu_profile(name: &str) -> Option<&'static CpuProfile> {
    CPU_PROFILES.iter().find(|profile| profile.name == name)
}

impl CpuProfile {
    /// The rustc flags, as `-C` pairs.
    pub fn rustflags(&self) -> Vec<String> {
        let mut flags = Vec::new();
        if let Some(cpu) = self.rust_target_cpu {
            flags.push("-C".to_string());
            flags.push(format!("target-cpu={cpu}"));
        }
        if !self.rust_target_features.is_empty() {
            flags.push("-C".to_string());
            flags.push(format!(
                "target-feature={}",
                self.rust_target_features.join(",")
            ));
        }
        flags
    }

    /// The flags a Rust artifact built for `target` gets: `None` when the
    /// target is for another architecture (a host tool, say).
    pub fn for_rust_target(&self, target: &str) -> Option<RustCpuSpec> {
        (crate::target_arch(target) == Some(self.arch)).then(|| RustCpuSpec {
            profile: self.name.to_string(),
            rustflags: self.rustflags(),
            c_flags: self.c_flags.iter().map(ToString::to_string).collect(),
        })
    }
}

/// The CPU flags of one Rust artifact, from the build's `[cpu]` profile.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RustCpuSpec {
    pub profile: String,
    /// Added to `CARGO_TARGET_<TRIPLE>_RUSTFLAGS`, which cargo merges with
    /// the project's own `target.<triple>.rustflags`.
    pub rustflags: Vec<String>,
    /// Added to `CFLAGS_<target>` and `CXXFLAGS_<target>`.
    pub c_flags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_apply_only_to_their_architecture() {
        let a76 = cpu_profile("cortex-a76").expect("profile");
        let flags = a76
            .for_rust_target("aarch64-unknown-linux-gnu")
            .expect("aarch64");
        assert_eq!(flags.rustflags, ["-C", "target-cpu=cortex-a76"]);
        assert_eq!(flags.c_flags, ["-mcpu=cortex-a76"]);
        assert!(a76.for_rust_target("x86_64-unknown-linux-gnu").is_none());
        assert!(
            a76.for_rust_target("armv7-unknown-linux-gnueabihf")
                .is_none()
        );

        let c908 = cpu_profile("xuantie-c908").expect("profile");
        let flags = c908
            .for_rust_target("riscv64gc-unknown-linux-gnu")
            .expect("riscv64");
        assert_eq!(flags.rustflags, ["-C", "target-feature=+v,+zba,+zbb,+zbs"]);

        let a7 = cpu_profile("cortex-a7").expect("profile");
        assert_eq!(
            a7.for_rust_target("armv7-unknown-linux-gnueabihf")
                .expect("armv7")
                .rustflags,
            [
                "-C",
                "target-cpu=cortex-a7",
                "-C",
                "target-feature=+neon,+vfp4"
            ]
        );
    }

    #[test]
    fn profile_names_are_unique() {
        for (index, profile) in CPU_PROFILES.iter().enumerate() {
            assert!(
                CPU_PROFILES[index + 1..]
                    .iter()
                    .all(|other| other.name != profile.name),
                "{}",
                profile.name
            );
        }
    }
}
