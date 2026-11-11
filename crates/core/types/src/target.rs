//! What something is built for.

use crate::kind::open_kind;
use daedalus::prelude::Type;
use serde::{Deserialize, Serialize};

open_kind!(
    /// A CPU architecture.
    Arch, "gaia:arch" {
        X86_64 => "x86_64",
        AArch64 => "aarch64",
        Arm => "arm",
        RiscV64 => "riscv64",
        RiscV32 => "riscv32",
    }
);

open_kind!(
    /// The system a target runs: Linux, or none for firmware on bare metal.
    Os, "gaia:os" {
        Linux => "linux",
        None => "none",
    }
);

/// What an artifact or image is built for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Type, Serialize, Deserialize)]
#[type_key("gaia:target")]
pub struct Target {
    pub arch: Arch,
    /// The CPU, when it is tuned for one (`cortex-a76`, `cortex-m33`).
    pub cpu: Option<String>,
    pub os: Os,
}

impl Target {
    /// The architecture and OS of a target triple such as
    /// `aarch64-unknown-linux-musl` or `thumbv8m.main-none-eabihf`. The
    /// vendor and C library do not change either.
    pub fn from_triple(triple: &str) -> Self {
        let lowered = triple.trim().to_ascii_lowercase();
        let mut parts = lowered.split('-');
        let arch_part = parts.next().unwrap_or_default();
        let arch = match arch_part {
            "x86_64" | "amd64" => Arch::X86_64,
            "aarch64" | "arm64" => Arch::AArch64,
            a if a.starts_with("riscv64") => Arch::RiscV64,
            a if a.starts_with("riscv32") => Arch::RiscV32,
            a if a == "arm" || a.starts_with("armv") || a.starts_with("thumbv") => Arch::Arm,
            other => Arch::Other(other.to_string()),
        };
        let os = if lowered.contains("-linux") {
            Os::Linux
        } else if lowered.split('-').any(|part| part == "none") {
            Os::None
        } else {
            Os::Other(lowered.split('-').nth(2).unwrap_or_default().to_string())
        };
        Self {
            arch,
            cpu: None,
            os,
        }
    }
}
