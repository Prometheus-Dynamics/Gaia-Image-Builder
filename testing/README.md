# Testing

Keep testing surfaces explicit and documented.

Test scratch directories live under the OS temp directory at `gaia-tests/`, for example `/tmp/gaia-tests` on most Linux hosts.
Tests must create their scratch space with `gaia_testkit::TempDir` (`crates/core/testkit`), which removes its directory when dropped, including when the test panics.
Do not hand-roll `std::env::temp_dir()` paths or rely on manual `remove_dir_all` calls at the end of a test; those leak on failure.
`TempDir::keep_on_failure()` is the only exception and keeps the directory only when the test panics.

Fixtures can add up to tens of megabytes per run, and `/tmp` is often a RAM-backed tmpfs. To keep the suite off RAM, point `TMPDIR` at a disk directory:

```sh
TMPDIR=/path/on/disk/gaia-test-tmp cargo test --workspace --no-fail-fast
```

After a run, `ls -A "$TMPDIR/gaia-tests"` should be empty. If anything remains, a test is leaking; fix that test rather than deleting the directory by hand.

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
