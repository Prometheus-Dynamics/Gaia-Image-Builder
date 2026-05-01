# Imported Raw Image Mutate

This example proves Gaia can:

- start from an existing raw `.img`
- mutate the mounted rootfs
- execute package-manager changes in the imported OS
- apply normal Gaia artifact install and stage-file overlay
- emit a final mutated `.img`

This example is intended to run inside the privileged Docker smoke harness:

```bash
./scripts/run-starting-point-raw-smoke.sh
```

The final image is:

- `.gaia/examples/imported-raw-image-mutate/out/images/imported-raw-image-mutate-2.0.0.img`
