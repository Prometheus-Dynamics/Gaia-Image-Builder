# Platform Support

Gaia's release target is Linux hosts. The core configuration, planning, reporting, and most provider validation code is ordinary Rust and should remain portable where practical, but release builds are expected to run on Linux.

## Supported Host

- Linux is the supported host platform for release builds.
- Commands that execute external build tools assume a Unix-style process model, paths, permissions, and signals.
- `gaia-process` uses Unix process groups on Unix hosts so cancellation and timeout handling can terminate subprocess trees. Non-Unix builds can compile the core runner, but they do not get equivalent process-tree cleanup semantics.

## Privileged Providers

The starting-point image provider has Linux host requirements for mutable rootfs and raw-image workflows:

- package reconciliation with `execute=true` uses `chroot` and bind mounts;
- raw image mutation uses `losetup`, partition refresh, `lsblk`, `mount`, and `umount`;
- image-feed overlays against mutable raw images require root privileges.

These flows fail early with a policy error when root privileges are missing. On non-Linux hosts, privileged starting-point mutation is reported as unsupported instead of attempting partial execution.

Read-only planning, config validation, and dry-run package reconciliation do not require privileged host mutation.
