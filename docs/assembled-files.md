# Files assembled from several owners

Some configuration has no `.d` directory: `/etc/mdev.conf`, `/etc/fstab`,
`/etc/inittab`. The OS builds the file. A component can't append to it,
and two components can't both install it. Each owner gives its part as a
fragment, and the product states the order. Gaia joins the parts into the
one file. No post-build script runs.

## The component: `[[fragments]]`

```toml
[[fragments]]
from = { source = "src", path = "packaging/init/mdev.conf.fragment" }
to = "/etc/mdev.conf"
```

- `from` takes any of an install's forms: a path in the build's output,
  `{ file }` beside the component, `{ source, path }`, `{ generated }`,
  `{ description }` or `{ input }`. A link is not allowed.
- `to` is the shared file, as an absolute path.
- A component gives at most one fragment per file.
- A fragment carries no `mode`, `owner` or `group`. The file takes the
  OS's own file's mode and owner. When the OS has no such file, it is
  root's, with mode 0644.

## The product: `[[images.assembled]]`

```toml
[[images.assembled]]
path = "/etc/mdev.conf"
order = ["flasher-lemnos-busybox", "board-mdev", "os"]
rule_key = 1   # the default
```

- `order` lists the parts in order:
  - the names of the image's `components` that give a fragment of `path`;
  - `os` for the file as the OS's tree has it.
- The product file's checks, when it is read:
  - each name in `order` appears once;
  - each component named gives a fragment of `path`;
  - each fragment a component of the image gives is a file the image
    assembles, and the component is named in that file's order;
  - `os` needs the image to have an OS;
  - assembled files go into the root filesystem only, so a component in
    `initramfs` or `data` that gives a fragment is refused.
- When the OS's tree has the file, `os` must be in the order, which says
  where its lines go. The file is never dropped silently.
- When the OS's tree has no such file, naming `os` is an error.

## Rules

- A rule is any line that is neither blank nor a comment (`#`).
- A rule's name is its `rule_key`th whitespace-separated field, counting
  from 1:
  - for mdev, field 1 is the device-name match;
  - for fstab, use 2, the mount point;
  - 0 compares whole rules, with whitespace collapsed;
  - a rule with fewer fields than `rule_key` is named whole.
- Two rules with the same name fail the build, in one part or across two.
  The error names both owners and both line numbers, e.g.
  `/etc/mdev.conf: the rule `^video[0-9]+$` is in lemnos (line 2) and in os (line 14)`.
  The file's reader would act on only one of them.

## The file

- Each part is copied byte for byte, in order.
- A part that doesn't end in a newline gets one.
- The file is a layer owned by `assembled`. It replaces the OS's file in
  the merge.
- The step that writes it (`gaia.tree.assemble`) is keyed on the bytes it
  writes and the file's mode, so a change to an unrelated part of the OS
  doesn't remake it.
- Fragments are never merged into the tree themselves. Only the
  assembled file is.

## mdev order

mdev acts on the first rule that matches a device. Rules that must win
therefore go before the OS's catch-all lines. For example, Lemnos's
fragment, then the board's `lemnos-ctl tree --mdev` file, then `os`.
