# Gaia in CI

Gaia runs the same way on a CI runner as on a workstation: `gaia run <build file>`.
This page covers what changes on a runner: installing Gaia, keeping its caches
between jobs, passing secrets, and keeping per-push jobs fast.

## The GitHub Action

The repository is a GitHub Action (`action.yml` at its root). It installs Gaia,
restores Gaia's caches, runs one command on a build file and uploads what the
run exports:

```yaml
- uses: actions/checkout@v5
- uses: Prometheus-Dynamics/Gaia-Image-Builder@main
  with:
    config: configs/builds/device.toml
    args: --set build.version=${{ github.ref_name }}
```

| input | default | meaning |
|---|---|---|
| `config` | (required) | The build file, relative to `working-directory`. |
| `command` | `run` | `run`, `validate`, `plan` or `prefetch`. |
| `args` | empty | Extra arguments, split on spaces (`--only artifacts --preset ci`). |
| `working-directory` | `.` | Where gaia runs. |
| `gaia-ref` | empty | A Gaia git ref (tag or commit) to install; empty builds the action's own copy. |
| `cache` | `true` | Restore and save Gaia's caches with `actions/cache`. Turn off on self-hosted runners. |
| `cache-key` | empty | An extra cache key part, one per build that should keep its own cache. |
| `cache-paths` | downloads, git mirrors, Buildroot `dl` and ccache, cargo target | The cached directories, one per line. |
| `export-dir` | `gaia-export` | `gaia run --export` directory. |
| `upload` | `true` | Upload the export directory as a workflow artifact after a successful run. |
| `upload-name` | `gaia-outputs` | The artifact name. |
| `retention-days` | `0` | Artifact retention; `0` is the repository default. |

The output `export-dir` is the absolute export directory.

The Gaia binary is cached by its source, so a job only compiles Gaia when the
action's copy changed (or `gaia-ref` names a new ref). The runner needs Rust
(`rustup`; GitHub-hosted Ubuntu images have it) and whatever the build itself
needs: Buildroot's host packages for a Buildroot image, Docker for artifacts
with `execution.backend = "docker"`.

### What Gaia prints on GitHub

With `CI` set, `gaia run` prints no live progress, and each finished operation
is a collapsible `::group::` in the log. With `GITHUB_ACTIONS=true`, every
validation diagnostic and every failed operation is also a workflow annotation
(`::error title=gaia <code>::...`), so it shows on the run's page and on the
pull request. With `GITHUB_STEP_SUMMARY` set (GitHub sets it for every step),
`gaia run` appends a Markdown summary to the job page: the outcome, the run
time, operations built and reused, the image outputs with their size change
since the previous run, the errors, and the export or `resume:` lines.

## Keep per-push jobs fast

A full image build does not belong on every push. Split the work by what each
event needs:

```yaml
on:
  pull_request:
  push:
    branches: [main]
  schedule:
    - cron: "0 3 * * *"
  workflow_dispatch:

jobs:
  check:
    # Every push: the config, the plan and the artifacts, no image.
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: Prometheus-Dynamics/Gaia-Image-Builder@main
        with:
          config: configs/builds/device.toml
          command: validate
      - uses: Prometheus-Dynamics/Gaia-Image-Builder@main
        with:
          config: configs/builds/device.toml
          args: --only artifacts
          upload: "false"

  image:
    # Nightly and on demand: the whole image, uploaded.
    if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'
    runs-on: [self-hosted, linux, x64]
    steps:
      - uses: actions/checkout@v5
      - uses: Prometheus-Dynamics/Gaia-Image-Builder@main
        with:
          config: configs/builds/device.toml
          cache: "false"
```

- `gaia validate` takes seconds and catches most config mistakes.
- `--only artifacts` builds the sources and artifacts and stops before the
  image (see `--only` in [cli.md](cli.md)).
- `gaia plan` shows what a run would rebuild and why, without building.
- A tag or release job builds the image with the version from the tag:
  `args: --set build.version=${{ github.ref_name }}`.

## Caches

Gaia reuses work through three layers. On CI they are kept like this:

1. **Reuse state and outputs.** These live in the build's `build_dir` and
   `out_dir`. A self-hosted runner keeps them between jobs when they sit
   outside the checkout (`build_dir = "/srv/gaia/<build>/build"`), or when the
   checkout is not cleaned. A GitHub-hosted runner starts empty every time.
2. **Download and compiler caches.** These are `.gaia/cache` (downloads, git
   mirrors) and `~/.cache/gaia` (Buildroot `dl`, ccache, the shared cargo
   target). The action caches them with `actions/cache` under a rolling key:
   each run saves a new entry, and the next run restores the newest one. GitHub
   keeps 10 GB of caches per repository. Buildroot's per-package cache
   (`~/.cache/gaia/buildroot/packages`) is usually larger than that, so it is
   not in the default paths; use a registry for it.
3. **A registry (`[[checkpoints]] backend = "oci"`).** This is the cache that
   works across runners and machines. Built Buildroot packages, artifacts and
   image layers are pushed as signed OCI artifacts, and any runner pulls them
   on a miss. See [Registry checkpoints](configuration-checkpoints.md#registry-checkpoints).
   A runner that only reads the cache needs the verify key and pull access. A
   runner that writes needs the sign key and push access too.

`gaia cache stats <build file>` lists every cache with its path and size.

## Secrets

Declare secrets in the build file and fill them from the job's environment.
The build file then holds only the names:

```toml
[secrets.registry_token]
env = "GAIA_REGISTRY_TOKEN"

[secrets.sign_key]
env = "GAIA_SIGN_KEY"

[[checkpoints]]
id = "registry"
backend = "oci"
registry = "registry.example.com"
repository = "team/gaia-cache"
verify_key = "keys/gaia-cache.pub"
sign_key = "secret:sign_key"
token = "secret:registry_token"
```

```yaml
- uses: Prometheus-Dynamics/Gaia-Image-Builder@main
  env:
    GAIA_REGISTRY_TOKEN: ${{ secrets.REGISTRY_TOKEN }}
    GAIA_SIGN_KEY: ${{ secrets.GAIA_SIGN_KEY }}
  with:
    config: configs/builds/device.toml
```

Secret values never reach the spec, the reuse state or the reports. They are
read when a process needs them. A docker build gets each one as `-e KEY`,
with the value in its environment rather than on the command line. A rust or
java artifact can pass a secret to its build with
`build_env = [["GIT_TOKEN", "${secret:git_token}"]]` (see
[Secrets](configuration.md#secrets)). For pull requests from forks, GitHub
withholds repository secrets: such a job can pull from a public cache but
cannot push to it.

## Self-hosted runners

A self-hosted runner keeps its disk, which removes most of the cache work:

- Set `cache: "false"` on the action. The caches are already on disk.
- Put `build_dir` and `out_dir` outside the checkout, so a fresh checkout still
  finds its reuse state.
- Run one Gaia build per runner at a time (`concurrency:` in the workflow). Two
  runs of the same build would share a build directory.
- Give scratch data a disk path: Gaia's RAM work dirs (`work_dir = "ram"`) use
  `/dev/shm`, and a runner's `/tmp` is often small.
- Prune now and then: `gaia clean <build file> --target caches --unused-mirrors`
  locally, and `gaia cache --prune --remote --checkpoint registry --keep-days 30`
  for a registry.

On a GitHub-hosted runner a Buildroot image needs the disk space of a full
Buildroot tree, about 15 to 30 GB for a typical board. The hosted Ubuntu
images have about 14 GB free, so a full Buildroot image needs space freed first,
a larger runner, or a self-hosted runner. Artifact-only and starting-point
builds fit easily.

## Other CI systems

Nothing in Gaia is GitHub-specific apart from the annotations and the step
summary, which turn on by themselves when GitHub's variables are set. On any
other system:

```bash
cargo install --locked --git https://github.com/Prometheus-Dynamics/Gaia-Image-Builder gaia
CI=true gaia run configs/builds/device.toml --export out/export
```

Then cache `.gaia/cache` and `~/.cache/gaia` with that system's cache, and keep
the export directory as the job's artifact.
