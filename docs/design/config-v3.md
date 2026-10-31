# Config v3 (draft for review)

Status: proposal, not implemented. It is to be confirmed with HeliOS before
any code is written, because every HeliOS, PV and Atlas build file changes.

v3 is a rewrite of the build-file *syntax* only. A v2 file and its `gaia
migrate` output must resolve to the same spec fingerprint, so nothing that
is built changes. v2 files keep loading with one deprecation warning that
names `gaia migrate`.

A file opts in with `config = 3` at the top level; without it the file is
read as v2.

## 1. One token syntax

v2 has three:

| v2 | Where | Uses in HeliOS's `gaia/configs` |
|---|---|---|
| `${build.name}`, `${workspace.build_dir}`, `${input.x}`, `${project.root_dir}` | anywhere, resolved at load | 29 |
| `$provider.images`, `$provider.buildroot_output`, `$assembly.work`, `$assembly.tree.<id>` | assembly paths, resolved at run | 21 |
| `@assets/...` | file sources | 48 |

v3 uses `${...}` for all of them. `$provider.images/x.dtb` becomes
`${provider.images}/x.dtb`, and `@assets/board/config.txt` becomes
`${assets}/board/config.txt`. Load-time and run-time tokens keep resolving
when they do today; a run-time token used where only load-time ones are
allowed is still an error, and the error says which.

The `${inputs.x}` alias of `${input.x}` is dropped.

Open question A: `@assets/` is by far the most used token and is short.
Keep `@assets/` as the one exception, or move it too?

## 2. A `[cache]` table

v2 spreads cache settings over the providers: `ccache` and `package_cache`
tables under each of eight `[providers.*]`, plus `providers.rust.sccache`,
`providers.rust.shared_target_dir` and `providers.archive.download_dir`.
v3 collects them:

```toml
[cache]
dir = "~/.cache/gaia"            # the root the others default under
downloads = "${cache.dir}/downloads"
ccache = { enabled = true, max_size = "50G" }   # was providers.buildroot.ccache
packages = true                                  # was providers.buildroot.package_cache.enabled
sccache = true                                   # was providers.rust.sccache
cargo_target = "shared"                          # was providers.rust.shared_target_dir
```

A per-provider override stays possible as `[providers.<kind>.cache]`, with
the same keys. HeliOS sets these only for Buildroot today
(`gaia/configs/workspace/base.toml`), so it moves to `[cache]` unchanged.
`providers.buildroot.host_tools.ccache = "system,build"` chooses which
ccache *binary* to use, not the cache, and stays where it is.

## 3. Outputs at the top level

v2 places outputs in four places: `workspace.out_dir`,
`workspace.export_dir`, `[image.output]` and `image.assembly.out_dir`. v3:

```toml
[outputs]
dir = "out"                      # was workspace.out_dir
export_dir = "export"            # was workspace.export_dir and image.output.export_dir
images_dir = "${outputs.dir}/images"   # was image.output.collect_dir
archive_name = "${build.name}-${build.version}"
disk = "..."
report = true                    # was image.output.emit_report
```

`image.assembly.out_dir` defaults to `${outputs.dir}/images` and stays
settable for the rare build that needs another place.

`image.output.collect_dir` becomes `outputs.images_dir`; HeliOS sets it in
`gaia/configs/os/base.toml`.

Open question B: HeliOS's `collect_dir` is its `out_dir` plus `/images`,
the proposed `images_dir` default. When a v2 value equals the v3 default,
should `migrate` drop it or keep it written out?

## 4. Real enums

v2 reads several choice fields as free strings and checks them afterwards
(`checkpoints.backend`, `checkpoints.sign_kind`, `image.kind_layer`,
`pi_model`, the transform and assembly `kind`s). v3 reads them as enums, so
a typo is a parse error at its line, with the allowed values listed. The
spelling of each value does not change.

## 5. Patch entries

The roadmap listed "no explicit `patch = true`". That is already true in
v2 (c73002b): an entry with an `id` and no declaring key is a patch. There
is nothing left to do.

## `gaia migrate`

`gaia migrate <file>` rewrites a v2 file to v3 in place (a `.v2.bak` copy is
kept) and checks that both resolve to the same fingerprint. Without that
match it refuses and leaves the file alone. Comments and key order are kept
(`toml_edit`). `--check` exits non-zero when a file is still v2, for CI.
