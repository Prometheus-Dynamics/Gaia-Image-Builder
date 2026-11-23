//! Checks on the unpacked root that fail the build when it is not what
//! Buildroot's own tables say it should be.
//!
//! Buildroot's fakeroot stage is what makes the users and the enable links
//! (see `perms`). A root that lacks them is an image that boots without its
//! users or its services, so this step says so rather than store it.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

/// Each user and group the users table declares must be in the root's
/// `etc/passwd` and `etc/group`. A missing table is no users. The table is
/// Buildroot's `full_users_table.txt`: `name uid group gid password home
/// shell groups comment` per line, `-` for none. It is read as `mkusers`
/// reads it: everything from a `#` on is dropped, blank lines are skipped,
/// and the name `-` declares a group without a user.
///
/// `removed` are the names the image says it takes out of the root after
/// the table is applied (a post-fakeroot script's work): a user or group of
/// such a name is not held to the root. So that the list cannot go stale,
/// each name must be one the table declares, and must be gone from the
/// root.
pub fn users(root: &Path, table: &Path, removed: &[String]) -> Result<(), String> {
    let text = match fs::read_to_string(table) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("the users table {}: {error}", table.display())),
    };
    if text.is_empty() && removed.is_empty() {
        return Ok(());
    }
    let names = |file: &str, column: usize| -> Result<Vec<String>, String> {
        let path = root.join(file);
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(text
            .lines()
            .filter_map(|line| line.split(':').nth(column))
            .map(str::to_string)
            .collect())
    };
    let passwd = names("etc/passwd", 0)?;
    let group = names("etc/group", 0)?;
    let mut missing: Vec<String> = Vec::new();
    let mut declared: Vec<&str> = Vec::new();
    let is_removed = |name: &str| removed.iter().any(|gone| gone == name);
    for line in text
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .filter(|line| !line.trim().is_empty())
    {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            return Err(format!(
                "the users table line {line:?} has fewer than 8 fields"
            ));
        }
        if fields[0] != "-" {
            declared.push(fields[0]);
            if !is_removed(fields[0]) && !passwd.iter().any(|name| name == fields[0]) {
                missing.push(format!("user {} in etc/passwd", fields[0]));
            }
        }
        let mut groups = vec![fields[2]];
        if fields[7] != "-" {
            groups.extend(fields[7].split(','));
        }
        for name in groups {
            if name == "-" {
                continue;
            }
            declared.push(name);
            if !is_removed(name) && !group.iter().any(|have| have == name) {
                missing.push(format!("group {name} in etc/group"));
            }
        }
    }
    // The list of removed names is held to as well: each is one the table
    // declares, and is gone.
    let mut stale: Vec<String> = Vec::new();
    for name in removed {
        if !declared.contains(&name.as_str()) {
            stale.push(format!(
                "{name}: the users table declares no user or group of that name"
            ));
        }
        if passwd.iter().any(|have| have == name) {
            stale.push(format!("user {name} is still in etc/passwd"));
        }
        if group.iter().any(|have| have == name) {
            stale.push(format!("group {name} is still in etc/group"));
        }
    }
    if !stale.is_empty() {
        return Err(format!(
            "the image declares users or groups removed (`removed_accounts`) that are not: take each out of the list, or remove it from the root:\n  {}",
            stale.join("\n  ")
        ));
    }
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the OS's users table declares {}, which the root does not have (Buildroot's fakeroot stage adds them, so something took {} out, or that stage did not reach the root). For each: declare it removed with a reason (`[[images.removed_accounts]]` in the product), or stop removing it:\n  {}",
        if missing.len() == 1 { "one" } else { "more" },
        if missing.len() == 1 { "it" } else { "them" },
        missing.join("\n  ")
    ))
}

/// The systemd preset files of the root, by name: `etc` overrides `usr/lib`
/// with the same name, as systemd reads them.
fn preset_files(root: &Path) -> io::Result<BTreeMap<String, std::path::PathBuf>> {
    let mut files = BTreeMap::new();
    for sub in ["usr/lib/systemd/system-preset", "etc/systemd/system-preset"] {
        let dir = root.join(sub);
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "preset") {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                files.insert(name, path);
            }
        }
    }
    Ok(files)
}

/// Whether `name` matches `pattern`, where `*` is any run of characters and
/// `?` one character (systemd's preset patterns).
fn glob(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    fn at(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => at(&p[1..], n) || (!n.is_empty() && at(p, &n[1..])),
            (Some('?'), Some(_)) => at(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => at(&p[1..], &n[1..]),
            _ => false,
        }
    }
    at(&p, &n)
}

/// Whether the unit file of `unit` (in `etc` or `usr/lib`) has an install
/// section that names a target to want or require it: the only units whose
/// enable makes a link.
fn wants_a_link(root: &Path, unit: &str) -> bool {
    let text = fs::read_to_string(root.join("etc/systemd/system").join(unit))
        .or_else(|_| fs::read_to_string(root.join("usr/lib/systemd/system").join(unit)));
    let Ok(text) = text else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("WantedBy=") || line.starts_with("RequiredBy=")
    }) && text.lines().any(|line| line.trim() == "[Install]")
}

/// Whether a link named `unit` is in `etc/systemd/system/<target>.wants/` or
/// `<target>.requires/` for some target.
fn has_link(root: &Path, unit: &str) -> io::Result<bool> {
    let system = root.join("etc/systemd/system");
    let Ok(read) = fs::read_dir(&system) else {
        return Ok(false);
    };
    for entry in read {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.ends_with(".wants") || name.ends_with(".requires")) {
            continue;
        }
        if fs::symlink_metadata(entry.path().join(unit)).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The root's systemd presets, applied as systemd applies them, must have
/// made every enable link they call for. A preset decides a unit by its first
/// matching line, in file order. A unit enabled with no `[Install]` section
/// that names a target makes no link, so none is expected; a root with no
/// systemd, or with no preset file, has nothing to check. A root whose
/// presets enable nothing (only `disable` lines, or no unit it names) passes.
///
/// # Errors
///
/// Each unit a preset enables, that wants a target, with no link in
/// `etc/systemd/system`.
pub fn presets(root: &Path) -> Result<(), String> {
    if !root.join("usr/lib/systemd/systemd").exists() {
        return Ok(());
    }
    let files = preset_files(root).map_err(|e| format!("the systemd presets: {e}"))?;
    if files.is_empty() {
        return Ok(());
    }
    let mut lines = Vec::new();
    for path in files.values() {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let mut words = line.split_whitespace();
            let verb = words.next().unwrap_or_default();
            if verb == "enable" || verb == "disable" {
                for pattern in words {
                    lines.push((verb == "enable", pattern.to_string()));
                }
            }
        }
    }
    // The units the presets can name: unit files of either place, no templates.
    let mut units = std::collections::BTreeSet::new();
    for sub in ["usr/lib/systemd/system", "etc/systemd/system"] {
        let Ok(read) = fs::read_dir(root.join(sub)) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains('.') && !name.contains("@.") {
                units.insert(name);
            }
        }
    }
    let mut problems = Vec::new();
    for unit in &units {
        let decided = lines
            .iter()
            .find(|(_, pattern)| glob(pattern, unit))
            .map(|(enable, _)| *enable);
        if decided != Some(true) || !wants_a_link(root, unit) {
            continue;
        }
        if !has_link(root, unit).map_err(|e| format!("the systemd links: {e}"))? {
            problems.push(unit.clone());
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "a systemd preset enables {} unit{} that wants a target, and no link was made in etc/systemd/system:\n  {}",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" },
        problems.join("\n  ")
    ))
}

/// Both checks: the users and groups in the root, and the enable links the
/// presets call for.
pub fn root(root: &Path, users_table: &Path, removed: &[String]) -> Result<(), String> {
    users(root, users_table, removed)?;
    presets(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, file: &str, text: &str) {
        let path = root.join(file);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }

    /// A root with systemd, a unit that wants a target, and `preset`.
    fn systemd_root(preset: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(root, "usr/lib/systemd/systemd", "");
        write(
            root,
            "usr/lib/systemd/system/app.service",
            "[Unit]\nDescription=app\n\n[Install]\nWantedBy=multi-user.target\n",
        );
        write(
            root,
            "usr/lib/systemd/system/plain.service",
            "[Unit]\nDescription=plain\n",
        );
        write(
            root,
            "usr/lib/systemd/system-preset/80-buildroot.preset",
            preset,
        );
        tmp
    }

    #[test]
    fn a_user_the_table_declares_must_be_in_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(root, "etc/passwd", "root:x:0:0::/root:/bin/sh\n");
        write(root, "etc/group", "root:x:0:\n");
        let table = root.join("full_users_table.txt");
        fs::write(&table, "app 1000 app 1000 - /home/app /bin/sh - App user\n").expect("table");
        let error = users(root, &table, &[]).expect_err("app is missing");
        assert!(error.contains("user app in etc/passwd"), "{error}");
        assert!(error.contains("group app in etc/group"), "{error}");
        write(
            root,
            "etc/passwd",
            "root:x:0:0::/root:/bin/sh\napp:x:1000:1000::/home/app:/bin/sh\n",
        );
        write(root, "etc/group", "root:x:0:\napp:x:1000:\n");
        users(root, &table, &[]).expect("both there");
        // No table: no users to hold to.
        users(root, &root.join("absent.txt"), &[]).expect("no table");
    }

    /// An image that takes a declared user out says so: the root is then
    /// not held to it, the rest still are, and the list is held to as well
    /// (a name the table does not declare, or one still in the root, fails).
    #[test]
    fn a_user_the_image_declares_removed_is_not_held_to_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(root, "etc/passwd", "root:x:0:0::/root:/bin/sh\n");
        write(root, "etc/group", "root:x:0:\n");
        let table = root.join("table.txt");
        fs::write(
            &table,
            "dbus -1 dbus -1 * /run/dbus - dbus DBus messagebus user\napp 1000 app 1000 - /home/app /bin/sh - App\n",
        )
        .expect("table");
        let removed = ["dbus".to_string()];
        // Not declared removed: named, with the way out.
        let error = users(root, &table, &[]).expect_err("dbus and app missing");
        assert!(
            error.contains("user dbus in etc/passwd")
                && error.contains("group dbus in etc/group")
                && error.contains(
                    "declare it removed with a reason (`[[images.removed_accounts]]` in the product), or stop removing it"
                ),
            "{error}"
        );
        // Declared removed: dbus is let go, app is still held to.
        let error = users(root, &table, &removed).expect_err("app missing");
        assert!(error.contains("user app in etc/passwd"), "{error}");
        assert!(!error.contains("dbus"), "{error}");
        write(
            root,
            "etc/passwd",
            "root:x:0:0::/root:/bin/sh\napp:x:1000:1000::/home/app:/bin/sh\n",
        );
        write(root, "etc/group", "root:x:0:\napp:x:1000:\n");
        users(root, &table, &removed).expect("dbus removed and said so, app there");
        // Still in the root: the list is stale.
        write(root, "etc/group", "root:x:0:\napp:x:1000:\ndbus:x:101:\n");
        let error = users(root, &table, &removed).expect_err("dbus's group is still there");
        assert!(
            error.contains("declares users or groups removed (`removed_accounts`) that are not")
                && error.contains("group dbus is still in etc/group"),
            "{error}"
        );
        // Not in the table at all.
        write(root, "etc/group", "root:x:0:\napp:x:1000:\n");
        let error = users(root, &table, &["avahi".to_string()]).expect_err("no such name");
        assert!(
            error.contains("avahi: the users table declares no user or group of that name"),
            "{error}"
        );
        // With no table, a removed name has nothing to be removed from.
        let error =
            users(root, &root.join("absent.txt"), &removed).expect_err("no table, a name listed");
        assert!(
            error.contains("dbus: the users table declares no"),
            "{error}"
        );
    }

    #[test]
    fn the_table_is_read_as_mkusers_reads_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(root, "etc/passwd", "root:x:0:0::/root:/bin/sh\n");
        write(root, "etc/group", "root:x:0:\ninput:x:101:\n");
        let table = root.join("table.txt");
        // A comment after whitespace, as systemd's table has, and a
        // group-only line (name `-`).
        fs::write(
            &table,
            " \t# udev user groups\n- - input -1 * - - - Input device group # trailing\n",
        )
        .expect("table");
        users(root, &table, &[]).expect("comments skipped, group-only line held to its group");
        write(root, "etc/group", "root:x:0:\n");
        let error = users(root, &table, &[]).expect_err("input missing");
        assert!(error.contains("group input in etc/group"), "{error}");
        assert!(!error.contains("user -"), "{error}");
    }

    #[test]
    fn a_supplementary_group_is_held_to_as_well() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(root, "etc/passwd", "app:x:1000:1000::/home/app:/bin/sh\n");
        write(root, "etc/group", "app:x:1000:\n");
        let table = root.join("table.txt");
        fs::write(
            &table,
            "app 1000 app 1000 - /home/app /bin/sh dialout,video App\n",
        )
        .expect("table");
        let error = users(root, &table, &[]).expect_err("dialout and video missing");
        assert!(error.contains("group dialout in etc/group"), "{error}");
        assert!(error.contains("group video in etc/group"), "{error}");
    }

    #[test]
    fn a_preset_that_enables_a_unit_needs_its_link() {
        let tmp = systemd_root("enable app.service\n");
        let error = presets(tmp.path()).expect_err("no link");
        assert!(error.contains("app.service"), "{error}");
        // The link systemctl makes for WantedBy=multi-user.target.
        let link = tmp
            .path()
            .join("etc/systemd/system/multi-user.target.wants/app.service");
        fs::create_dir_all(link.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink("/usr/lib/systemd/system/app.service", &link).expect("link");
        presets(tmp.path()).expect("linked");
    }

    #[test]
    fn a_preset_that_enables_nothing_needs_no_link() {
        // Only a disable line, and a unit enabled with no install section.
        let tmp = systemd_root("disable *\n");
        presets(tmp.path()).expect("disabled everything");
        let tmp = systemd_root("enable plain.service\n");
        presets(tmp.path()).expect("plain has no install section");
    }

    #[test]
    fn the_first_matching_preset_line_decides() {
        // The later line would enable it; the first disables.
        let tmp = systemd_root("disable app.service\nenable app*\n");
        presets(tmp.path()).expect("disabled first");
        let tmp = systemd_root("enable app*\ndisable app.service\n");
        assert!(presets(tmp.path()).is_err(), "enabled first");
    }

    #[test]
    fn a_root_without_systemd_has_nothing_to_check() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "usr/lib/systemd/system-preset/80.preset",
            "enable app.service\n",
        );
        presets(tmp.path()).expect("no systemd");
    }
}
