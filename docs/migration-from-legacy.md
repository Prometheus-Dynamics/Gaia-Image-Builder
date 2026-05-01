# Migration From Legacy Gaia

Legacy Gaia configs were organized around broad buckets such as:
- `buildroot`
- `program`
- `stage`

Current Gaia is organized around typed runtime domains:
- `sources`
- `artifacts`
- `install`
- `stage`
- `image`
- `checkpoints`
- `reporting`

There is no legacy compatibility layer anymore. Old configs must be translated.

## Old To New Mapping

### Old `[buildroot]`

Split into:
- a `git`/`path`/`archive`/`download` source if Buildroot source identity matters
- `[image] kind = "buildroot"`
- `[image.feed]`
- `[[image.expected_images]]`
- `[image.output]`

### Old `[program]`

Split into:
- `[[artifacts]]`
- `[[install]]`

### Old `[stage.files]`, `[stage.env]`, `[stage.services]`

Split into:
- `[[stage.files]]`
- `[[stage.env_sets]]`
- `[[stage.services]]`

### Old checkpoint task anchors like `buildroot.build`

Replace with typed anchors:
- `image`
- `install:<id>`
- `stage-file:<id>`
- `stage-env:<id>`
- `stage-service:<id>`

## Practical Translation Example

Old:

```toml
[program.install]
[[program.install.items]]
artifact = "helios-api"
dest = "/usr/bin/helios-api"
mode = 493
```

New:

```toml
[[artifacts]]
id = "helios-api"
kind = "rust"
package = "helios-api"
source = "workspace-root"
output_path = "${workspace.out_dir}/artifacts/helios-api"
install_name = "helios-api"
install_class = "binary"
install_dest_hint = "/usr/bin/helios-api"

[[install]]
id = "install-helios-api"
artifact = "helios-api"
dest = "/usr/bin/helios-api"
replace = true
mode = 493
owner = "root"
group = "root"
```

## Common Migration Mistakes

- keeping old module bucket names in new configs
- assuming image feed is implied instead of declared
- treating artifact install identity as optional human metadata instead of part of the typed model
- using old checkpoint anchor names
- expecting old examples to run unchanged

## Recommended Migration Strategy

1. Translate workspace/build metadata first.
2. Declare real `sources`.
3. Declare real `artifacts`.
4. Translate program install mappings into `[[install]]`.
5. Translate stage files/env/services into typed stage domains.
6. Translate Buildroot or starting-point behavior into `[image]`.
7. Add checkpoints last.
8. Use `resolve`, then `validate`, then `plan` before `run`.
