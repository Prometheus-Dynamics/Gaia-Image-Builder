//! Leaving the D-Bus daemon out of the root filesystem (`[image.rootfs]
//! drop_dbus_daemon`).
//!
//! Buildroot's systemd selects dbus for the system bus, and packages built
//! next to it (dnsmasq, for one) link libdbus-1 whenever dbus is configured,
//! so `exclude_packages = ["dbus"]` is refused on most images. An image that
//! runs nothing on the bus can still leave the daemon out: the exclusion's
//! post-build script removes the files of the `dbus` package except the
//! library (`usr/lib/libdbus-1.so.*`), every bus policy and activation file
//! (`usr/share/dbus-1/`, `etc/dbus-1/`, whichever package installed it), and
//! the `dbus.service`/`dbus.socket` links a preset made under `etc/systemd/`.
//! A path a kept package also lists stays.
//!
//! The check after the make: no unit left in `target/` is `Type=dbus` or
//! needs `dbus.socket`/`dbus.service` (`Requires=`, `BindsTo=`,
//! `Requisite=`). Such a service never starts without the daemon (avahi-daemon
//! is one), so the image is refused rather than shipped with it broken.
use super::*;

/// The script block, after the package exclusion. `$drop_dbus` is `1` when on.
/// Rows go to the removal list as `nodbus` (removed) or `shared` (kept).
pub(crate) const NO_DBUS_BLOCK: &str = r#"if [ "$drop_dbus" = 1 ]; then
    nodbus=$removals.nodbus
    awk '
      {
        pos = index($0, ","); if (pos == 0) next
        pkg = substr($0, 1, pos - 1); path = substr($0, pos + 1); sub(/^\.\//, "", path)
        bus = (path ~ /^(usr\/share|etc)\/dbus-1\//)
      }
      NR == FNR { if (pkg != "dbus" && !bus) kept[path] = 1; next }
      ((pkg == "dbus" && path !~ /^usr\/lib\/libdbus-1\.so\./) || bus) && !(path in seen) {
        seen[path] = 1; print ((path in kept) ? "shared" : "nodbus") "\t" pkg "\t" path
      }
    ' "$list" "$list" > "$nodbus"
    # Links a preset made (not in the package file list).
    if [ -d "$target/etc/systemd" ]; then
        find "$target/etc/systemd" -type l \( -name dbus.service -o -name dbus.socket \) |
            LC_ALL=C sort | while IFS= read -r link; do
                printf 'nodbus\tsystemd-preset\t%s\n' "${link#"$target"/}"
            done >> "$nodbus"
    fi
    remove_rows "$nodbus" nodbus
    cat "$nodbus" >> "$removals"
    rm -f -- "$nodbus"
fi
"#;

/// Whether the image leaves the D-Bus daemon out.
pub(crate) fn rootfs_drop_dbus_daemon(image: &ImageSpec) -> bool {
    matches!(&image.definition, ImageDefinition::Buildroot(buildroot) if buildroot.rootfs_drop_dbus_daemon)
}

/// The unit directories of a target tree (merged /usr or not).
const UNIT_DIRS: &[&str] = &[
    "etc/systemd/system",
    "usr/lib/systemd/system",
    "lib/systemd/system",
];

/// The first unit left in `target` that needs the D-Bus daemon, with why.
pub(crate) fn unit_needing_bus(target: &Path) -> Option<(String, String)> {
    let mut seen = BTreeSet::new();
    for dir in UNIT_DIRS {
        let dir = target.join(dir);
        // Merged /usr: lib/systemd/system is usr/lib/systemd/system.
        let Ok(canonical) = fs::canonicalize(&dir) else {
            continue;
        };
        if !seen.insert(canonical) {
            continue;
        }
        let Ok(listing) = fs::read_dir(&dir) else {
            continue;
        };
        let mut units: Vec<PathBuf> = listing
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        units.sort();
        for unit in units {
            let Ok(text) = fs::read_to_string(&unit) else {
                continue;
            };
            if let Some(reason) = bus_need(&text) {
                let rel = unit.strip_prefix(target).unwrap_or(&unit);
                return Some((rel.display().to_string(), reason));
            }
        }
    }
    None
}

/// Why a unit's text needs the bus daemon, if it does.
fn bus_need(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "Type" && value == "dbus" {
            return Some("is Type=dbus".into());
        }
        if matches!(key, "Requires" | "BindsTo" | "Requisite")
            && value
                .split_whitespace()
                .any(|name| name == "dbus.socket" || name == "dbus.service")
        {
            return Some(format!("has {key}={value}"));
        }
    }
    None
}

/// `rootfs: dropped the D-Bus daemon (libdbus-1 kept): N files, X KiB`.
pub(crate) fn no_dbus_message(files: usize, bytes: u64) -> String {
    format!(
        "rootfs: dropped the D-Bus daemon (libdbus-1 kept): {files} files, {:.1} KiB",
        bytes as f64 / 1024.0
    )
}

pub(crate) fn no_dbus_error(message: String) -> ImageProviderError {
    ImageProviderError::backend_command(format!("image.rootfs.drop_dbus_daemon: {message}"))
}

#[cfg(test)]
#[path = "rootfs_no_dbus_tests.rs"]
mod tests;
