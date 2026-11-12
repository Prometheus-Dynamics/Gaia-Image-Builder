# Packages for another device's installer

This product's output is not a disk image. It is an `.ipk` that a
SystemCore-style device's installer (opkg) takes. The package is built from
the files a component ships.

- `components/robot-status`: the files (`files/`), and in `[package]` what
  the package says about itself.
- `recipes/ipk.toml`: one stage, `gaia.package.ipk`. Its tree is the
  component's one source (`source`) and its control data comes from the
  component's `[package]` (`control`).
- `boards/systemcore`: the target the package is for.

```
gaia lock product.toml
gaia plan product.toml
gaia build product.toml
gaia show robot-status.ipk
```

`control` is fed by `gaia build` as a host input, because a graph constant
can't be a struct yet. `gaia plan` lists it under "host inputs".
