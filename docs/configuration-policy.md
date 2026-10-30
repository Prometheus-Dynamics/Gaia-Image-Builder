# Configuration: Policy, workspace and clean

[Configuration index](configuration.md)

## Failure Policy

```toml
[failure]
rollback_on_error = true
rollback_completed = false
preserve_failed_outputs = false
rollback_domains = ["sources", "artifacts", "installs", "stage", "images", "checkpoints"]
keep_going = false
```

Meaning:
- `rollback_on_error` (default `true`)
  On failure or cancellation, clean the failed operation's own partial outputs.
  Completed operations' outputs are kept and recorded for reuse. Set `false` to
  leave all current-run outputs in place.
- `rollback_completed` (default `false`)
  Also unwind every completed current-run output on failure or cancellation,
  within `rollback_domains`. This was the default before finished work was kept.
  Only meaningful when `rollback_on_error = true`. Override with
  `--set policy.failure.rollback_completed=true`.
- `preserve_failed_outputs`
  Keep the failed operation’s partial outputs for debugging.
- `rollback_domains`
  Restrict cleanup to specific domains.
- `keep_going` (default `false`)
  After a failure, let independent operations run to completion instead of
  stopping them. Operations that depend (directly or transitively) on a failed
  operation are skipped and reported as skipped; the run still fails. Work that
  finished is kept rather than rolled back, and is recorded in the reuse state
  so the next run reuses it. Failed operations' partial outputs are still
  cleaned according to `rollback_on_error`, `preserve_failed_outputs` and
  `rollback_domains`. Override with `--set policy.failure.keep_going=true`.

Allowed rollback domains:
- `sources`
- `artifacts`
- `installs`
- `stage`
- `images`
- `checkpoints`

## Execution Policy

```toml
[execution]
jobs = 4

[execution.output_retention]
stdout_bytes = 1048576
stderr_bytes = 1048576
stdout_lines = 1000
stderr_lines = 1000
failure_tail_lines = 100
```

Output retention controls how much external command output Gaia keeps in memory and reports. The byte and line limits apply to retained stdout/stderr tails from subprocess execution. `failure_tail_lines` controls how many merged log lines are copied into structured failure reports.

Each value defaults to the shown release default when omitted or set to `0`. The same fields can be set from the CLI with `--set execution.output_retention.<field>=...` or `--set policy.execution.output_retention.<field>=...`.

`offline = true` builds without network access, as `gaia run --offline` does (see `docs/cli.md`, Offline builds). It is a run setting: the build config records what to build, and a run decides whether to reach the network. The default is `false`.

```toml
[execution]
offline = true
```

`jobs` controls Gaia's operation scheduler. It limits how many independent Gaia operations may run at once; it is not forwarded to backend build tools as-is.

Ready operations that can run in parallel start first, in plan order, while an exclusive operation waits. An exclusive operation starts once nothing else runs, and nothing starts beside it. So that it cannot wait forever, once eight starts have passed a waiting operation, Gaia starts nothing new until that operation can run. An operation never runs beside one that holds the same resource (the same destination, output or package directory).

When several CPU-heavy operations (artifact builds, Buildroot prepare/build) run at the same time, Gaia caps each spawned tool with `CARGO_BUILD_JOBS`, `MAKEFLAGS=-jN` and `CMAKE_BUILD_PARALLEL_LEVEL` (and Buildroot's `make -j` when `providers.buildroot.local_jobs = 0`). Each one is sized when it starts, from the cores the running heavy operations do not hold: the free cores are split between it and the heavy operations that can start beside it. A tool reads its job count once, so a running operation keeps its budget; the cores a finished operation held go to the operations that start after it. A heavy operation that runs alone keeps its tools' defaults. Variables you already set in the environment are never overridden, and an explicit `local_jobs` wins.


`failure_tail_lines` also bounds streamed build output: every line goes to the live progress/TUI sink as it arrives, but only the newest `failure_tail_lines` lines are retained, and only failed operations report them. Successful operations do not re-emit their build log.

### Docker Containers

With `[execution.docker] enabled = true`, each command runs in the docker image. By default every command is one `docker run --rm --init`.

```toml
[providers.docker]
reuse_container = true   # default false
```

With `reuse_container = true`, a run starts one long-lived container per image, mount set and user (`docker run -d --init ... sleep infinity`) and runs its commands with `docker exec`, with the same working directory, user and environment. The container is removed when the run ends, when the run is cancelled, and when it unwinds from a panic. The image must contain `sleep`.

Measured on this host (`debian:sid`, 20 runs of `true`): 3.6 s per command with `docker run`, 41 ms with `docker exec`. A cancelled command's processes are not killed inside the container when the `docker exec` client stops; they end when the container is removed at the end of the run. If Gaia is killed outright, remove the containers by hand (`docker rm -f`).

Build-context hashes (`dockerfile` artifacts) read each file in chunks and skip re-reading a context whose files have the same sizes, modification times and modes as at the last hash in the same process.

## Provider Execution Policy

Provider policy lives under `[providers.*]`.

Rust, Git and Java have extra specialized fields:
- Rust: `allow_nested_build`, `batch_builds`, `shared_target_dir`, `sccache`, `strip`
- Git: `allow_remote_resolution`
- Java: `gradle_home` (see [Java and Gradle](#java-and-gradle))

`allow_nested_build` (default `false`) is what lets Gaia run cargo for a rust
artifact. Without it a rust artifact is built only when its output file already
exists (that file is used as it is); otherwise the build fails with an error
that names the artifact and the output. Gaia never writes a placeholder file in
its place, because one would be shipped as the binary.

`batch_builds` (default `false`) builds nested cargo artifacts that share a
source workspace, target triple, profile, feature flags and execution backend
with a single `cargo build -p a -p b ...` (one container start and one
dependency resolution) and then copies each output. Per-artifact outputs,
marker and state files are the same as for individual builds. When cargo names
the packages that failed to compile (the "could not compile" lines name them), those
artifacts fail with cargo's error and the others are built on their own. When
the batch fails without naming a package (for example a network failure), each
artifact is rebuilt on its own. Note that cargo unifies dependency features across all
packages selected in one invocation, so a batched binary can differ from the
same package built alone when packages enable different features of a shared
dependency. It is therefore opt-in; enable it when your packages agree on
shared dependency features. Artifacts that must agree on features should use
a [Rust build group](configuration-artifacts.md#rust-build-groups) instead: groups always build together,
with or without `batch_builds`.

`sccache` (default `false`) compiles nested cargo artifacts through sccache:
`RUSTC_WRAPPER=sccache` is set when `sccache --version` runs where cargo runs,
that is on the host, or inside the docker image. On the host the cache lives
under the user cache root (`<user cache>/sccache`, unless `SCCACHE_DIR` is set);
docker builds keep it in the workspace cache. Without sccache the build runs
without it and the log says so. The compiler wrapper does not change the
outputs, so recorded state does not depend on it.

`strip` (default `false`) strips every collected Rust output. The copy in the
output path is stripped with the target's strip tool: `<triple>-strip` for a
cross target, `strip` for the host, and `llvm-strip` as the fallback for both.
Cargo's own files in the target dir are not touched. A host without any of
these tools fails the artifact. The setting is part of the recorded state, so
changing it rebuilds the artifact.

`shared_target_dir` (default `false`) builds the nested cargo artifacts of
every source into one cargo target directory per toolchain, target triple,
profile and execution backend, under the user cache:
`<user cache>/cargo-target/<key>` (`$GAIA_CACHE_DIR`, else
`$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`). Sources built with the same
toolchain then reuse each other's compiled dependencies. By default each
source builds into `<source>/.gaia/cargo-target` and compiles its dependencies
again. Docker builds mount the shared directory at its own path. The build log
names the directory in use.

Cargo identifies a package by name and version, so two sources cannot share
one directory when they have a local (path or workspace) package with the same
name and version in different directories: cargo would reuse the first
package's outputs for the second. A source whose local packages collide with
packages already built from another directory keeps its own directory, and the
build log says why. Registry and git packages are shared without this check.
Gaia lists a source's packages with `cargo metadata`, run on the host; if that
fails, the source keeps its own directory.

Notes: concurrent builds in one shared directory wait for cargo's build lock,
and that wait counts toward `timeout_seconds`. The directory is not cleaned up
automatically; delete it under the user cache to reclaim space. Build outputs
and recorded artifact state do not depend on the directory, so switching the
setting does not invalidate recorded artifacts. It is opt-in for now.

Every provider supports:
- `retry_attempts`
- `retry_backoff_ms`
- `retry_backoff_strategy`
- `timeout_seconds`

`retry_attempts` applies only to failures that look transient: a command killed
by a signal, or stderr that reports a network failure (`failed to download`,
`spurious network error`, `failed to fetch`, `connection reset`, ...). A
deterministic failure, such as a cargo compile error (`error[E...]`, `could not
compile`), fails at once: the same inputs fail the same way.

Command providers may also expose provider-local worker counts. Buildroot uses `local_jobs` for `make -j`; leave it at `0` to let Buildroot choose its own default or set it explicitly to avoid nested oversubscription.

Buildroot's long `make` (the build itself) has its own settings, because a
build takes hours and a compile error fails the same way every time:
- `make_timeout_seconds` (default `0`, no timeout) bounds that `make`. The
  other Buildroot commands (defconfig, olddefconfig, clean) keep
  `timeout_seconds`.
- `make_retry_attempts` (default `1`) is retried only when stderr reports a
  download or network failure (`failed to download`, `could not resolve host`,
  `connection timed out`, ...). A compile error is never retried.
- `erofs_workers` (default `"auto"`, the smaller of 8 and the host's cores)
  builds erofs-utils with `--enable-multithreading` and passes `--workers` to
  `mkfs.erofs` for EROFS rootfs images. The image is byte-identical for every
  count, so the setting does not change outputs. `"off"` or `0` leaves both
  alone. Changing it on or off rebuilds `host-erofs-utils` once.

Example:

```toml
[providers.rust]
allow_nested_build = false
batch_builds = true  # opt-in; see above
shared_target_dir = true  # opt-in; see above
sccache = true  # opt-in; see above
strip = true  # opt-in; see above
retry_attempts = 2
retry_backoff_ms = 500
retry_backoff_strategy = "exponential"
timeout_seconds = 300

[providers.git]
allow_remote_resolution = true
retry_attempts = 2
retry_backoff_ms = 250
retry_backoff_strategy = "fixed"
timeout_seconds = 60

[providers.buildroot]
retry_attempts = 1
retry_backoff_ms = 0
retry_backoff_strategy = "fixed"
timeout_seconds = 900
make_timeout_seconds = 0          # the long make that builds the image; 0 = none
make_retry_attempts = 1           # its attempts, retried only for network failures
erofs_workers = "auto"            # mkfs.erofs threads: "auto", "off", or a count
local_jobs = 4
parallel_packages = true
# download_dir = "/srv/buildroot-dl"     # default: the user cache below

[providers.buildroot.ccache]
enabled = true
# dir = "/srv/buildroot-ccache"          # default: the user cache below
# max_size = "50G"                       # default: from free space, see below
```

Downloads and the compiler cache are shared by every build of every project
of the user by default, so a second project, or a rebuild after the output
tree was wiped, reuses them. They live under the user cache root:
`$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`
(`buildroot/dl` and `buildroot/ccache`), or under `<workspace>/.gaia/cache`
when that cannot be created.

- `download_dir` is passed as `BR2_DL_DIR` (written to `.config` when set,
  through the environment otherwise), so source tarballs survive clean
  builds and re-fetching the Buildroot source.
- `ccache.enabled = true` sets `BR2_CCACHE=y`, `BR2_CCACHE_DIR` and
  `BR2_CCACHE_USE_BASEDIR=y` (paths relative to the output tree, so trees in
  other directories share cache entries), and sets `max_size` in the cache's
  `ccache.conf`. Buildroot keys entries on its toolchain, so builds with the
  same toolchain share them. The run summary reports how many of the run's
  compilations came from the cache:
  `buildroot ccache: 8123/9410 compilations from cache (86.3%)`.
- The compiler cache's `max_size`, unset, is computed when a run starts, from
  the free space `F` on the cache directory's filesystem and the bytes `C`
  the cache already holds: `clamp(F / 2 + C, 2 GiB, 40 GiB)`, written to
  `ccache.conf` in whole MiB and logged. A cache directory that does not exist
  yet holds nothing. When the free space cannot be read the size is 40 GiB.
  Only an explicit `max_size` larger than the free space warns; the default
  never does. The size is written to `ccache.conf` at run time only: it is not
  in `.config`, so a change in free space rebuilds nothing. ccache reads the
  limit on each run and trims the cache to it when it cleans up, so a lowered
  `max_size` takes effect at that cleanup.
- `parallel_packages = true` builds independent packages concurrently: it
  sets `BR2_PER_PACKAGE_DIRECTORIES=y` and runs the top-level `make` with
  `-j<local_jobs>` and a load limit of the same value (inherited by each
  package's own `make`, which still uses `BR2_JLEVEL` jobs). Buildroot marks
  per-package directories experimental; a few packages may not support them.
  It also splits the `make`: Gaia builds up to `target-finalize`, digests
  everything the filesystem image step reads (the finalized target tree by
  content, the images packages installed such as the kernel and device
  trees, the whole `.config`, the post-image and fakeroot scripts, users and
  device tables and the directories they are in, Buildroot's `fs/` and
  `support/scripts`, and the host packages built), and runs the rootfs
  images and post-image script only when that digest differs from the one
  recorded with the current images, or an expected image is missing. A
  rebuild where nothing those read changed skips EROFS, ext4, squashfs and
  genimage entirely.
- Turning `ccache.enabled` or `parallel_packages` on or off changes the
  toolchain wrapper or the tree layout, so the next build cleans the output
  tree once.

#### Work directory

```toml
[providers.buildroot]
work_dir = "disk"                 # "disk" (default), "ram", or a directory
ram_budget = "60G"                # most RAM a "ram" tree may use
keep_ram_tree = true              # keep the tree after a build for fast rebuilds
```

`work_dir = "ram"` builds the output tree on tmpfs under `/dev/shm`, which
makes compiles and tree writes much faster. The tree is kept for rebuilds
unless `keep_ram_tree = false`, and the build falls back to disk when RAM is
short (`ram_budget` caps the RAM the tree may take).

The tree lives in `/dev/shm/gaia-<user>/`, named by the account name of the
real user id (from the password database, not `$USER`, which any process can
set). Gaia creates that directory with mode 0700, tightens an existing one of
yours to 0700, and refuses to use it when it is a symbolic link or is owned by
another user, with a message naming the directory to remove.

#### Host tools

```toml
[providers.buildroot.host_tools]
default = "build"                 # policy for tools not listed below
ccache = "system,build"           # per tool
pkgconf = "system,build"
```

Each policy is a comma-separated, non-empty list of steps tried in order:
`system` uses the build environment's own tool, `build` uses the tool
Buildroot builds for itself, and `fail` stops the build with an error naming
the tool and what was required. `"system,build"` uses the system tool when
one is usable and builds it otherwise; `"build,system"` prefers the built
tool. A `--set` value with an unknown step is rejected. In the TOML file, an
invalid policy compiles to `fail` (the build stops rather than silently
choosing a default), and tool names other than `ccache` and `pkgconf` are
ignored.

`system` currently applies to `ccache` and `pkgconf`. For `ccache`, Gaia uses
the build environment's `ccache` 4.x through a wrapper in `HOST_DIR/bin` that
points it at `BR2_CCACHE_DIR`, and removes Buildroot's `host-ccache` and its
`host-zstd`, `host-hiredis`, `host-xxhash` and `host-blake3` dependencies.
The decision and the system tool's version enter the package cache keys, so
switching a tool between `system` and `build` does not reuse packages built
the other way.

For `pkgconf`, Gaia uses the build environment's `pkgconf` 1.8 or newer and
keeps Buildroot's own `pkg-config` wrapper (the same install and `sed` steps,
with the static or shared variant), so the wrapper's environment and flags
are unchanged. Only `HOST_DIR/bin/pkgconf`, the binary the wrapper runs, is
replaced by a script that executes the system `pkgconf`; `host-pkgconf`'s
configure, build and other installed files are skipped. The system's
`pkg.m4` is copied into `HOST_DIR/share/aclocal` when present, so
`PKG_CHECK_MODULES` in host autotools packages still works. Two things differ
from Buildroot's own tool: Buildroot's pkgconf 2.3.0 carries a patch that
applies the sysroot only to a few variables (`includedir`, `libdir`, ...),
which the system pkgconf does not do, so a `pkg-config --variable` of another
path can come out prefixed with the staging directory; and the system's
`pkgconf` may differ in its defaults. The version floor is the oldest one
whose options the wrapper uses (`--keep-system-libs`, `--static`) and the
build image's 1.8.1 has.

#### Package cache

```toml
[providers.buildroot]
parallel_packages = true          # required

[providers.buildroot.package_cache]
enabled = true
# level = "system"                # default level for packages: "system" | "project"
# system_dir = "/srv/buildroot-packages"   # default: see below
# project_dir = ".gaia/cache/buildroot/packages"
# project_packages = ["myapp-*", "my-tool"]  # kept in this project only
# system_packages = []            # shared even when level = "project"
# max_size = "100G"               # per level; unset: see "Size budget" below
```

Built packages are cached by content, so a wiped output tree or a second
image with the same kernel, Mesa or libcamera restores them instead of
compiling. After a
successful `make`, each package built in that run is stored as a
directory: the files it added to its per-package directories (everything
that is not a hard link to the same path in a direct dependency's trees, so
also what it installs outside its install steps, such as an extracted
external toolchain), its image files, file lists, kconfig `.config` and
stamps. Files are cloned with `cp --reflink=auto`: when the cache is on the
build's filesystem and that supports reflinks (btrfs, XFS), storing and
restoring copy no file data.

The cache has two levels:

- the **system level**, shared by every project of the user:
  `system_dir`, by default `<user cache root>/buildroot/packages` (the user
  cache root is `$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else
  `~/.cache/gaia`). When that default is on another filesystem than the
  build, it would copy every file, so its packages go to the project level
  instead (with a note in the run output); set `system_dir` to a directory on
  the build disk to share packages between projects.
- the **project level**: `project_dir`, by default
  `<workspace>/.gaia/cache/buildroot/packages`.

Packages are stored at `level` (default `system`), except those matching
`project_packages` (stored at the project level, for the project's own,
frequently changing packages) or `system_packages` (stored at the system
level when `level = "project"`). Restores look in the project level first,
then the system level. `dir` is accepted as an alias of `system_dir`.

**Size budget.** Each level is kept at its `max_size` (least recently used
packages are evicted first, until the level is at nine tenths of it). Unset,
a level's budget is computed when a run starts, from the free space `F` on
the filesystem holding that level and the bytes `C` the level already holds:

    budget = clamp(F / 2 + C, 2 GiB, 40 GiB)

so an existing cache is not over budget merely for being there, and a nearly
full disk gets a small cache. When the free space cannot be read, the budget
is 40 GiB. Eviction runs at the start of a run (when a lowered `max_size` or
a large cache is over its budget) and after packages are stored; each
eviction is reported in the run output with its count and size. A `max_size`
that is set is only warned about when it is larger than the free space; the
default never warns.

`gaia cache` manages the cache entry by entry, so one bad package never
means wiping everything:

```sh
gaia cache build.toml                          # list entries, largest first, with totals per level
gaia cache build.toml --level project --package 'mesa*'
gaia cache build.toml --remove mesa3d          # every cached build of a package
gaia cache build.toml --remove linux@ab12cd    # one build, by key prefix
gaia cache build.toml --clear project          # a whole level (or system, or ccache)
gaia cache build.toml --remove mesa3d --dry-run
```

To build without the cache for one run, pass
`--set policy.providers.buildroot.package_cache.enabled=false`.

The keys ignore Gaia's own patches to Buildroot (such as the reflink copy of
per-package directories), so a build with and without them shares entries.

Before the next `make`, packages not yet built are restored,
dependencies first, when their key is cached and all their dependencies are
built or restored: as Buildroot's per-package preparation does, their
dependencies' trees are linked in, then their own files are added, and their
stamps are recreated in build order so `make` treats them as built.

A package's key covers:
- Buildroot's package infrastructure and the settings it references
  (architecture, toolchain, optimisation, hardening, init system);
- the package's `.mk` directory, `.hash` files, patches, version and
  download names;
- the values of the settings its `.mk` references, where a setting naming a
  file or directory counts by content, not path;
- the Docker image (or host compiler) it is built with;
- its dependencies' keys.

It does not depend on where the output tree is, the build name, or packages
the package does not use.

Packages built from a local directory (`SITE_METHOD = local`,
`<PKG>_OVERRIDE_SRCDIR`), and everything depending on them, are always
built. Before each `make`, the stamps of every built package whose key is
unchanged are made newer than its inputs, so sources copied again (newer
files, same content) neither rebuild packages nor break restored kconfig
packages, which have no sources to redo a step from. In text files that hold the output tree's path, it is rewritten on
restore. Packages whose binaries hold it (most host tools) are stored for
that path only: they are restored when the same build is rebuilt after a
wipe, and in other trees once they are built there, but not across trees at
different paths, which keeps their dependents building there too. Running
builds in Docker with the output tree at a fixed path lifts this. `linux` is
restored only when nothing still to be built needs its build tree (for
example out-of-tree kernel modules). The run summary reports
`buildroot package cache: <n> restored, <m> stored`.

Cache directories outside the workspace are mounted into Docker builds.

Buildroot never rebuilds a built package by itself, so Gaia compares the
effective `.config` and the package override trees with those the output tree
was last built from, and rebuilds as little as keeps it correct:

- Settings that cannot change what packages build are ignored: the version
  header, `BR2_DL_DIR`, `BR2_CCACHE_DIR`, `BR2_JLEVEL`, download mirrors,
  `BR2_EXTERNAL_*` (tree names, paths, versions), root filesystem image
  settings (`BR2_TARGET_ROOTFS_*`, post-image and fakeroot scripts, users and
  device tables) and import-source checkout directories in paths. Root
  filesystem images are regenerated on every `make`.
- Newly enabled packages, including new packages in override or external
  trees, simply build.
- A built package whose options, override directory contents or version
  changed is uninstalled (the files it installed in `target/`, `staging/`
  and `host/` that no other package also installed are removed) and its
  build directory is removed (`<pkg>-dirclean`), as is every package that
  depends on it, recursively.
- A package no longer enabled is uninstalled, and the packages that depended
  on it are rebuilt.
- A built package that gained or lost a dependency (for example kmod once xz
  is enabled) is rebuilt.
- A setting that belongs to no package (an external tree's own option, for
  example) is looked up in what reads it:
  - Buildroot's infrastructure (`Makefile`, `package/Makefile.in`,
    `package/pkg-*.mk`, `toolchain/`, `arch/`, `system/system.mk`), an
    architecture or CPU choice, or an `external.mk` using it for something
    Gaia cannot follow (an `include`, a rule, a global variable such as
    `TARGET_CFLAGS`): the whole output tree is cleaned.
  - Package `.mk` files, or `<PKG>_*` variables and hooks an `external.mk`
    sets from it (directly or through its own variables): those packages
    are rebuilt.
  - Only target finalization: `BR2_ROOTFS_OVERLAY`,
    `BR2_ROOTFS_POST_BUILD_SCRIPT`, the hostname, issue and root password,
    or an `external.mk` feeding it to `PACKAGES_USERS`,
    `PACKAGES_PERMISSIONS_TABLE`, `PACKAGES_DEVICES_TABLE` or
    `TARGET_FINALIZE_HOOKS`. No package is rebuilt; with per-package
    directories `target/` is removed and reassembled from them, so files of
    a removed overlay go too (without them, the tree is cleaned).
  - Nothing at all: no package is rebuilt.

  Each decision is logged with its reason, for example
  `BR2_MYBOARD_DEVMGR_USERS_TABLE unset -> "/x/devmgr-users.table": only read
  when finalizing the target, no package rebuilt`.
- Toolchain, architecture, libc, init system and other system-wide settings
  still clean the whole output tree (`make clean`), as does any change when
  Buildroot cannot report its package graph (`make show-info`, which Gaia
  runs only when something changed).

Both the clean and the rebuild log what caused them, for example
`buildroot rebuild of 3 package(s): libcamera, libcamera-apps, myapp`
followed by `libcamera changed: BR2_PACKAGE_LIBCAMERA_PIPELINE_RPI_PISP unset
-> y`.

Gaia delivers the image feed (installs, stage files, env sets, services)
through Buildroot itself: it stages the feed next to the output tree and
appends a generated script to `BR2_ROOTFS_POST_BUILD_SCRIPT` on the `make`
command line (`.config` is not modified). Post-build scripts run after
package installation, stripping and rootfs overlays, so a single `make` packs
every image once with the feed included. Feed paths removed since the last
run are deleted from `target/` before `make`. If Buildroot does not run the
script, Gaia falls back to applying the feed after `make` and refreshing the
images.

Stage files mirror their sources. After each successful apply Gaia records,
in `<output>/.gaia-image-feed-stage-owned.txt`, the paths each `[[stage.files]]`
entry installed (files, symlinks and directories, with the size and
modification time a file had after the copy). On the next apply, a path an
entry owned and no longer produces is removed before the copy: a file whose
size and modification time still match, a symlink with the same target, and
a directory once it is empty. A file a package rewrote, a path that another
entry or the feed now produces, and anything the stage did not record stay.
A removed entry's paths are removed the same way. A moved path is kept, not
removed.

A tree built before this record existed has no record: its leftovers from
earlier runs are not removed, and the build says so once ("no stage ownership
record yet"). To remove them, clean that image's Buildroot output (or run a
clean build) once.

#### Shared Buildroot trees

```toml
[providers.buildroot]
shared_output = true
# optional; default ".gaia/cache/buildroot/shared" under the workspace root
shared_output_dir = "/var/cache/gaia/buildroot-shared"
```

By default every build compiles its own Buildroot output tree under
`${workspace.build_dir}/image/buildroot-output`. With `shared_output = true`,
builds whose Buildroot inputs are identical compile packages once, in a tree
at `<shared_output_dir>/<key>`. The key is a digest of:

- the Buildroot source identity (the resolved commit or content digest the
  source provider recorded),
- `defconfig`, `defconfig_path` and `config_fragments` (paths and content),
- `config_overrides`,
- the external tree and package override directories,
- `ccache.enabled`, `parallel_packages` (when on) and the Docker image.

The build name, build directory and image feed are not part of the key, so
`acme-base-os-evk` and `acme-full-evk` share one tree when only their
feeds differ. Changes inside external trees or package override directories
are handled in place, like a private tree.

Each build keeps a view at its usual output path: `build`, `host`, `staging`
and `per-package` link into the shared tree, `.config` is a copy, and
`target/` and `images/` are private copy-on-write clones (`cp --reflink=auto`,
falling back to a plain copy on filesystems without reflinks). The feed is
applied only to the private target, and each root filesystem image is packed
from it with the fakeroot script Buildroot generated for the shared tree, then
the post-image scripts run against the private `images/`. After the first
full `make`, the shared tree only runs `make target-finalize`, so each build
packs its images once. No file from one build's feed reaches the shared tree or
another build's image.

A file lock (`<key>.lock`) serializes builds that use the same tree; builds
with different keys run in parallel. When a build moves to a new key (for
example after a config change), it releases the old tree, which is deleted
once no build uses it. Turning `shared_output` off again replaces the view
with a private tree on the next run.

Limitations: initramfs, UBI, ISO9660, AXFS, cloop, OCI and YAFFS2 root
filesystems depend on more than the target tree and are rejected in shared
mode. Setting a different squashfs compression per build gives those builds
separate trees, because the key includes `config_overrides`.

#### Dropped config overrides

```toml
[providers.buildroot]
override_check = "error" # default; or "warn" / "off"
```

`olddefconfig` silently resets every symbol whose `depends on` is not met and
drops symbols that no longer exist, so a requested `BR2_PACKAGE_OPENJDK=y`
can vanish (for example when `BR2_PACKAGE_XORG7` is off) while validate and
plan pass. After the defconfig, fragments, `config_overrides` and cache
settings are applied (private and shared trees alike), Gaia compares every
`config_overrides` entry with the final `.config`:

- `y`, `m` or a value is **dropped** when the symbol is missing or
  `# KEY is not set`, and **changed** when it holds a different value;
- `n` (or `""`) is satisfied by a missing symbol or `# KEY is not set`;
- string quotes are optional, and `int`/`hex` values compare numerically;
- for repeated keys the last entry counts; `BR2_DL_DIR` and `BR2_CCACHE_DIR`
  are skipped because Gaia's cache policy rewrites them.

With `"error"` the image operation fails before the long `make`, listing each
symbol with the requested and final value and the hint
"usually an unmet `depends on`; check menuconfig for <KEY>". With `"warn"` the
build continues and each entry is reported as a warning: in the `gaia run`
output, in `summary.json` (`image_warnings`, counted in `warning_count`) and
on the image record of `manifest.json` (`warnings`). `"off"` skips the
comparison. `--set policy.providers.buildroot.override_check=warn` relaxes it
for one run.

#### Faster compression for development builds

Buildroot `config_overrides` can be set from presets and `--set` with
`image.buildroot.config_overrides.<SYMBOL>`. Keep XZ in the base config for
releases and switch to zstd (or lz4) in a development preset:

```toml
[image]
kind = "buildroot"
config_overrides = [["BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "y"]]

[presets.dev]
overrides = [
  ["image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_XZ", "n"],
  ["image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_ZSTD", "y"],
]
```

```sh
gaia run configs/builds/acme.toml --preset dev
# or, for one run:
gaia run configs/builds/acme.toml \
  --set image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_XZ=n \
  --set image.buildroot.config_overrides.BR2_TARGET_ROOTFS_SQUASHFS4_ZSTD=y
```

Set the old choice to `n` explicitly so Kconfig does not keep it. Switching
compression does not clean the Buildroot output tree; only the root filesystem
image is regenerated.

#### EROFS lzma

With `BR2_TARGET_ROOTFS_EROFS_LZMA=y` Buildroot also runs `lzma -9` over
`rootfs.erofs` (its generic image compression is keyed on the same symbol),
writing a second image larger than the first and costing about 35 s per build.
Gaia rewrites that choice to the `custom` one with `lzma,<level>` (the level
from the config, default 6) before `olddefconfig`, so `mkfs.erofs` gets the
same `-zlzma,<level>` argument and the extra file is not written. The settings
step reports `erofs: lzma via custom compression ...`. Other filesystems are
not changed: ext2, cpio, tar and ubifs keep Buildroot's generic compression on
purpose.

Retry strategies:
- `fixed`
- `exponential`

### Java and Gradle

`[providers.java] gradle_home` chooses where Gradle's user home (its
dependency and wrapper caches) lives for Java artifacts built in Docker:

- `"workspace"` (default): `<workspace>/.gaia/docker-home/.gradle`, kept per
  workspace.
- `"user-cache"`: `<user cache root>/gradle-home`, in the per-user Gaia cache
  (`$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`) and
  shared by every workspace of the user. Gaia mounts it into the container at
  the same path.

In `user-cache` mode, Docker builds also get a persistent `HOME`
(`<user cache root>/java-home`, mounted at the same path), so the caches of
tools the build runs under `HOME` (pnpm's store, npm, Python virtualenvs)
survive the `--rm` container. A build whose `build_env` sets its own
`GRADLE_USER_HOME` or `HOME` outside `.gaia/docker-home` keeps it; one that
sets none, or one inside `.gaia/docker-home`, gets the user-cache paths. Host
builds keep their own homes. The first build in `user-cache` mode is cold:
Gradle and the other tools download their caches again.

```toml
[providers.java]
gradle_home = "user-cache"
```

Deprecated: the environment variable `GAIA_GRADLE_HOME=workspace|user-cache`
still overrides `gradle_home` when it is set, so existing setups keep working
during the transition. Any other value fails the build. Set
`[providers.java] gradle_home` (or `--set policy.providers.java.gradle_home=...`)
instead.

## Provenance

```toml
[provenance.identity]
project = "gaia-image-builder"
vendor = "Example Vendor"
channel = "release"
labels = [
  ["branch", "${build.branch}"],
]
```

## Workspace

```toml
[workspace]
root_dir = "."
build_dir = ".gaia/build/my-build"
out_dir = ".gaia/out/my-build"

[[workspace.named_paths]]
alias = "assets"
path = "assets"
kind = "host"

[[workspace.named_paths]]
alias = "generated"
path = "generated"
kind = "logical"
```

Fields:
- `root_dir`
  Logical repo/workspace root for path resolution.
- `build_dir`
  Mutable build workspace.
- `out_dir`
  Mutable published output location.
- `export_dir`
  Optional default directory for `gaia run --export` in every build of the
  workspace. A build's own `image.output.export_dir` takes precedence. See
  [Image output](configuration-image.md#image) and [`gaia run --export`](cli.md#exporting-the-image).

Named path kinds:
- `host`
  Real host filesystem path.
- `logical`
  Logical alias used as a semantic reference.

## Clean

```toml
[clean]
default = "dist"

[clean.profiles.dist]
description = "Remove generated build outputs and package cache"
build = true
out = true
paths = [
  ".cache/gaia",
  "@generated",
]

[clean.profiles.outputs]
out = true
```

Fields:
- `default`
  Optional profile used by `gaia clean --target configured` and by bare
  `gaia clean` when no explicit target or path is passed.
- `clean.profiles.<name>.description`
  Optional human-facing profile description.
- `clean.profiles.<name>.build`
  Include `workspace.build_dir`.
- `clean.profiles.<name>.out`
  Include `workspace.out_dir`.
- `clean.profiles.<name>.paths`
  Additional paths to remove. Paths use the same workspace resolution as other
  Gaia paths, including `@alias/...`.

Bare `gaia clean` removes `workspace.build_dir` and `workspace.out_dir` when no
default clean profile is configured.
