# Configuration: Artifacts, install and stage

[Configuration index](configuration.md)

## Artifacts

```toml
[[artifacts]]
id = "acme-api"
kind = "rust"
package = "acme-api"
source = "workspace-root"
profile = "${build.profile}"
dependencies = ["acme-common"]
install_name = "acme-api"
install_class = "binary"
install_dest_hint = "/usr/bin/acme-api"
output_path = "${outputs.dir}/artifacts/acme-api"

[[artifacts]]
id = "worker-node"
kind = "java"
build_target = "build/libs/worker-node.jar"
source = "workspace-root"
output_path = "${outputs.dir}/artifacts/worker-node.jar"

[[artifacts]]
id = "camera-jni"
kind = "java"
build_target = "build/libs/camera-jni-linuxarm64.jar"
build_command = ["tools/build_arm64_jni.sh"]
build_env = [
  ["SYSROOT_DIR", "${workspace.build_dir}/image/buildroot-output/host/aarch64-buildroot-linux-gnu/sysroot"],
  ["MAVEN_LOCAL_REPO", "${workspace.build_dir}/m2"],
]
after_image_prepare = true
source = "camera-jni-source"
output_path = "${outputs.dir}/artifacts/camera-jni-linuxarm64.jar"

[[artifacts]]
id = "frontend-package"
kind = "node"
package_dir = "frontend"
source = "workspace-root"
output_path = "${outputs.dir}/artifacts/frontend.tgz"

[[artifacts]]
id = "python-wheel"
kind = "python"
package_dir = "sdk/python"
source = "workspace-root"
output_path = "${outputs.dir}/artifacts/sdk.whl"

[[artifacts]]
id = "acmectl"
kind = "go"
package = "./cmd/acmectl"
source = "workspace-root"
output_path = "${outputs.dir}/artifacts/acmectl"
```

Artifact common fields:
- `id`
- `kind`
- `source`
- `profile`
- `dependencies`
- `sources`
- `after_image_prepare`
- `install_name`
- `install_class`
- `install_dest_hint`
- `output_path`

Artifact kinds:
- `rust`
  - `package`
  - `target_name`
  - `emit_directory`
  - `features` (list, passed as `--features a,b`; supports interpolation)
  - `no_default_features` (bool, passed as `--no-default-features`)
  - `all_features` (bool, passed as `--all-features`; cannot be combined with
    `features` or `no_default_features`)
  - `build_env` (list of `[KEY, value]` pairs set for cargo, in order; values
    take config tokens and `${secret:NAME}`, read when cargo starts and passed
    to a docker build as `-e KEY` with the value kept off the command line).
    Gaia's own variables for the toolchain, sccache and offline mode win over
    these, except `CARGO_TARGET_<TRIPLE>_RUSTFLAGS`: a value here is kept, and
    the toolchain's and the `[cpu]` profile's flags are appended to it. `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` are refused for an
    artifact with a `toolchain` or a `[cpu]` profile: cargo would let them
    replace the toolchain's or the profile's flags.
  - `cpu` (bool, default true): `false` builds the artifact for its target's
    generic CPU whatever the build's `[cpu]` profile says (see
    [CPU profiles](configuration-sources.md#cpu-profiles)).

    ```toml
    [secrets.git_token]
    env = "GIT_TOKEN"

    [[artifacts]]
    id = "helios-api"
    kind = "rust"
    package = "helios-api"
    output_path = "out/helios-api"
    build_env = [
      ["CARGO_NET_GIT_FETCH_WITH_CLI", "true"],
      ["GIT_CONFIG_COUNT", "1"],
      ["GIT_CONFIG_KEY_0", "url.https://x-access-token:${secret:git_token}@github.com/.insteadOf"],
      ["GIT_CONFIG_VALUE_0", "https://github.com/"],
    ]
    ```

  - `toolchain` (string; the `[[toolchains]]` id to cross-compile with; see
    [Toolchains](configuration-sources.md#toolchains))
  - `build_group` (string; see [Rust build groups](#rust-build-groups))
  - `cargo_git_sources` (table: repository URL as Cargo.lock records it →
    the id of a declared `kind = "git"` source; see
    [Private cargo git dependencies](#private-cargo-git-dependencies))

  Feature flags are part of the artifact's identity: changing them rebuilds the
  artifact, and non-default flags are recorded in its backend state. Only
  artifacts with identical flags are batched into one cargo invocation.

  ```toml
  [[artifacts]]
  id = "worker-node"
  kind = "rust"
  package = "worker-node"
  no_default_features = true
  features = ["metrics"]
  output_path = "out/worker-node"
  ```

  Gaia collects `target/<triple>/<profile>/<target_name>` from the cargo
  target directory; `target_name` defaults to the file name of
  `output_path`. For a `cdylib` (or other library) set it to the file cargo
  writes, for example `target_name = "libvision_plugin.so"` for a package
  `vision-plugin`.
- `java`
  - `build_target`
  - `build_args`
  - `build_command`
  - `build_env`
- `node`
  - `package_dir`
- `python`
  - `package_dir`
- `go`
  - `package`

`sources = ["corelib"]` names sources the artifact reads besides its own
`source`. The artifact depends on each of them, so when one re-materializes
with changed content the artifact rebuilds, and the rebuild reason names the
source. A `${source.<id>.path}` or `${source.<id>.commit}` token in a Java
artifact's build fields adds its source automatically (the artifact depends on
it without listing it), so `sources` is for a source the artifact reads some
other way, such as files it opens inside that source. Each name must be a
declared source.

#### Rust build groups

Rust artifacts that name the same `build_group` are always built by one cargo
invocation, so cargo resolves features once for all of them. Use this when
binaries and plugins must agree on the features of shared crates, for example
an engine and the `cdylib` plugins it loads after checking an ABI or feature
fingerprint: separate `cargo build -p` runs can enable different features of
a shared dependency and produce incompatible builds.

For every member, Gaia runs `cargo build -p <each member package>` with the
sorted union of all members' `features`. The command is identical for each
member, whether the members are built together or one at a time (the second
build is then an incremental no-op), and members always share one batched
build without `[providers.rust] batch_builds`. Each member still collects its
own output (`package`, `target_name`, `output_path`) and has its own install
identity.

Members must share `source`, `target`, `profile`, `execution`,
`no_default_features`/`all_features`, `build_env` and their `[cpu]` profile; validation reports the conflicting
members otherwise. `build_group` on a non-rust artifact is an error. A group
of one is allowed. Since one `--features` list serves every selected
package, prefer `package/feature` names so each feature is enabled on the
package that defines it.

```toml
[[artifacts]]
id = "acme-engine"
kind = "rust"
source = "acme"
package = "acme-engine"
profile = "release"
features = ["acme-engine/plugins"]
build_group = "acme"
install_name = "acme-engine"
install_dest_hint = "/usr/bin/acme-engine"
output_path = "${outputs.dir}/artifacts/acme-engine"

[[artifacts]]
id = "vision-plugin"
kind = "rust"
source = "acme"
package = "vision-plugin"          # [lib] crate-type = ["cdylib"]
target_name = "libvision_plugin.so"
profile = "release"
features = ["vision-plugin/simd"]
build_group = "acme"
install_name = "libvision_plugin.so"
install_class = "library"
install_dest_hint = "/usr/lib/acme/plugins/libvision_plugin.so"
output_path = "${outputs.dir}/artifacts/libvision_plugin.so"

[[install]]
id = "install-acme-engine"
artifact = "acme-engine"
dest = "/usr/bin/acme-engine"
mode = 493

[[install]]
id = "install-vision-plugin"
artifact = "vision-plugin"
dest = "/usr/lib/acme/plugins/libvision_plugin.so"
mode = 420
```

Both artifacts build with `cargo build -p acme-engine -p vision-plugin
--features acme-engine/plugins,vision-plugin/simd --release`.

#### Shared Rust libraries (experiment)

This is for measuring, not for products. Calls across a Rust `dylib` stay out
of reach of LTO: in HeliOS's A/B on a CM5 (2026-11-04) a per-frame graph ran
26% slower per tick than the fully static build. Products share a dependency
as a `cdylib` with a small C ABI instead, the dependency's core linked into it
with LTO.

`link = "shared"` on a rust artifact links the standard library, and the
crates a Rust `dylib` of the same build provides, dynamically: Gaia adds
`-C prefer-dynamic` to the rustc flags cargo reads for the artifact's target
(`CARGO_TARGET_<TRIPLE>_RUSTFLAGS`, or `RUSTFLAGS` for a host build), after the
project's own and the `[cpu]` profile's. `link = "static"` is the default. The
value can come from an input, so one build file makes either kind of image:
`link = "${input.link_mode}"`. Every member of a build group that builds crates
must link the same way (`rust_build_group_link_mismatch`). Rustc refuses LTO
with `prefer-dynamic`, so a shared build's cargo profile sets `lto = false`.

The artifacts then need the toolchain's `libstd-<hash>.so` at run time. An
artifact with `package = "std"` is that library: no cargo package (cargo
refuses `std` as a package name), but a directory holding the target's
`libstd-*.so`, read from the rustc that builds the source (its
`rust-toolchain.toml`, in the artifact's `execution` backend), so the hash is
the one the other artifacts linked against. In a build group it is compared
with the other members only on `source`, `target`, `profile` and `execution`,
and it needs a member with `link = "shared"` (`rust_std_without_shared_link`).
Install it at the library directory; the directory's files are added to it.

```toml
[[artifacts]]
id = "rust-std"
kind = "rust"
source = "acme"
package = "std"
profile = "release"
build_group = "acme"
output_path = "${outputs.dir}/artifacts/rust-std"

[[install]]
id = "install-rust-std"
artifact = "rust-std"
dest = "/usr/lib"
```

#### Declared surfaces

A `cdylib` that other programs link against (a product's surface library,
called through a versioned C ABI) can declare what it exports. After the
build, its dynamic exports must be exactly the declared names: an undeclared
export (a Rust function that leaked) or a declared one that is missing fails
the artifact, naming them. The library's build decides what it exports (rustc
exports a `cdylib`'s `#[no_mangle] extern "C"` items; a version script can
narrow it further); Gaia only checks the result.

```toml
[[artifacts]]
id = "libsurface"
kind = "rust"
source = "acme"
package = "acme-surface"
target_name = "libacme_surface.so"
output_path = "${outputs.dir}/artifacts/libacme_surface.so"
surface = { exports = "surface/acme_surface.map" }
```

`exports` is relative to the artifact's source and may come from an input.
It is either one symbol per line (`#` starts a comment) or the library's own
linker version script, whose `global:` names are the surface (version node
names are not symbols). Wildcards are refused: a surface names each symbol.
Absolute symbols (the version nodes) and rustc's `rust_metadata_*` are not
part of the comparison. The artifact must be one library file, not
`emit_directory` or `package = "std"` (`rust_surface_not_a_library`).

The image checks then read the root filesystem: every program or library
there that needs an installed surface library (by file name or `SONAME`) may
import only what the surface, and the other libraries it loads, export. A
program that imports more was linked against another build of the surface
(`surface_import_undeclared`, an error). For a library it is a warning, since
the program loading it may define the rest; a consumer whose other libraries
are not in the image is not checked (`surface_consumer_unchecked`). This runs
with or without a kernel in the build, and for a starting-point image too (on
its `<collect_dir>/rootfs`): a build that installs a surface plans
`image:check` even with `[image.checks] kernel_modules = false`. The report
(`<out>/<build>.checks.json`) counts the consumers in `surface_consumers`.

A surface's process-wide state (its handle tables) may be in a program
once: a program links it in, or loads the library, not both. Handles from
one copy are unknown to the other.

```toml
[[artifacts]]
id = "lemnos-capi"
kind = "rust"
source = "lemnos"
package = "lemnos-capi"
target_name = "liblemnos_capi.so"
output_path = "${outputs.dir}/artifacts/liblemnos_capi.so"
surface = { exports = "crates/lemnos-capi/lemnos.exports", shared_feature = "shared" }
```

- `state_crates`: the crates that hold the state, default the artifact's
  own package.
- `shared_feature`: the package's feature with which its Rust wrapper calls
  the installed library instead of linking the surface in (Lemnos's
  `shared`). A program that has the package with this feature on holds no
  copy of it; another state crate counts whatever its features.

With a surface in the build, every cargo build records its crate graph
(`cargo tree -e normal`, in the artifact's backend: each crate and the
features cargo resolved) next to its output, in
`.gaia/<file>.crates.json`. The image checks then fail a program whose
record has a state crate and whose `DT_NEEDED` closure loads the surface
(`surface_state_twice`, naming the crates and the feature to turn on). A
program without a record (built before the build declared a surface) is a
`surface_crates_unrecorded` warning; `--rebuild artifact:<id>` records it.

#### Private cargo git dependencies

A rust artifact whose workspace depends on a private git repository (`git =
"https://github.com/org/private.git"` in `Cargo.toml`) needs that repository's
credentials when cargo fetches it, and a docker container has none. Map the
dependency to a declared git source instead, and Gaia serves it from that
source's mirror, prepared on the host with your own git credentials:

```toml
[[sources]]
id = "corelib"
kind = "git"
repo = "https://example.com/org/Corelib.git"
branch = "main"

[[artifacts]]
id = "vision"
kind = "rust"
source = "workspace"
package = "vision"
cargo_git_sources = { "https://example.com/org/Corelib.git" = "corelib" }
output_path = "${outputs.dir}/artifacts/vision"
```

- The key is the repository URL as `Cargo.lock` records it. It also matches
  without `.git`, with a trailing `/`, and with the `git+` prefix or a `?branch=`
  query that the lock adds. The value is the id of a declared `kind = "git"` source.
- The artifact depends on each named source, so a change to it rebuilds the
  artifact. The source's branch or tag does not decide which commits build: the
  lock does. Cargo.toml and Cargo.lock are not changed.
- Before cargo runs, on the host: every commit the workspace's `Cargo.lock`
  pins for the mapped repository must be in the source's mirror under
  `.gaia/cache/git`. A commit the mirror lacks is fetched; if it still cannot be
  found the build fails and names the URL, the locked commit and the source.
  The commit is then kept reachable from `refs/heads/gaia-locked/<commit>` in the
  mirror, because cargo's git fetch only asks for branches. The mirror's refs are
  rewritten on every build.
- Cargo runs with `CARGO_NET_GIT_FETCH_WITH_CLI=true` and git URL rewriting
  (`url.<mirror>.insteadOf`), so it reads the dependency from the mirror, never
  from the network. In docker the mirror is mounted read-only at its own path,
  and the execution image needs `git` (the official rust images have it).
  Entries already in `GIT_CONFIG_COUNT` are kept.
- Each mapping must name a `kind = "git"` source. Two keys naming the same
  repository for different sources, or `cargo_git_sources` on a non-rust
  artifact, are validation errors.
- Rewrites match by prefix, so a locked repository whose URL starts with a
  mapped one (`https://h/a` and `https://h/a-core`) is a build error unless it
  is mapped too.

Java artifacts run Maven or Gradle automatically when `build_command` is omitted. Use `build_args` to replace the default Maven/Gradle arguments while keeping tool detection, or use `build_command` for a source-local command such as a wrapper script. `build_env` adds environment variables for either mode. `after_image_prepare = true` schedules the artifact after Buildroot prepare, which is useful when the artifact needs the generated Buildroot sysroot before the final image feed is assembled.

### Artifact Execution

Artifacts run on the host unless `[execution.docker]` or the artifact's own
`execution` table selects Docker:

```toml
[[artifacts]]
id = "acme-engine"
kind = "rust"
package = "acme-engine"
source = "workspace-root"
output_path = "${outputs.dir}/artifacts/acme-engine"

[artifacts.execution]
backend = "docker"          # or "host"

[artifacts.execution.docker]
# Either reference an existing image ...
image = "acme-cross-rust194"
# ... or let Gaia build it from a Dockerfile (paths are workspace paths,
# `${alias}/...` works).
dockerfile = "docker/cross-rust/Dockerfile"
context = "docker/cross-rust"   # optional, defaults to the Dockerfile's directory
```

With `dockerfile`, Gaia hashes the Dockerfile and every file in the context
(skipping `.git`, `.gaia`, `target` and `node_modules`) and runs the artifact
in `gaia-local/<name>:<hash>`, where `<name>` is the repository part of
`image` when set, otherwise the artifact id. Before the artifact builds, Gaia
checks for that tag with `docker image inspect` and runs
`docker build -f <dockerfile> -t <tag> <context>` only when it is missing. The
hash is part of the artifact fingerprint, so editing the Dockerfile or its
context rebuilds the image and the artifact. The artifact state records
`execution_backend_image` (the tag), `execution_backend_image_id`,
`execution_backend_image_hash` and `execution_backend_image_dockerfile`. Keep
the context small: it is hashed at every plan. A clean machine only needs
Docker and the repository to build the image.

Reuse fingerprints identify the toolchain an artifact was built with. Host
artifacts hash the host tool versions (`cargo`/`rustc`, `go`, `python3`,
`npm`/`node`, `mvn`/`gradle`). Docker artifacts never probe host tools (so a
host without Maven or Gradle no longer logs failed probes); they hash the
execution image instead:

- a Dockerfile-built image contributes its content hash, without calling
  Docker;
- a plain `image` (or `[execution.docker] image`) contributes its image id
  from `docker image inspect --format '{{.Id}}'`, probed once per process, so
  pulling or rebuilding a different image under the same tag rebuilds the
  artifact;
- an image that is not present locally contributes `image-missing:<tag>`.
  Planning does not fail; the first run after the image is pulled rebuilds the
  artifact once.

Moving to these signatures changes the fingerprint of every docker-backed
artifact once, so they rebuild on the first run after upgrading.

Install classes:
- `binary`
- `library`
- `archive`
- `config`
- `service`
- `data`

## Install

```toml
[[install]]
id = "install-acme-api"
artifact = "acme-api"
dest = "/usr/bin/acme-api"
replace = true
mode = 493
owner = "root"
group = "root"
```

## Stage

```toml
[[stage.files]]
id = "motd"
src = "${assets}/etc/motd"
dest = "/etc/motd"
origin = "static-asset"

[[stage.env_sets]]
id = "runtime-env"
name = "runtime"
entries = [
  ["GAIA_MODE", "release"],
  ["ACME_TARGET", "${build.target}"],
]

[[stage.services]]
id = "acme-api"
name = "acme-api.service"
unit_path = "${assets}/systemd/acme-api.service"
```

Stage file origins:
- `static-asset`
- `generated`
- `provider-emitted`

A stage file takes `src` (copied) or `content` (text written to `dest`), not
both. Config tokens in `content` are rendered, so a file can carry the build's
own version:

```toml
[[stage.files]]
id = "os-release"
content = """
NAME="Acme"
VERSION_ID="${build.version}"
IMAGE_VERSION="${build.version}"
"""
dest = "/etc/os-release"
mode = 0o644
```

The text is written to `<build_dir>/stage-content/<id>` and fed like a copied
file; it is part of the entry's fingerprint, so a new version restages it. A
patch that sets `content` replaces the entry's `src`, and the reverse.
`content` declares a new entry only together with `dest`; without `dest` it
patches the earlier entry with that id.
