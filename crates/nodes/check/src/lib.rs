//! Checks on a built image: the image's tree is read, and a check that finds a
//! problem fails the build, naming every problem it found.
//!
//! - **`gaia.check.modules`**: the kernel modules and firmware files a board
//!   needs are in the image.
//! - **`gaia.check.kernel-config`**: the kernel configuration options a board
//!   needs are set as it needs them.
//! - **`gaia.check.device-tree`**: the device tree nodes a board names exist in
//!   its blobs.
//! - **`gaia.check.units`**: no service unit in the image has a start limit.
//! - **`gaia.check.binaries`**: every dynamically linked file's interpreter and
//!   libraries are in the image.
//!
//! Each node gives back the tree it was given, so a graph orders a check
//! before whatever uses the image. A check writes nothing and runs nothing;
//! its only inputs are the tree and, for the first three, a JSON requirements
//! text. Requirements are data from the board and product descriptions, not
//! names in this code.

mod binaries;
mod common;
mod device_tree;
mod elf;
mod fdt;
mod kernel_config;
mod modules;
mod tree;
mod units;

pub use binaries::check_binaries;
pub use device_tree::check_device_tree;
pub use kernel_config::check_kernel_config;
pub use modules::check_modules;
pub use units::check_units;

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 5] = [
    &modules::CHECK_MODULES_NODE,
    &kernel_config::CHECK_KERNEL_CONFIG_NODE,
    &device_tree::CHECK_DEVICE_TREE_NODE,
    &units::CHECK_UNITS_NODE,
    &binaries::CHECK_BINARIES_NODE,
];
