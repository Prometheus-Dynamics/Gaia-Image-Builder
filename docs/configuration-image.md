# Configuration: Image

[Configuration index](configuration.md)

## Image

### Buildroot

```toml
[image]
kind = "buildroot"
defconfig = "raspberrypi_defconfig"
external_tree = "@assets/buildroot"
external_tree_mode = "required"

[image.feed]
install_entries = ["install-acme-api"]
stage_files = ["motd"]
stage_env_sets = ["runtime-env"]
stage_services = ["acme-api"]

[[image.expected_images]]
name = "sdcard.img"
format = "raw"
required = true

[image.output]
collect_dir = "${workspace.out_dir}/images"
archive_name = "${build.name}-${build.version}.img.xz"
export_dir = "~/images/gaia"
emit_report = true
```

`export_dir` is where a successful `gaia run` copies the primary image (see
[Exporting the image](cli.md#exporting-the-image)). It is set per build here
and falls back to `[workspace] export_dir`. A path that is not absolute and
does not start with `~/` is taken from the workspace root; `~` and `~/...`
expand to the home directory. An empty value is an error. `gaia run --export
<dir>` overrides both, and `gaia run --no-export` skips the configured
export for one run. The `--set` keys are `image.output.export_dir` and
`workspace.export_dir`.

An `archive_name` ending in `.img`/`.raw` publishes the raw disk image
itself; `.img.xz`/`.raw.xz` compresses it with xz (smallest, slow) and
`.img.zst`/`.raw.zst` with zstd (several times faster to compress and
decompress, a little larger). Both use every core and give the same bytes
for any core count. A common split is zstd for development images and xz
for releases, for example
`archive_name = "${build.name}-${build.version}.img.${input.compression}"`.

A compressed raw archive is skipped when its image and compressor settings are
unchanged since the last build (the image's digest, from the content manifest,
and the compressor and its arguments). A Buildroot build's archive name must
end in `.tar` or `.txz` to hold the expected images, or name one raw disk
image the build produces. Any other name is an error that names the expected
images found: Gaia does not tar the whole Buildroot output to fill it. A raw
disk archive that typed assembly builds is written by that step, so the path
stays absent until then.

When the assembly builds more than one disk, a raw disk archive needs
`disk = "<assembly disk id>"` in `[image.output]` to say which disk it holds
(the id must be an `[[image.assembly.disks]]` entry, and the archive name must
be a raw disk image). Without it the run fails and names the disks:

```toml
[image.output]
archive_name = "${build.name}-${build.version}.img.xz"
disk = "sdcard"
```

The run reports every image it publishes (the `image sizes` table and the
summary's `published_outputs`): the primary, the other disks written under the
image output dir, published filesystem images, and archives and transform
outputs written there. `gaia run --export` (see
[Exporting the image](cli.md#exporting-the-image)) copies the primary and those
outputs, except uncompressed raw disks other than the primary, which the
primary archive already holds; compress such a disk with an `xz` transform to
export it.

Buildroot fields:
- `defconfig`
- `external_tree`: one or more `BR2_EXTERNAL` trees, `:`-separated as
  Buildroot expects (`"@source:bsp/devices/myboard/gaia/buildroot-external:myboard/assets/buildroot"`).
  Each entry is resolved on its own: `@self`, `@source:<id>` and `@alias`
  tokens may start any entry, and plain relative entries resolve against the
  workspace root. Every tree needs its own `external.desc` with a unique
  `name`. Gaia's generated package-override tree (`GAIA_GENERATED`) is
  appended automatically, so do not reuse that name. A later layer's
  `external_tree` replaces an earlier one rather than appending, so a layer
  that adds a tree must list the full combination.
- `external_tree_mode = "auto" | "required" | "disabled"`
- `expected_images[]`
- `allow_fallback = true`: when no Buildroot tree is available to build in,
  assemble the rootfs from the image feed alone (into `<collect dir>/rootfs`)
  instead of failing. For dry runs and tests: the result has no Buildroot
  packages, and the run notes say the fallback was used. Off by default.
- `legal_info = true` (under `[image]`): after the build, runs `make legal-info`
  and publishes `legal-info.tar.zst` (the license and manifest files of every
  package) in the collect dir. Needs `tar` with zstd support. Off by default;
  see "Package sizes and legal info" below.
- A defconfig's support files: a file that the defconfig or a config fragment
  names by a relative path (`-c genimage.cfg`, `overlays/config.txt`) is copied
  into the output dir, where Buildroot resolves it. Nothing else beside the
  defconfig is copied, so a defconfig at a project root does not copy the
  project. A name that is a Buildroot output entry (`Makefile`, `target`,
  `build`, `host`, `images`, ...) is never copied over it.

Buildroot checks:
- `[providers.buildroot] override_check = "error" | "warn" | "off"` (default
  `"error"`): fails before `make` when `olddefconfig` dropped or changed a
  requested `config_overrides` entry.
- `[providers.buildroot] kernel_modules_check = "error" | "warn" | "off"`
  (default `"error"`): after `make`, fails when `target/lib/modules` holds
  fewer kernel modules than the kernel build's `modules.order` lists.
  Buildroot's `linux.mk` does not fail when `modules_install` stops part way,
  so without this an image can ship with most modules missing. Use `"warn"`
  if a post-build script removes modules on purpose.

Source commits: `${source.<id>.commit}` in stage env set values and build
labels becomes the exact commit the source builds from: a git source's
`gaia lock` commit or full-sha `rev`, an import source's checkout commit, or a
path source's `git rev-parse HEAD` (with `-dirty` for uncommitted tracked
changes). Use it to record versions in the image, for example:

```toml
[[stage.env_sets]]
id = "image-version"
name = "image-version"
entries = [
  ["DEVICE_PACKAGE_COMMIT", "${source.bsp.commit}"],
  ["WORKER_COMMIT", "${source.worker.commit}"],
]
```

A git source that follows a branch or tag without a lock has no exact commit
before it is fetched, so its token is reported as unresolved: pin it with
`rev` or `gaia lock`.

`${source.<id>.commit_short}` is the first 8 characters of that commit; a
`-dirty` suffix is kept, so a short token still shows an uncommitted tree.

Project identity: `${project.commit}` and `${project.describe}` describe the
git repository holding the build file. `${project.commit}` is the full `HEAD`
sha, suffixed `-dirty` when tracked files have uncommitted changes, and
`${project.describe}` is `git describe --tags --always --dirty`. They resolve
during normal interpolation, so they work anywhere `${build.version}` does,
including `version`, `archive_name`, env sets, archive manifest entries and
input values (a declared `default`, or a `--set input.<name>=` value), so
`default = "${project.commit_short}"` gives an input the build file's own
commit. A build file in a linked git worktree resolves to that worktree's
`HEAD`.
`${project.commit_count}` is `git rev-list --count HEAD`, which only grows along a branch: put it before the commit in a package version (`2026.2.0-dev.${project.commit_count}+${project.commit_short}`) so opkg and dpkg order dev builds. `${project.commit_short}` is the first 8 characters of `${project.commit}`,
with the `-dirty` suffix kept. Use them to make image versions unique per build:

```toml
version = "${input.app_version}+${project.describe}"
```

Outside a git repository they stay unresolved and validation reports them.
Anything that embeds them changes with every commit. For example, putting
them in `version` gives artifacts a new build context on every commit, so
they rebuild each time. To keep artifact reuse, put the commit only into the
image's version records (env sets, os-release files, the update bundle
manifest) rather than `version`.

Source paths: `${source.<id>.path}` becomes the directory holding the
source's files: an import source's checkout (under
`.gaia/cache/import-sources/`), a path source's directory, or
`<build_dir>/sources/<id>` for git, archive and download sources (where Gaia
materializes them; with build variants, `<build_dir>/sources/variants/<variant>/<id>`
for a source a variant checks out for itself). Like the commit token it works
in stage env set values and build labels; both tokens also work in Java
artifacts' `build_target`, `build_args`, `build_command` and `build_env`
(for example `build_env = [["CORELIB_PATH", "${source.corelib.path}"]]`) and in
Buildroot `config_overrides` values, so a setting can name a file inside a
source:

```toml
[image]
kind = "buildroot"
config_overrides = [
  ["BR2_ROOTFS_USERS_TABLES", "\"${source.worker.path}/packaging/buildroot/worker-users.table\""],
]
```

Inside `[image.assembly]` every string takes both source tokens: tree ids
and tree or file paths, file `src`, `dest` and `content`, `json` values (keys
are not substituted), archive outputs and members, disk and filesystem
outputs, and the rest. A token there resolves at config time like the others,
so a slot id can carry the commit:

```toml
[[image.assembly.trees]]
id = "slot-${source.app.commit_short}"
path = "$assembly.work/slot-${source.app.commit_short}"

[[image.assembly.files]]
tree = "slot-${source.app.commit_short}"
json = { rev = "${source.app.commit}" }
dest = "slot.json"
```

A `${source.<id>...}` token that stays unresolved there (an unknown source id,
or a git source with no commit yet) is reported by validation, like any other
unresolved token. Each declared source the assembly names is a dependency of
its `image:assembly` step: a changed file inside a path source (or a new
commit of a git source) reruns the assembly, and an unchanged source keeps it
reused.

When an override value names a source's directory, the image operations
depend on that source, so it is materialized first. `BR2_ROOTFS_USERS_TABLES`,
`BR2_ROOTFS_DEVICE_TABLE` and `BR2_ROOTFS_STATIC_DEVICE_TABLE` are applied
when rootfs images are generated on every `make`, so changing them (including
an import source's checkout path changing with its `rev`) does not force a
full Buildroot clean.

Expected image formats:
- `tar`
- `cpio`
- `ext2`
- `ext3`
- `ext4`
- `ubifs`
- `ubi`
- `jffs2`
- `romfs`
- `cramfs`
- `cloop`
- `f2fs`
- `btrfs`
- `squashfs`
- `raw`
- `kernel`
- `erofs`
- `xfs` (Buildroot 2026.08 or newer, `BR2_TARGET_ROOTFS_XFS`)
- `file`: any plain file Buildroot writes. The name is not checked against a
  format (`flash-id` is accepted as is), and the defconfig need not enable a
  filesystem for it. Use it for images that Buildroot's post-image scripts
  produce under a name without a format suffix.

Buildroot run notes:
- Config steps are reused. The build operation repeats the prepare operation's
  defconfig, fragments, `config_overrides` and cache settings only when their
  inputs changed: the defconfig and fragment files, the overrides, the cache
  settings Gaia writes, the Buildroot version (`Makefile`) and every Kconfig
  file of the Buildroot and `BR2_EXTERNAL` trees (by content). The state is
  `.gaia-buildroot-config-inputs` in the output tree; a `.config` edited
  outside Gaia is rebuilt. The run message is `buildroot config unchanged
  since the last configuration; config steps skipped`.
- `target-finalize` is reused. Each `make` that reaches it copies every
  package's per-package tree into `target/` and patches the ELF files of
  `host/` (about 40 s on a large application tree, with nothing to build). After a
  prepare (or build) finalize, the build operation skips it when every
  package is installed, the config is unchanged and nothing has been built
  since; the images step then runs as before. The marker
  `.gaia-target-finalized` is removed when a make may build, and when the
  image feed is applied to `target/`. The run message is `skipped
  target-finalize: ...`.
- `SOURCE_DATE_EPOCH` is set for Buildroot's `make` and its post-build and
  post-image scripts: the caller's own `SOURCE_DATE_EPOCH` when the
  environment sets one, else the commit time of the workspace git `HEAD`
  (stable for a commit, changes with each commit; nothing is set outside a
  git workspace). It is not part of the package cache keys, so a package
  restored from the cache keeps the timestamps of the epoch it was built
  with until it is rebuilt.
- The package cache reports installed packages it cannot key (for example
  packages with local sources) instead of leaving them out silently.

#### Leaving packages out of the rootfs

```toml
[image.rootfs]
exclude_packages = ["libcamera", "libpisp", "opencv4"]   # Buildroot package names
```

One Buildroot tree can build every package while an image leaves some of them
out of its root filesystem, without a second tree. `image:build` removes the
files of the listed packages from `target/` before the image's post-build
scripts and before the filesystem images, so `rootfs.*` does not contain them.

#### Package sizes and legal info

Every Buildroot image build writes `<build>.package-sizes.json` into the collect
dir: the bytes each package installs into `target/` (its regular files, from
`build/packages-file-list.txt`), largest first, with the total. The run notes
name the ten largest packages and their change since the previous build of the
same name (`+2.0 KiB`, `unchanged`, `new`). Files a package puts in `images/`
are not counted.

```toml
[image]
kind = "buildroot"
legal_info = true   # publish legal-info.tar.zst after the build
```

With `legal_info = true`, `make legal-info` runs after a successful build and
its `legal-info/` directory (`manifest.csv`, `host-manifest.csv`, the license
files) is archived as `legal-info.tar.zst` in the collect dir. The archive is
kept, not made again, while the package list is unchanged; it is recorded in
the output tree. `make legal-info` may fetch sources that are not downloaded
yet, so run it on a machine with network access.
Names are lowercase Buildroot package names; a name listed twice is an error.
`--set image.rootfs.exclude_packages=libcamera,libpisp` sets the list (a
variant can set it the same way). Layered configs concatenate their lists.
The image:prepare operation does not apply it.

Checks, before anything is removed:
- each name is a target package of the Buildroot tree: an unknown name and a
  `host-*` package are errors;
- no kept target package depends on an excluded one, directly or through
  other packages. The error names the chain, for example `app -> libcamera ->
  libpisp`. `skeleton` and the toolchain are not counted.

After the make, before the images:
- every removed path is gone, and no object left in `target/` needs a shared
  object name that only a removed file provides. The check reads each ELF
  object's `DT_NEEDED` and `DT_SONAME`.

Run notes:
- Only the files `build/packages-file-list.txt` lists for an excluded package
  are removed. A path a kept package also lists stays and is reported.
  Directories go when the removal empties them.
- The exclusion needs `policy.providers.buildroot.parallel_packages` (per-package
  directories): without them Buildroot copies the files back into `target/`
  on every finalize, and the build refuses. It is refused with shared
  Buildroot output, which one tree serves to every image.
- The checks do not see runtime loads that are not `DT_NEEDED` entries, such as
  a library opened by path with `dlopen` or a plugin directory. Leave those
  packages in, or remove what loads them too.
- The run message is `rootfs: left out N packages (...): M files, X MiB`;
  `gaia preview` shows the names, the checks on the last build's graph, and the
  files that build lists for them.

### Starting Point

```toml
[image]
kind = "starting-point"
rootfs_path = "${workspace.build_dir}/seed-rootfs"
rootfs_validation_mode = "require-directory"
output_mode = "copy-and-archive"
```

The rootfs is copied into the build tree as it is. Symlinks are copied as
links and never followed, so an absolute link into the host (such as
`etc/resolv.conf -> /run/systemd/resolve/stub-resolv.conf`) stays a link and
no host file is copied in. File and directory modes are kept, and hardlinks
inside the tree stay hardlinks. Device nodes and fifos are recreated only
when gaia runs as root; otherwise the build fails and names the file.
Ownership is not carried over.

Starting-point fields:
- `rootfs_path`
- `rootfs_validation_mode`
- `output_mode`
- `source` / `source_path`: take the starting point from a source instead of
  `rootfs_path`: the source's checkout, or `source_path` inside it. The
  starting point may be a rootfs directory or a raw disk image file; for a
  raw image Gaia mounts its rootfs partition, applies the feed and the
  package changes, and writes the image back.
- `image_partition`: the partition of a raw image that holds the rootfs, by
  device path or its tail (`p2`, `sda2`). Unset: the largest partition that is
  not vfat.
- `image_read_only` (default `true`): leave the raw image unchanged. Applying a
  feed with content or running package commands needs `image_read_only =
  false`, and mutating a raw image needs root.

#### Packages in a starting-point rootfs

`[image.packages]` reconciles the starting point's packages with its package
manager, run inside the rootfs with `chroot`:

```toml
[image.packages]
enabled = true
execute = true            # false: plan the commands and list them, run nothing
manager = "apt"           # apt, dnf, yum, apk or pacman; unset or "auto": detected from the rootfs
update = true             # refresh the package index first
dist_upgrade = false      # upgrade every package (apt-get dist-upgrade, dnf upgrade, ...)
install = ["curl"]
remove = ["nano"]
extra_args = ["--no-install-recommends"]   # added to the install, remove and upgrade commands
release_version = "24.04" # the release to install from; unset: VERSION_ID of os-release
allow_major_upgrade = false
os_release_path = "/etc/os-release"        # inside the rootfs
```

A `release_version` whose major version differs from the rootfs's
`VERSION_ID` is refused unless `allow_major_upgrade = true`.

Validation modes:
- `require-exists`
- `require-directory`
- `require-file`
- `allow-missing`

Output modes:
- `copy-rootfs`
- `archive-only`
- `copy-and-archive`

### None

With `kind = "none"` the feed defaults to empty. The defaults of a rootfs image
(every install and stage entry is fed to the image) do not apply, and setting
`[image.feed]` is a validation error. A layer that switches a build to `none`
replaces the Buildroot fields it set (see "Image kind switch", under "Profiles").

```toml
[image]
kind = "none"

[image.output]
collect_dir = "${workspace.out_dir}"
```

`kind = "none"` builds no rootfs, kernel or image, and no image provider runs.
The plan has no `image:prepare`, `image:build` or `image:check` operation. The
build is its sources, artifacts, installs and `[image.assembly]`: the assembly
makes the deliverable, typically an ipk package for an external device OS (see
"Packages (ipk, deb)" below). `examples/systemcore-package` is a complete build.

How the rest of the build fits:
- `[image.assembly]` is optional. Without it, the build makes its sources and
  artifacts, and the report comes last, after all of them.
- With an assembly, `image:assembly` takes the place of the image: it waits for
  every artifact, install and stage entry of the build, and for the sources its
  `${source.<id>...}` tokens name (there is no `image:build` to wait for).
- `$assembly.work`, `$assembly.out`, `$assembly.tree.<id>` and workspace paths
  work. A `$provider.*` token is a validation error, because there is no image
  provider.
- `[image.output]` `collect_dir` is the assembly's output directory unless
  `[image.assembly] out_dir` says otherwise. `archive_name` and `disk` name an
  assembly archive or disk as they do for a Buildroot image.
- `[image.feed]`, `[image.checks]`, `[[image.hardware]]` and `[image.layers]`
  are validation errors: each acts on a rootfs or kernel this build does not
  make. The feed's installs and stage entries reach an assembly tree through
  `[[image.assembly.files]]` instead.
- A checkpoint anchored at `image` (the default) anchors at the assembly, so it
  needs `[image.assembly]`; an anchor at a feed entry is an error here.

The primary output (the one the run summary names and `gaia run` exports) is
`image.output.archive_name`'s archive when it is set. Otherwise it is the first
existing assembly deliverable, in this order: the first
`[[image.assembly.packages]]` package, then the first archive, then the first
disk.

### Image Feed

The image feed declares which install/stage domains are part of the final image contract:
- `install_entries`
- `stage_files`
- `stage_env_sets`
- `stage_services`

If omitted, the current compiler auto-feeds all entries in the corresponding domain.

### Signing

`[image.signing]` writes a signature next to each published image output. It runs after a successful run, over the outputs the run published (reused images too), so a signature always matches its file.

```toml
[image.signing]
kind = "gpg"                           # "minisign" | "gpg" | "cosign-blob"
key = "env:GAIA_SIGNING_KEY"           # a key path, env:VAR naming a variable that holds it, or secret:NAME ([secrets])
outputs = ["*.img.xz", "*.slot.tar"]   # globs over the published output file names
```

- `minisign`: writes `<file>.minisig` with `minisign -S -s <key>`. The key must not need a password.
- `gpg`: writes `<file>.asc`, an armored detached signature, with `gpg --detach-sign --local-user <key>`. For gpg the key is a key id or user id, not a path.
- `cosign-blob`: writes `<file>.sig` with `cosign sign-blob --key <key>`.

A verity root hash file (a published output named `*.roothash`) is signed whether or not `outputs` names it. The signature files join the published outputs in the report. Gaia uses the tool from `PATH` and holds no key material; it never prints the key. A `key = "secret:NAME"` names a `[secrets]` entry, read when the signing starts; the value is then the key path (minisign, cosign-blob) or key id (gpg), and it is never stored or printed (see [Secrets](configuration.md#secrets)). The validator rejects an unknown kind, an empty key or an empty `outputs` before the build. A missing tool or key fails the run at the signing step, and the message names the variable or the tool, never the key.

### Image Layers

`[image.layers]` splits a Buildroot image's root filesystem into one tar per layer (for example `base`, `platform` and `app`), so each layer can be shipped, verified and reused on its own. It is off unless the table is set; without it the image is built exactly as before. Gaia forces `BR2_TARGET_ROOTFS_TAR=y` when it is set, and splits Buildroot's `rootfs.tar`, which carries the ownership Buildroot applied.

```toml
[image.layers]
order = ["base", "platform", "app"]   # lowest first
default = "base"                      # takes every path no selector claims
erofs = true                          # an erofs image per layer (default true)
verity = false                        # a dm-verity tree appended to each erofs (default false; needs erofs)
overrides = []                        # non-directory paths two layers select: the upper copy ships

[image.layers.platform]
packages = ["linux", "linux-ext-*", "rpi-firmware"]   # Buildroot packages: every path they install
paths = ["/lib/modules", "/lib/firmware"]             # rootfs path prefixes

[image.layers.app]
installs = ["helios-*"]     # install ids: the install's destination
stage = ["helios-*"]        # stage file ids: the paths each entry wrote
services = ["helios-*"]     # stage service ids: the unit file
```

- **Membership.** Each rootfs path goes to the layer of its highest-ranked selector that matches: `paths` first, then `stage`, `installs` and `services`, then `packages` (from `build/packages-file-list.txt`), then `default`. Ids and package names are globbed with `*`. Two layers with selectors of the same rank that claim one non-directory path are an error, unless the path is listed in `overrides`, where the upper layer's copy ships. Directories may repeat: a layer carries the directories above its members so it extracts on its own.
- **Contents.** Each layer tar keeps every entry's owner, mode and link. Its entries are in path order and carry a fixed mtime (`SOURCE_DATE_EPOCH`, or 0), so the same rootfs gives the same bytes. A hardlink whose target is in another layer becomes a copy of the target's data.
- **erofs options.** `erofs_options` on `[image.layers]` sets the `mkfs.erofs` options of every layer, and on `[image.layers.<name>]` replaces them for that layer. The default is `["-zlzma,level=6", "-C262144", "-Eall-fragments,dedupe,ztailpacking"]`. Example, large clusters for the layers that rarely change and smaller ones for the app layer, which is read fresh after every update:

  ```toml
  [image.layers]
  order = ["base", "platform", "app"]
  erofs_options = ["-zlzma,level=6", "-C1048576", "-Eall-fragments,dedupe,ztailpacking"]

  [image.layers.app]
  installs = ["helios-*"]
  erofs_options = ["-zlzma,level=6", "-C262144", "-Eall-fragments,dedupe,ztailpacking"]
  ```

  The options are part of each layer's fingerprint. Gaia sets the input, owners, times, xattrs and UUID itself, so `--tar`, `--all-root`, `-T`, `-U`, `-x`, `--uuid` and `--ignore-mtime` are rejected. Each layer's erofs is made from its tar unpacked under `fakeroot` (owners, modes and device nodes kept), not with `mkfs.erofs --tar`: with `-Eall-fragments`, erofs-utils 1.8.10 writes `--tar` images whose file data is lost while `fsck.erofs` still passes.
- **Outputs.** In the image's collect dir: `layers/<name>.tar`, `layers/<name>.erofs` (unless `erofs = false`; needs `mkfs.erofs`), and `layers/layers.json`. Each layer's record has `name`, `id` (the sha256 of the erofs file as shipped; the board stores layers by it, and it is null without erofs), `tar`, `tar_sha256` (the OCI diff id), `tar_size`, `erofs`, `erofs_sha256`, `erofs_size`, `size` and `sha256` (the same two values under the names Atlas's layered slot reads), `verity` and `inputs` (the layer's fingerprint). With `verity = true` the verity record has `root_hash`, `salt`, `hash_offset`, `data_blocks`, `data_block_size` and `hash_block_size`, named as the verity transform names them for `slot.json`.
- **Reuse.** A layer's fingerprint covers its members (path, kind, mode, owner, size and content digest or link target) and the output settings. A layer whose fingerprint and outputs are unchanged is reused without writing its tar, erofs or verity. The summary says `reused` or `rebuilt` for each layer.
- **Checks.** Selector ids that name nothing in the build are errors (globs may match nothing), and so are a `default` or a layer table that is not in `order`, a relative path, and `verity` without `erofs`.

### Image checks

`image:check` reads the built image and the kernel tree of the Buildroot output, and fails the run on an error. It runs after the image build (after the assembly, when there is one), only reads, and is reused while the build it reads is unchanged.

```toml
[image.checks]
kernel_modules = true                    # default for Buildroot images; false turns the module checks off
modules_dir = "build/image/buildroot-output/target/lib/modules/7.2.9"  # default: target/lib/modules/<kernel release>
symvers = "build/image/buildroot-output/build/linux-custom/Module.symvers"  # default: build/linux-*/Module.symvers
kernel_image = "build/image/buildroot-output/images/Image"  # default: images/Image
pi_model = "cm5"                         # optional: evaluate the boot config's [model] sections for this board
```

Paths are as written: a relative one resolves against the workspace root. The three paths are overrides for a build that keeps its kernel elsewhere; an override that names no file or directory is an error, while a default that is missing is a warning.

The kernel module checks run on every shipped `.ko`, `.ko.xz`, `.ko.zst` or `.ko.gz` (compressed modules are decompressed with `xz`, `zstd` or `gzip` from `PATH`). The kernel release is read from the shipped kernel image's `Linux version <release>` banner, gunzipped first when the image is gzip, so a kernel and modules from different builds are caught. When the Buildroot config has no `BR2_LINUX_KERNEL=y` (and no override is set) the check is reported as skipped.

- **Errors** fail the run. `vermagic_mismatch`: a module's vermagic does not start with the kernel release. `vermagic_flags_differ`: a module's flags differ from the most common flags. `undefined_symbol`: an import that neither `Module.symvers` nor a shipped module exports. `export_not_shipped`: an import that a module exports, where that module is neither shipped nor built in. `abi_mismatch`: a module's `__versions` CRC for a symbol differs from the kernel's `Module.symvers` (`CONFIG_MODVERSIONS`), the case where a module loads on one kernel and is refused by another. `dependency_missing`: a `depends=` module that is neither shipped nor in `modules.builtin`. `module_invalid`: a file that is not a readable ELF object. `kernel_release`: no banner in the kernel image.
- **Warnings** do not fail the run and appear in the run summary. `module_duplicate`: a module name shipped more than once. Both copies are checked; the one `modprobe` loads is named, from `modules.dep` (depmod's own entry), else by depmod's search order (`updates`, `extra`, `weak-updates`, then the kernel tree). `firmware_missing`: a `firmware=` file not under `lib/firmware`, with or without a compression suffix. Drivers often list optional firmware, so this is a warning. `symvers_missing`: no `Module.symvers` was found, so imports are not checked.

The report is `<out dir>/<build>.checks.json`, written on success and on failure. It holds `status` (`passed`, `failed` or `skipped`), `hardware_entries` (the count), the release, the module and import counts, and each error and warning with its `code`, `module` and `message`. Paths in it are relative to the Buildroot output.

#### Boot config

The boot trees the image ships are read as the Raspberry Pi firmware reads them: each assembly tree with a top-level `config.txt`, or, when the assembly makes none (Buildroot's own image is the boot partition), Buildroot's `images/rpi-firmware`. With an assembled boot tree, Buildroot's `rpi-firmware/config.txt` is only an input the assembly copies from and is not checked. The firmware reads `config.txt` on a normal boot and `tryboot.txt` (when present) on a trial boot, so each path is read on its own, with the rules Atlas's `check-boot-overlays.sh` uses:

- `include <file>` reads a file relative to the tree root, as if its text were in place of the line. `#` comments, CRs and surrounding whitespace are dropped, and file names compare without regard to case (a FAT volume).
- Section filters (`[all]`, `[cm5]`, ...) are not evaluated by default: every `dtoverlay=` counts, whatever its section. That is stricter than the firmware, and right for a must-exist check. With `pi_model = "cm5"` in `[image.checks]` they are evaluated for that model: `[all]` and `[none]` as written, `[tryboot]` on the trial path only, a model filter (`pi*`, `cm*`) only when it names that model. An unknown filter applies, with a `boot_filter_note` warning.
- `os_prefix=` is the last value on the path (it may be empty). `overlay_prefix=` (default `overlays/`) is read the same way and names the directory relative to `os_prefix`.
- The firmware reads overlays from `${os_prefix}${overlay_prefix}` when that directory holds a `README` (any content), else from `${overlay_prefix}` at the root. `dtoverlay=<name>[,params]` names the overlay before the first comma or colon; an empty name is skipped; `dtparam` is ignored.

What fails the run, per boot path:

- `boot_overlay_missing`: a `dtoverlay` whose `<name>.dtbo` is not in the directory the firmware reads. The error names the tree, the boot file, the overlay and that directory, and says where the `.dtbo` is when it is elsewhere: `slots/<id>/overlays/ has ov9782.dtbo but no README, so the firmware reads overlays/`.
- `boot_overlays_without_readme`: a `${os_prefix}${overlay_prefix}` directory that exists with no README. The firmware ignores it, so its overlays do not boot even when the root has them.
- `boot_fallback_overlay_missing`: on `config.txt`, when the slot's directory is read, the root `overlays/` (the firmware's fallback copy) must hold every `.dtbo` the slot ships.
- `boot_include_missing` and `boot_include_cycle`: an `include` that names no file, or loops.

Not handled: `autoboot.txt`, `boot_partition` and `upstream_kernel`.

#### Hardware list

`[[image.hardware]]` declares what the image must support: one table per device. Atlas owns the lists for its boards, and the file paths in them are the ones the drivers' owners ship. Each entry is checked as a chain, and an error names the build, the entry and the broken link:

```toml
[[image.hardware]]
id = "ov9782"                 # unique in the build
kind = "camera-sensor"        # a free-form label for the reports
modules = ["ov9282"]          # module names (dashes or underscores); built-in drivers too; empty = any driver may claim
compatible = ["ovti,ov9782"]  # device-tree compatibles the driver must claim
overlays = ["ov9782"]         # .dtbo basenames that must ship in the overlays directory a boot config reads
files = ["/usr/share/libcamera/ipa/rpi/pisp/ov9782.json"]  # must exist in the rootfs
firmware = []                 # must exist under /lib/firmware, with or without a compression suffix
```

- **Modules:** each named module passes the module checks, with no error on it. A name shipped twice is checked as the copy modprobe loads. A built-in driver (`dwc2`, `ramoops`) is found through `modules.builtin` and its `modules.builtin.modinfo` aliases. An empty `modules` lets any shipped or built-in driver claim the compatible.
- **Claims:** each compatible must be claimed by a driver, through an `of:N*T*C<compatible>` alias in the module's `.modinfo` or in `modules.alias` (or a built-in's alias). With `modules` set, the module modprobe loads must claim it.
- **Naming:** each compatible must be named where the board instantiates it: by an overlay of the entry (the `__overlay__` nodes of its fragments) or by a shipped board DTB (`images/*.dtb`). Some overlays only configure a node the board already has, so the board DTB names the compatible.
- **Overlays:** each `<name>.dtbo` must be in the overlays directory that the normal boot path (`config.txt`) of at least one boot tree reads (see Boot config). The old search of every `overlays/` directory is gone: a `.dtbo` beside the wrong directory fails. Board DTBs are read from `images/*.dtb` and each assembly tree's top-level `*.dtb`.
- **Files:** each `files` path exists in the final rootfs, which is `target/` without the files of `[image.rootfs] exclude_packages` (the paths `build/packages-file-list.txt` lists for those packages, unless a kept package lists them too). Each `firmware` name must exist under `lib/firmware` in that rootfs.

Every `dtoverlay=<name>` on each boot path (see Boot config), whether or not an entry names it, must have its `.dtbo` in the directory that path reads. The hardware list runs with `kernel_modules = false` too (that turns off only the module checks). It needs the kernel's modules, so a non-empty list with no kernel (`BR2_LINUX_KERNEL` not `y`) is an error, not a skip.

Errors: `hardware_module_missing`, `hardware_module_faulty` (the module fails its module checks), `hardware_module_unreadable`, `hardware_compatible_unclaimed`, `hardware_compatible_unnamed`, `hardware_overlay_missing`, `hardware_overlay_unreadable`, `hardware_file_missing`, `hardware_firmware_missing` and `hardware_board_unreadable`. The boot config reports `boot_overlay_missing`, `boot_overlays_without_readme`, `boot_fallback_overlay_missing`, `boot_include_missing` and `boot_include_cycle` (errors), and `boot_filter_note` (a warning). Validation reports `image_hardware_id_missing` and `image_hardware_id_duplicate` (an id is required and unique), and `image_hardware_not_buildroot` for a list on an image that is not Buildroot (nothing would check it).

Only the checks listed above are made. A driver that loads but fails in its `probe` is caught by a board test, not here.
