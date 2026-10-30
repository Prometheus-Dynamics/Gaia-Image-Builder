# Configuration: Checkpoints, registry and reporting

[Configuration index](configuration.md)

## Checkpoints

```toml
[[checkpoints]]
id = "base-image"
backend = "local"
anchor = "image"
use_policy = "auto"
upload_policy = "off"

[[checkpoints]]
id = "after-api-stage"
backend = "local"
anchor = "stage-service:acme-api"
use_policy = "always"
upload_policy = "off"
```

Checkpoint fields:
- `id`
- `backend`
- `anchor`
- `use_policy`
- `upload_policy`

Checkpoint policies:
- `off`
- `auto`
- `always`

Supported anchor forms:
- `image`
- `install:<install-id>`
- `stage-file:<stage-file-id>`
- `stage-env:<stage-env-set-id>`
- `stage-service:<stage-service-id>`

Important:
- unknown anchors are rejected
- anchors outside the active image feed are rejected
- required/conditional checkpoints on disconnected anchors are rejected as impossible ordering

## Registry checkpoints

A checkpoint with `backend = "oci"` makes an OCI registry a build cache. Build products are stored there as signed artifacts and fetched on a miss, so another machine or a clean checkout can reuse them. It serves three kinds of entry:

- `package`: a Buildroot package build (the package cache's entries, see `[providers.buildroot.package_cache]`);
- `artifact`: an artifact's output (its reuse fingerprint is the key);
- `layer`: an image layer (`[image.layers]`), keyed by the layer's input fingerprint.

```toml
[[checkpoints]]
id = "registry"
backend = "oci"
registry = "ghcr.io"                   # host[:port]
repository = "owner/gaia-cache"        # entries are <repository>/<kind>:<key>
verify_key = "env:GAIA_VERIFY_PUB"     # public key; pulls need it
sign_key = "env:GAIA_SIGN_KEY"         # private key; without it nothing is pushed (a path, env:VAR or secret:NAME)
token = "secret:ghcr"                  # registry token (secret:NAME or env:VAR); see Secrets
username = "gaia"                      # user paired with token; default gaia
sign_kind = "minisign"                 # "minisign" | "gpg" | "cosign-blob"; default [image.signing] kind, else minisign
use_policy = "auto"                    # off: no pulls; any other policy pulls on a miss
upload_policy = "auto"                 # off: no pushes; any other policy pushes what a run built
```

- `registry`, `repository` and `verify_key` are required when pulls are on. Only one checkpoint may use `backend = "oci"`. `anchor` is not used: the registry holds entries of every kind. `auto` and `always` behave the same for registry entries.
- A key is a path, `env:VAR` naming a variable that holds it, or `secret:NAME` naming a `[secrets]` entry that holds it (see [Secrets](configuration.md#secrets)). A key given as `env:VAR` whose variable is unset is a warning in the run (for a sign key, nothing is pushed); the error message names the variable, never its value. A `secret:` key is read when a signature is made, so its failure is a failed push, a warning in the run.
- **Entries.** Each entry is an OCI artifact at `<repository>/<kind>:<key>` with media type `application/vnd.gaia.cache.v1+tar`: one layer, a tar.zst of the entry's files, and the empty config descriptor. Annotations hold the Gaia version, `dev.gaia.kind`, `dev.gaia.key`, `org.opencontainers.image.created` (the push time, used by prune) and, for packages, the package name, the entry's file stem and whether it is relocatable. The signature is a second artifact at tag `<key>.sig` whose blob is the signature over the exact bytes of the entry's manifest.
- **Signing and verification.** The signature tool and its flags are the same as `[image.signing]` (`minisign -S`, `gpg --detach-sign`, `cosign sign-blob`) and verification uses the matching verify command (`minisign -V`, `gpg --verify`, `cosign verify-blob`). A pulled entry is used only when its manifest verifies, its annotations name the requested kind and key, and its blob matches the manifest's digest. A signature that does not verify refuses the entry and the run says so loudly; the package is then built.
- **Unsigned pulls are unsafe.** `trust = "unsigned"` accepts pulls without a signature check: a registry tag that someone else wrote can then reach the build. It is for a registry you fully control. Without `verify_key` and without `trust`, validation rejects pulls.
- **Credentials.** The token is taken from the checkpoint's `token` (`secret:NAME` or `env:VAR`, with the user from `username`, default `gaia`), else `GAIA_REGISTRY_TOKEN` (with `GAIA_REGISTRY_USER`, default `gaia`), else `~/.docker/config.json` (`auths`, `credHelpers` or `credsStore`), else `gh auth token` for `ghcr.io`, else anonymous. A literal token in the build file is refused; a `token` whose secret cannot be read fails the request instead of falling back to anonymous. A bearer challenge is answered with a token from its realm, and the credentials go to that realm only when it is on the registry's own host. Credentials are passed to `curl` through a 0600 config file that is removed after each request, never on a command line, and never printed.
- **Failures are warnings.** A pull or push that fails (network, a missing tag, a refused write) is a `WARN` line in the run and the build goes on. Pushes run on scoped threads that are joined before the build step returns. A push is skipped when the tag already holds an entry that verifies; a tag whose entry does not verify (poisoned or corrupt) is overwritten with this build's signed entry, with a `WARN` saying so, so a refused entry does not stay in the registry. Without a `verify_key` the pusher cannot check, and an existing tag is kept.
- **Restored artifacts** count as reused (from the registry) in the run summary.
- **Transport.** Gaia runs the external `curl`, `tar` and `zstd` programs; a `registry` on `localhost`, `127.0.0.1` or `[::1]` is spoken to over plain HTTP (for local test registries), any other host over HTTPS.
- **Pruning.** `gaia cache --prune --remote --checkpoint registry [--keep-days 30] [--dry-run]` deletes entries whose created time is older than `--keep-days` (30 by default), with their signature tags. Entries whose created time cannot be read are kept. Deletion uses the generic OCI manifest DELETE. GHCR does not accept it with a plain token: deleting packages there needs the GitHub Packages API and a token with `delete:packages`, and a refused delete is reported without stopping the run. Use `--dry-run` first; it lists every entry that would go.

Not implemented yet: the roots index (a build's list of keys, which would keep older entries that it still uses) and the per-kind hit and miss counts for `gaia cache stats`. Until the roots index exists, `--prune` looks only at age: it can remove an entry that an older build still uses. The next build of that package, artifact or layer rebuilds it and pushes it again.

Artifact keys are a sha256 over the build's fingerprint, the content of its sources (the git commit; a path source's file contents, wherever the checkout sits; an archive's bytes, or its `sha256` for a `url` archive), the identity of the toolchain it builds with (see [Toolchains](configuration-sources.md#toolchains)) and, recursively, the artifacts it depends on: an edited source never pulls an entry built from the old one, and two checkouts of the same content share entries.

Gaps. Layer entries are pushed before the image's later steps (sizes, legal info and archive), so a failure of those steps still leaves them in the registry.

Boot trees for `image:check`: after every assembly, Gaia copies the boot files of each tree with a top-level `config.txt` (every `*.txt`, `README`, `.dtbo` and `.dtb`, at their paths in the tree) to `<build_dir>/image-check/boot-trees/<tree id>/`, and `image:check` reads the firmware rule and the hardware list from that copy. A RAM work dir's trees are removed when the assembly ends, so the copy is what the check can read; it is replaced on every assembly run.

## Canonical tree path

```toml
[execution]
canonical_tree = true   # default false
```

`canonical_tree = true` places a RAM Buildroot tree (`[providers.buildroot] work_dir = "ram"`) at a path named by the build, not by the checkout. It can also be set with `--set execution.canonical_tree=true`.

- Path: `/dev/shm/gaia/<hash>/buildroot-output`, under `GAIA_RAM_BASE` when that is set. `<hash>` is the first 12 hex digits of the SHA-256 of these lines: `gaia-canonical-tree-v1`; `build=<id>`, the build's id; and the Buildroot config identity, which is the shared-output key material (source identity, defconfig and fragment contents, config overrides, external tree, package overrides, ccache and parallel-package settings, Docker image) with the workspace root written as `<workspace>`. Two checkouts of one build with one config therefore use one tree. Without the option the tree is `/dev/shm/gaia-<user>/<hash of the checkout's output path>/buildroot-output`.
- The identity only chooses the location. Config steps and packages are still checked against their inputs as before.
- Disk work dirs are unchanged. A disk tree lives in the build dir and moving it would discard it and change what Docker builds mount. A RAM build that falls back to disk also keeps its per-checkout placement.
- Owner record: `owner.json` in the `<hash>` dir, written under an exclusive lock of that dir. It names the pid, the process start time (field 22 of `/proc/<pid>/stat`), the checkout's output dir and the build id. A record is live while its pid runs with that start time, so a reused pid is not mistaken for its owner. A second live build of the same id does not share the tree: it builds in its per-checkout RAM tree and says so. A stale record is taken over. The record is removed when the build finishes; a kept tree keeps its directory.
- `/dev/shm/gaia` is checked as the per-user RAM directory is: a directory owned by the user, not a symbolic link, with mode 0700 (tightened when looser). Otherwise the build refuses to use it.
- Package cache: a pinned entry (a package whose binaries hold the tree path) restores into any checkout that uses the same canonical path. Without the option it restores only into the checkout that built it. Relocatable entries are unaffected.
- A source given by path has an identity that is a digest of its path, so its canonical tree differs per location.
- Like every RAM tree, the canonical tree is gone after a reboot and the next build starts a fresh one.

## Reporting

```toml
[reporting]
summary = true
provenance = true
manifest = true

[reporting.masking]
enabled = true
replacement = "***"
patterns = ["TOKEN", "SECRET", "PASSWORD", "API_KEY"]
```

`[reporting.post_build]` runs a script after every `gaia run` that writes its
reports:

```toml
[reporting.post_build]
script = "scripts/after-build.sh"   # relative to the workspace root
timeout_seconds = 300               # default 300
```

The script gets the path of a JSON payload (the run's summary, its image
sizes as `sizes`, and the report files) as its first argument and in
`GAIA_POST_BUILD_PAYLOAD`, and the report directory in
`GAIA_POST_BUILD_REPORT_DIR`. It runs from the workspace root, through `bash`
when it is not executable. A hook that fails or times out fails the run.

`patterns` hide the value of a key whose name contains a pattern (in the provenance and the selection). Resolved `[secrets]` values are masked by value in every output, whatever `enabled` says; see [Secrets](configuration.md#secrets).
