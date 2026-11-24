//! The crate's integration tests, one binary: each module was a file of
//! its own under tests/.

mod acache;
mod cli;
mod disk;
mod gate_lanes;
mod gate_steps;
mod gate_target;
mod input;
mod os;
mod qemu_board_sysfs;
mod qemu_riscv64;
mod qemu_runner;
mod qemu_virt_kernel;
mod recipes;
mod sizes;
mod test_bound;
mod validate;
