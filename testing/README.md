# Testing

Keep testing surfaces explicit and documented.

Helper-generated test scratch directories live under the OS temp directory at `gaia-tests/`, for example `/tmp/gaia-tests` on most Linux hosts.
Clean all such test scratch data by removing that one directory.

## Default Surface

- `./scripts/check-file-sizes.sh`
- `cargo fmt --check`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Extended Surface

Add Docker, remote-backend, soak, or hardware-specific suites only when they cover real behavior that the default surface cannot.

- `./scripts/run-starting-point-raw-smoke.sh` exercises privileged raw-image loop setup, partition mount, package mutation, image-feed application, unmount, and loop detach inside a privileged Docker container.
- Timeout/cancel behavior for long-running provider commands is covered by image-provider unit tests; rerun the raw smoke after cleanup changes to confirm host loop devices and mounts are not left behind.

## Additional Coverage

- File-size linting is warning-only, supports `FILE_SIZE_EXCLUDE_DIRS=path1:path2`, and tracks any current exceptions through `testing/ci/file-size-baseline.txt`
- The existing GitHub Actions remote checkpoint workflow remains the extended surface for networked backend validation
