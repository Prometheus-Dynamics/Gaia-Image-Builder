//! `gaia.tree.by-role`: a directory filled by role from a layout file.

use gaia_step::{Ctx, Store};
use gaia_tree::{RecordRole, placements, records, tree_by_role};
use gaia_types::Tree;

/// The shape of a slot's boot directory layout: files the build has by
/// role and one the layout's owner's generator writes, for one firmware,
/// and one file for another.
const LAYOUT: &str = r#"
version = 3

[[file]]
name = "kernel8.img"
role = "kernel"
source = "build"
firmware = ["rpi-tryboot"]
required = true

[[file]]
name = "board.dtb"
role = "device-tree"
source = "build"
firmware = ["rpi-tryboot"]
required = true

[[file]]
name = "cmdline.txt"
role = "cmdline"
source = "build"
firmware = ["rpi-tryboot"]
required = true

[[file]]
name = "overlays/board.dtbo"
role = "board-overlay"
source = "build"
firmware = ["rpi-tryboot"]
required = true

[[file]]
name = "overlays/README"
role = "overlay-readme"
source = "generated"
firmware = ["rpi-tryboot"]
required = true

[[file]]
name = "notes.txt"
role = "notes"
source = "build"
firmware = ["rpi-tryboot"]
required = false

[[record]]
name = "trial.rec"
role = "trial"
firmware = ["rpi-tryboot"]
bytes = 1024
source = "generated"

[[record]]
name = "reason.rec"
role = "reason"
firmware = ["rpi-tryboot"]
bytes = 1024
source = "generated"

[[file]]
name = "extlinux/extlinux.conf"
role = "boot-menu"
source = "build"
firmware = ["uboot"]
required = true
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    store: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = root.join("store").display().to_string();
        Self {
            _dir: dir,
            root,
            store,
        }
    }

    fn tree(&self, name: &str, files: &[(&str, &str)]) -> Tree {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        }
    }

    fn fill(&self, layout: &str, name: &str, files: &[(&str, &str)]) -> Result<Tree, String> {
        self.fill_with(
            layout,
            name,
            files,
            &[("overlays/README", "generated\n"), ("config.txt", "c")],
        )
    }

    fn fill_with(
        &self,
        layout: &str,
        name: &str,
        files: &[(&str, &str)],
        generated: &[(&str, &str)],
    ) -> Result<Tree, String> {
        let layout = self.tree(
            &format!("layout-{name}"),
            &[("contracts/slot-layout.toml", layout), ("src/other", "x")],
        );
        tree_by_role(
            &self.store,
            layout,
            "contracts/slot-layout.toml",
            "rpi-tryboot",
            self.tree(name, files),
            self.tree(&format!("generated-{name}"), generated),
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())
    }

    fn path(&self, tree: &Tree) -> std::path::PathBuf {
        let store = Store::open(&self.store).expect("store");
        tree.object.path_in(&store).expect("in the store")
    }
}

const FILES: [(&str, &str); 4] = [
    ("kernel", "the kernel"),
    ("device-tree", "the dtb"),
    ("cmdline", "console=ttyS0\n"),
    ("board-overlay", "the overlay"),
];

#[test]
fn each_role_goes_where_the_layout_says() {
    let f = Fixture::new();
    let made = f.fill(LAYOUT, "all", &FILES).expect("filled");
    let dir = f.path(&made);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).expect(name);
    assert_eq!(read("kernel8.img"), "the kernel");
    assert_eq!(read("board.dtb"), "the dtb");
    assert_eq!(read("cmdline.txt"), "console=ttyS0\n");
    assert_eq!(read("overlays/board.dtbo"), "the overlay");
    // The generator's file, from the generator's tree at the same path;
    // what else the generator wrote is not this directory's.
    assert_eq!(read("overlays/README"), "generated\n");
    assert!(!dir.join("config.txt").exists());
    // The optional file is not there, and nothing of another firmware.
    assert!(!dir.join("notes.txt").exists());
    assert!(!dir.join("extlinux").exists());
    // No file keeps its role as a name.
    assert!(!dir.join("kernel").exists());

    // An optional role, when the build has it.
    let mut with_notes = FILES.to_vec();
    with_notes.push(("notes", "n"));
    let made = f.fill(LAYOUT, "notes", &with_notes).expect("filled");
    assert_eq!(
        std::fs::read_to_string(f.path(&made).join("notes.txt")).expect("notes"),
        "n"
    );
}

#[test]
fn a_missing_required_role_and_an_unknown_role_are_named() {
    let f = Fixture::new();
    let why = f
        .fill(LAYOUT, "short", &FILES[..2])
        .expect_err("two roles are missing");
    println!("{why}");
    assert!(
        why.contains(
            "the layout contracts/slot-layout.toml requires for rpi-tryboot what is not there"
        ),
        "{why}"
    );
    assert!(
        why.contains("cmdline: the build has no file for this role (it goes to cmdline.txt)"),
        "{why}"
    );
    assert!(
        why.contains("board-overlay: the build has no file"),
        "{why}"
    );

    // The generator's file is the generator's to write: the build cannot
    // supply it by role, and its absence names the generator.
    let why = f
        .fill_with(LAYOUT, "no-readme", &FILES, &[("config.txt", "c")])
        .expect_err("the generator wrote no README");
    assert!(
        why.contains("overlay-readme: the generator wrote no overlays/README"),
        "{why}"
    );
    let mut own = FILES.to_vec();
    own.push(("overlay-readme", "mine"));
    let why = f
        .fill(LAYOUT, "own-readme", &own)
        .expect_err("not the build's role");
    assert!(
        why.contains("has no role overlay-readme for the build to supply"),
        "{why}"
    );

    let mut extra = FILES.to_vec();
    extra.push(("initramfs", "i"));
    let why = f
        .fill(LAYOUT, "extra", &extra)
        .expect_err("a role the layout does not list");
    assert!(
        why.contains("has no role initramfs for the build to supply for rpi-tryboot; those roles are kernel, device-tree, cmdline, board-overlay, notes"),
        "{why}"
    );
}

#[test]
fn another_version_an_unknown_key_and_a_bad_layout_are_refused() {
    let read = |text: &str| placements(text, "slot-layout.toml", "rpi-tryboot");
    assert_eq!(
        read(&LAYOUT.replace("version = 3", "version = 2")),
        Err("slot-layout.toml: version 2; this build reads version 3".into())
    );
    assert_eq!(
        read(&LAYOUT.replace("version = 3", "")),
        Err("slot-layout.toml: no `version`".into())
    );
    let err = read(&LAYOUT.replace("role = \"notes\"", "role = \"notes\"\nmode = \"0644\""))
        .expect_err("unknown key");
    assert!(err.contains("unknown field `mode`"), "{err}");
    let err = read(&format!("{LAYOUT}\n[later]\nx = 1\n")).expect_err("unknown table");
    assert!(err.contains("unknown field `later`"), "{err}");
    let err = read(&LAYOUT.replace("\"kernel8.img\"", "\"../kernel8.img\"")).expect_err("outside");
    assert!(err.contains("not a path inside the tree"), "{err}");
    let err = read(&LAYOUT.replace("role = \"notes\"", "role = \"kernel\"")).expect_err("twice");
    assert!(
        err.contains("cannot both be listed for rpi-tryboot"),
        "{err}"
    );
    let err = read(&LAYOUT.replace("source = \"generated\"", "source = \"downloaded\""))
        .expect_err("unknown source");
    assert!(err.contains("unknown variant `downloaded`"), "{err}");
    let err = placements(LAYOUT, "slot-layout.toml", "grub").expect_err("no such firmware");
    assert_eq!(
        err,
        "slot-layout.toml: no file is listed for grub; it lists rpi-tryboot, uboot"
    );
    // Another firmware's files are its own.
    let uboot = placements(LAYOUT, "slot-layout.toml", "uboot").expect("read");
    assert_eq!(uboot.len(), 1);
    assert_eq!(
        (
            uboot[0].role.as_str(),
            uboot[0].path.as_str(),
            uboot[0].required
        ),
        ("boot-menu", "extlinux/extlinux.conf", true)
    );
}

#[test]
fn a_change_for_another_firmware_or_elsewhere_in_the_source_is_no_new_step() {
    use gaia_step::{logs_dir, step_records};
    let f = Fixture::new();
    let first = f.fill(LAYOUT, "a", &FILES).expect("filled");
    let other = LAYOUT.replace("extlinux/extlinux.conf", "boot/extlinux.conf");
    let second = f.fill(&other, "a", &FILES).expect("filled");
    assert_eq!(second, first);
    let store = Store::open(&f.store).expect("store");
    let made = step_records(&logs_dir(&store))
        .into_iter()
        .filter(|r| r.step == "gaia.tree.by-role" && r.made)
        .count();
    assert_eq!(made, 1, "the second was found in the store");
    // Another file of the generator's changes nothing either; the file
    // taken from it does.
    let readme = ("overlays/README", "generated\n");
    let third = f
        .fill_with(LAYOUT, "a", &FILES, &[readme, ("config.txt", "other")])
        .expect("filled");
    assert_eq!(third, first);
    let fourth = f
        .fill_with(LAYOUT, "a", &FILES, &[("overlays/README", "new\n")])
        .expect("filled");
    assert_ne!(fourth, first);
}

/// The boot partition's record files: read, not placed in a slot's
/// directory, and held to the same refusals.
#[test]
fn the_record_files_are_read_and_what_is_unknown_in_them_is_refused() {
    let read = |text: &str| records(text, "slot-layout.toml", "rpi-tryboot");
    let found = read(LAYOUT).expect("reads");
    assert_eq!(found.len(), 2);
    assert_eq!(
        (found[0].name.as_str(), found[0].role, found[0].bytes),
        ("trial.rec", RecordRole::Trial, 1024)
    );
    assert_eq!(found[1].role, RecordRole::Reason);
    // None for a firmware that has none.
    assert_eq!(records(LAYOUT, "x", "uboot"), Ok(Vec::new()));

    let err = read(&LAYOUT.replace("role = \"reason\"", "role = \"counter\"")).expect_err("role");
    assert!(err.contains("unknown variant `counter`"), "{err}");
    let err = read(&LAYOUT.replace(
        "bytes = 1024\nsource = \"generated\"\n\n[[record]]",
        "bytes = 1024\nsource = \"generated\"\ncopies = 2\n\n[[record]]",
    ))
    .expect_err("unknown key");
    assert!(err.contains("unknown field `copies`"), "{err}");
    let err =
        read(&LAYOUT.replace("name = \"reason.rec\"", "name = \"trial.rec\"")).expect_err("twice");
    assert!(
        err.contains("the record trial.rec is listed twice"),
        "{err}"
    );
    let err = read(&LAYOUT.replace("name = \"reason.rec\"", "name = \"slots/reason.rec\""))
        .expect_err("not at the root");
    assert!(
        err.contains("is not at the root of the boot partition"),
        "{err}"
    );
    // A record the build would have to supply: there is none to supply.
    let err = read(&LAYOUT.replacen(
        "role = \"trial\"\nfirmware = [\"rpi-tryboot\"]\nbytes = 1024\nsource = \"generated\"",
        "role = \"trial\"\nfirmware = [\"rpi-tryboot\"]\nbytes = 1024\nsource = \"build\"",
        1,
    ))
    .expect_err("a build's record");
    assert!(err.contains("is not the generator's"), "{err}");
    // A layout with no record table at all is an older file: refused.
    let cut = LAYOUT.split("[[record]]").next().expect("head").to_string()
        + "[[file]]\nname = \"extlinux/extlinux.conf\"\nrole = \"boot-menu\"\nsource = \"build\"\nfirmware = [\"uboot\"]\nrequired = true\n";
    let err = read(&cut).expect_err("no records");
    assert!(err.contains("missing field `record`"), "{err}");
}
