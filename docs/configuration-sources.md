# Configuration: Sources and toolchains

[Configuration index](configuration.md)

## Sources

```toml
[[sources]]
id = "workspace-root"
kind = "path"
path = "${workspace.root_dir}"
refresh = "never"
pin = "locked"

[[sources]]
id = "buildroot-upstream"
kind = "git"
repo = "https://github.com/buildroot/buildroot.git"
tag = "2025.11"
update = true
refresh = "always"
pin = "floating"

[[sources]]
id = "seed-rootfs"
kind = "archive"
path = "@assets/rootfs/base-rootfs.tar"
strip_components = 0
refresh = "never"
pin = "locked"

[[sources]]
id = "tool-cache"
kind = "download"
url = "https://example.invalid/tool.tar.xz"
sha256 = "abc123"
output_path = "${workspace.build_dir}/downloads/tool.tar.xz"
refresh = "auto"
pin = "locked"
```

A git source can name a `subdir` of the repository. Gaia still checks out the
whole repository; `subdir` is recorded next to the checkout
(`selected-subdir.txt`) and is part of the source's identity, so changing it
rebuilds what uses the source. Point an artifact at the directory itself with
its own path inside the source.

Source kinds:
- `git`
- `path`
- `archive`
- `download`

An archive's symlinks must resolve inside it: a target with `..` that stays inside (`include/dt-bindings/input/../../uapi/...`) is fine, one that leaves is refused, and so is an absolute target unless the source sets `allow_absolute_symlinks = true`. A root filesystem or an SDK's sysroot needs that: its absolute links are meant relative to that root, the link is created inside the destination, and GNU tar does not write through a link it extracted. Hard links between members of the archive are extracted; one to a path outside it, or one that `strip_components` removes, is refused.

An `archive` source takes either `path` (an archive in the workspace) or `url`
with its `sha256`, never both (`sha256` alone, with `path`, is an error). A
`url` archive is downloaded (with the archive provider's `timeout_seconds` and retries: `[providers.archive] timeout_seconds = 3600` for a large SDK) into the download cache (see "Download Cache"
below), its bytes are hashed and checked against `sha256` before anything is
extracted (a mismatch is an error that names the expected and the actual
digest, and nothing is stored or extracted), and then it is extracted as a
`path` archive is, with `strip_components`. Its content is its `sha256`, so
two checkouts with the same archive share their build entries. It is pinned
(`pin = "locked"`) by default, as a download with a checksum is. Offline
(`[execution] offline`) it is read from the cache only, and a missing entry is
an error that says to prefetch online first. `url` takes `http://`,
`https://` or `file://`.

```toml
[[sources]]
id = "seed-rootfs"
kind = "archive"
url = "https://example.invalid/base-rootfs.tar.gz"
sha256 = "<64 hex digits of the archive>"
strip_components = 0
```

Policies:
- `refresh = "auto" | "always" | "never"`
- `pin = "floating" | "locked"`

A floating ref's plan-time `git ls-remote` is reused for 60 seconds within one
process. `GAIA_FLOATING_REF_TTL_SECONDS` sets that window; `0` resolves the ref
for every plan.

`pin = "locked"` on its own only means "do not re-fetch once materialized":
each machine keeps whatever commit it fetched first. To pin git sources to
the same commit everywhere, use a lockfile.

A git source with `refresh = "auto"`, `pin = "floating"` and a `branch` (or no
ref, for `HEAD`) is checked at plan time instead of being re-materialized on
every run. Gaia runs `git ls-remote` for the ref, once per ref per plan (refs
are resolved in parallel and each call is limited to 30 seconds), and compares
the commit with the one its last materialization recorded:

- The ref still points at that commit: the source is reused, and so is what
  was built from it. The plan reports it as `reused from floating ref 'main'
  still at <short commit>`.
- The ref moved: the source materializes and its dependents rebuild. The new
  commit is part of the source fingerprint, so a moved ref also invalidates
  reuse on its own.
- The ref cannot be resolved (offline, unreachable, or the branch is missing):
  the source materializes, and the plan says why.

The check needs `[providers.git] allow_remote_resolution = true` for a remote
repository; a local repository is checked either way. A source is not checked
when it names a `tag` or `rev`, pins with `pin = "locked"` or a lockfile, uses
`git_backend = "execution"` with docker, or refreshes with `refresh = "always"`
(which always materializes). A build upgraded to this version materializes each
such source once, because its fingerprint now includes the commit.

#### Git source credentials

Git sources run their git commands (mirror clone and fetch, `ls-remote`,
clone, checkout) on the host with your own environment: git config,
credential helpers (for example `gh auth setup-git`), the ssh agent and
`GIT_*` variables. This holds even when the build runs in docker, which is
why a private repository works with `[policy.execution] docker`. Builds
still run in docker; only git runs on the host.

- `git_backend = "host" | "execution"` on a `kind = "git"` source. The default
  is `"host"`. `"execution"` runs the git commands through the execution
  backend (docker, when configured) as older versions did.
- If `git` is not installed on the host, a `"host"` source falls back to the
  execution backend and the build log says so.
- Git runs with `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never`: a
  missing credential fails at once with the source id and the redacted URL,
  and the error suggests a credential helper or an ssh key. It does not
  prompt or hang. ssh keys that need a passphrase must be loaded in ssh-agent.
- Credentials in a URL (`https://user:token@host/...`) are never written by
  Gaia. Logs, errors, `.gaia-source-state.txt`, the source marker, the
  provenance and manifest reports and `<build>.gaia.lock` show them as
  `https://***@host/...`. Prefer a credential helper over embedding a token;
  a token in the URL is still stored in the mirror's `.git/config` under
  `.gaia/cache/git` (the checkout's copy is redacted).
- The `git_backend` setting does not change source fingerprints: a build
  upgraded to this version keeps its materialized sources.

**Git checkouts.** A materialized git source is updated in place on each run
instead of being deleted and cloned again. Its `.gaia` state (such as
`.gaia/cargo-target`) is kept, and the checkout is made to match its commit
exactly: tracked files are reset, and untracked and ignored files are removed
(`git clean -ffdx`), so the tree is the one a fresh clone gives. A checkout
already at its commit with nothing changed is left alone. A checkout that
cannot be updated (another origin, a damaged `.git`, a commit that will not
check out) is cloned again.

- A remote repository's checkout is cloned from its mirror in
  `.gaia/cache/git` and shares the mirror's objects instead of copying them.
  Keep the mirror while checkouts use it: `gaia clean --unused-mirrors` removes
  a mirror that no source maps to, and a checkout that lost its objects is
  cloned again on its next run.
- The mirror is fetched only when the source needs it. A locked commit, or a
  `rev` that is a full commit id, already in the mirror needs no fetch and no
  network. A branch or tag is fetched and resolved from the mirror after that
  fetch, so no separate `ls-remote` runs; if the fetch fails, the source fails
  rather than building from stale refs. The plan-time check of a floating ref
  is unchanged.
- A clean checkout's `materialized_tree_digest` is its git tree id
  (`git-tree:<id>`). The tree names every file's content and mode, and a
  submodule's commit, so no file is read to compute it. A checkout with local
  changes is hashed file by file as before. The digest is only compared for
  equality, so the tree id is a stable identity. Upgrading changes the form of
  the recorded digest, so the dependents of each git source rebuild once.

**Path source identity.** A path source inside a git work tree is identified by
the files git lists for it: tracked files and untracked files that are not
ignored, with `identity_ignore` and the built-in ignores applied on top. Ignored
build output such as `target/` or `node_modules/` is never read, so validating a
build on a large checkout stays fast. Outside a git work tree the whole directory
is walked as before. The identity of a git-backed path source changed once with
this rule, so its first build after the change may rebuild.

### Git Source Lockfile

`gaia lock <build.toml>` writes `<build>.gaia.lock` next to the build
entrypoint (for example `configs/builds/evk.toml` →
`configs/builds/evk.gaia.lock`). Commit it alongside the build config.

```toml
# Generated by `gaia lock`. Commit this file to pin git sources.
version = 1

[[git]]
source = "worker"
repo = "https://github.com/example/worker.git"
ref = "branch:main"
commit = "3f2c9e0d4b1a7c8e6f5d4c3b2a1908f7e6d5c4b3"
```

Each entry is keyed by the source id plus its `repo` and `ref` (`branch:<name>`,
`tag:<name>`, or `head:HEAD`). When an entry matches the configured source:
- materialization clones the repository and checks out exactly that commit
  (detached), instead of the branch or tag tip; remote sources skip the
  `ls-remote` resolution step,
- the source counts as pinned, so `refresh = "auto"` no longer re-materializes
  a remote branch on every run,
- the locked commit is part of the source fingerprint, so moving the lock
  (`gaia lock --update`) re-materializes the source and rebuilds what depends
  on it, while new upstream commits do not,
- the source state records `lock_mode=locked` and `locked_commit_sha`.

Validation warns with `git_lock_stale` when an entry exists for a source whose
`repo` or ref changed; the stale entry is ignored (the source floats) until
`gaia lock` rewrites it. An unreadable lockfile is a validation error
(`git_lockfile_invalid`). Sources with an explicit `rev` are already pinned and
get no entry. Without a lockfile, git sources behave exactly as before.

Validation also warns with `git_source_ref_divergence` when two sources point
at the same repository (ignoring a trailing `.git`) at different refs.

### Download Cache

Download sources with a `sha256` are stored after verification in a
content-addressed cache, `<workspace>/.gaia/cache/downloads/sha256/<sha>`,
shared by every build in the workspace. An archive source with a `url` uses
the same cache, under the same name. Re-materializing the source copies
the cached file instead of downloading it again. Downloads without a
`sha256` are always fetched. The file is hashed once per download, both to
verify it and to record `output_sha256` in the source state. Remove the cache
with `gaia clean <build> --all-caches`.

## Toolchains

`[[toolchains]]` declares a cross toolchain that an external OS SDK provides,
so a rust artifact can be built for a device. Gaia installs nothing: the SDK
is read where it is. The SDK is a source (a downloaded and verified archive,
for example) or a local directory.

```toml
[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
source = "systemcore-sdk"                 # the SDK root is this source's tree ...
# path = "/opt/systemcore-sdk"            # ... or this directory (exactly one of the two)
relocate = true                           # run <root>/relocate-sdk.sh once per extracted location
prefix = "aarch64-buildroot-linux-gnu-"   # compiler prefix in <root>/bin (default: "<triple>-")
triple = "aarch64-buildroot-linux-gnu"
sysroot = "aarch64-buildroot-linux-gnu/sysroot"   # under the root ... (or: sysroot_source = "<source id>")
rust_target = "aarch64-unknown-linux-gnu"
```

- `id`: unique; the artifacts and device tables name it.
- `kind`: `"buildroot-sdk"`, the only kind. The SDK has `<root>/bin/<prefix>gcc`
  (and `g++`, `ar`) and a sysroot.
- `source` or `path`: the SDK root. A source's tree is the root once the
  source is materialized; the artifact waits for it.
- `relocate`: `false` by default. When `true`, `<root>/relocate-sdk.sh` runs
  once per root, and a marker file `.gaia-relocated` records that it ran, so
  later builds skip it. An SDK without the script is used as it is, and the
  build log says so.
- `prefix`: the compiler prefix in `<root>/bin`. Default: the triple followed
  by `-`.
- `triple`: required. The GNU triple of the SDK's compiler.
- `sysroot` (a path under the root, or absolute) or `sysroot_source` (a
  source whose tree is the sysroot): exactly one.
- `rust_target`: the Rust target the compiler serves. A rust artifact without
  a `target` of its own builds for it.
- `page_size`: the device's page size in bytes, a power of two of at least
  4096 (optional). Set, a rust artifact of this toolchain links with
  `-z max-page-size=<page_size>`, and a package built with it checks its
  segments against that size by default (see "Device checks"). The reason: the
  dynamic loader refuses a library whose `PT_LOAD` `p_align` is not a multiple of
  the kernel's page size, and a kernel with 16 KiB pages needs each segment's
  `p_vaddr` and `p_offset` congruent modulo 16 KiB. An SDK's linker writes 4 KiB
  segments by default, so a device with larger pages needs this. The flag is
  part of the toolchain's identity, so changing `page_size` rebuilds the
  artifacts.

A rust artifact names its toolchain with `toolchain = "<id>"`. Only rust
artifacts take one, and each needs a target (its own `target`, or the
toolchain's `rust_target`). The artifact then depends on the toolchain's
sources (the SDK's tree and the sysroot's), so it waits for them and rebuilds
when they change. Gaia runs cargo for the target with this environment (a
variable the calling environment already sets is kept in front of Gaia's
flags):

- `--target <rust_target>`, unless the artifact has its own `target`.
- `CARGO_TARGET_<TRIPLE>_LINKER` = `<root>/bin/<prefix>gcc`.
- `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` = `RUSTFLAGS` (or the target's own
  variable) from the calling environment, then `-C link-arg=--sysroot=<sysroot>`.
  Cargo reads the environment before `.cargo/config.toml`, so a
  `[target.<triple>] rustflags` in a config file is replaced by this variable:
  put such flags in `RUSTFLAGS` instead.
- `CC_`, `CXX_` and `AR_` for the target (the `cc` crate's names, with
  underscores): `<root>/bin/<prefix>gcc`, `<prefix>g++` and `<prefix>ar`.
- `CFLAGS_<target>` and `CXXFLAGS_<target>`: `--sysroot=<sysroot>` after the
  calling environment's value.
- `PKG_CONFIG_SYSROOT_DIR`, `PKG_CONFIG_PATH` (the sysroot's `usr/lib` and
  `usr/share` pkg-config directories) and `PKG_CONFIG_ALLOW_CROSS=1`.

With `execution` = docker, the SDK root and the sysroot are mounted at their
host paths, so the compiler and the sysroot are the ones the environment names.

The toolchain is part of the artifact's fingerprint and of its registry key:
a source root is identified by its content (its source identity), a local
`path` by its configured path together with the compiler's version. Another
SDK, prefix, sysroot or Rust target is another build.

`toolchain` is interpolated like the other artifact fields, so one artifact
can take its toolchain from an input: `toolchain = "${input.toolchain}"`. An
empty value after interpolation means no toolchain: the artifact builds for the
host.

Validation: an unknown toolchain id, a toolchain on a non-rust artifact, a
missing root, sysroot or triple, both `source` and `path` (or `sysroot` and
`sysroot_source`), and a rust artifact with no target are errors.

### SystemCore

Limelight's SystemCore OS is built with a Buildroot SDK. Its releases are on
GitHub (`LimelightVision/systemcore-os-public`). The ALPHA release is
`limelightosr-2027.0.0-alpha17-403` and the BETA release is
`limelightosr-2027.0.0-beta17-233`. The SDK is the `systemcore-aarch64-toolchain.tar.gz`
asset of ALPHA, and `systemcorebeta-aarch64-toolchain.tar.gz` of BETA: a
Buildroot SDK (`bin/aarch64-buildroot-linux-gnu-*`, `relocate-sdk.sh`, a
glibc 2.42 sysroot) under one top directory. `systemcorelinux.tar.xz`
(`systemcorebetalinux.tar.xz` for BETA) is SystemCore's Linux kernel source
tree with its `Module.symvers`, for out-of-tree kernel modules; it is not a
sysroot and this example does not use it. GitHub publishes each asset's
sha256 (`gh api repos/LimelightVision/systemcore-os-public/releases/tags/<tag>`,
the `digest` field). The values below are ALPHA 17's, and this example was
built against them: a rust program cross-compiled to an aarch64 PIE with
16 KiB segments and glibc needs up to `GLIBC_2.34`, packed into an ipk that
passes the device checks.

On a SystemCore (ALPHA 18), `opkg print-architecture` lists `all`, `noarch`
and `aarch64`: a package with binaries uses `Architecture = "aarch64"`, and
`all` is for packages without any (as Limelight's own add-ons are). An
`aarch64` ipk with `debian_binary = false` (Limelight's layout) installs and
upgrades with `opkg install --force-reinstall`, keeping its conffiles. The
standard layout (`debian_binary = true`) is not yet confirmed on the device.

A build that cross-compiles a rust program for SystemCore, packs it into an
ipk and checks the ipk against the SDK's sysroot:

```toml
[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[providers.rust]
allow_nested_build = true

# ALPHA. For BETA, use the systemcorebeta-* asset names and its release tag.
[[sources]]
id = "systemcore-sdk"
kind = "archive"
url = "https://github.com/LimelightVision/systemcore-os-public/releases/download/limelightosr-2027.0.0-alpha17-403/systemcore-aarch64-toolchain.tar.gz"
sha256 = "889ea7f6e6cc0d571aa43b4f02954f251016c5a5739f05c962d526429ae71990"
strip_components = 1                       # the SDK root is systemcore-aarch64-toolchain/
allow_absolute_symlinks = true             # its sysroot has absolute links (usr/bin/iptables-xml -> /usr/sbin/...)

[[sources]]
id = "helios"
kind = "path"
path = "helios"

[[toolchains]]
id = "systemcore"
kind = "buildroot-sdk"
source = "systemcore-sdk"
relocate = true
triple = "aarch64-buildroot-linux-gnu"
sysroot = "aarch64-buildroot-linux-gnu/sysroot"
rust_target = "aarch64-unknown-linux-gnu"
page_size = 16384                          # the device's pages are 16 KiB

[[artifacts]]
id = "helios"
kind = "rust"
package = "helios"
source = "helios"
toolchain = "systemcore"
profile = "release"
output_path = "${workspace.out_dir}/artifacts/helios"

[image]
kind = "none"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"
out_dir = "${workspace.out_dir}"

[[image.assembly.trees]]
id = "pkg-root"
path = "$assembly.work/pkg-root"

[[image.assembly.files]]
tree = "pkg-root"
src = "${workspace.out_dir}/artifacts/helios"
dest = "usr/bin/helios"
mode = "0755"

[[image.assembly.packages]]
id = "helios"
tree = "pkg-root"
dest = "$assembly.out/"
allowed_prefixes = ["/usr/bin"]

[image.assembly.packages.control]
Package = "helios"
Version = "${build.version}"
Architecture = "aarch64"
Maintainer = "Helios Team <dev@example.invalid>"
Description = "Helios for SystemCore."
Depends = ["libc6 (>= 2.38)"]

[image.assembly.packages.device]
toolchain = "systemcore"                   # the device's libraries: the SDK's sysroot
```

The SDK's sysroot is taken as the device's: the device checks read its
`libc.so.6` for the architecture and the highest `GLIBC_x.y` (`max_glibc`
defaults to it). The toolchain's `page_size` makes the rust binary link for
16 KiB pages, and the device checks take `min_page_size` from it (16 KiB here),
so nothing else is needed. Without a `page_size`, the checks default to 4 KiB,
and a binary the SDK links with 4 KiB segments passes them. An explicit
`min_page_size` in the device table always wins.
