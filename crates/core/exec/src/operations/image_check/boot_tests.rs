//! The Raspberry Pi boot model on synthesized FAT trees: which overlays
//! directory each boot path reads, and what it must hold. The last test reads
//! the real Raze FAT tree and is ignored by default.

use super::super::kernel_modules::{Outcome, Severity};
use super::*;
use gaia_testkit::TempDir;

fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    std::fs::write(path, text).expect("write");
}

/// Reads the boot trees under `tree` (the tree itself is the one tree).
fn run(tree: &Path, pi_model: Option<&str>) -> (Outcome, Vec<OverlayDir>) {
    let mut out = Outcome::default();
    let dirs = check("build", &[tree.to_path_buf()], pi_model, &mut out);
    (out, dirs)
}

fn errors(out: &Outcome) -> Vec<String> {
    out.findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .map(|finding| finding.message.clone())
        .collect()
}

fn codes(out: &Outcome) -> Vec<&'static str> {
    out.findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .map(|finding| finding.code)
        .collect()
}

fn assert_error_containing(out: &Outcome, needle: &str) {
    let messages = errors(out);
    assert!(
        messages.iter().any(|message| message.contains(needle)),
        "no error contains {needle:?}: {messages:#?}"
    );
}

#[test]
fn a_slot_with_a_readme_is_the_directory_the_firmware_reads() {
    let scratch = TempDir::new("boot-readme");
    let tree = scratch.join("tree");
    put(
        &tree.join("config.txt"),
        "os_prefix=slots/a/\ndtoverlay=ov\n",
    );
    put(&tree.join("slots/a/overlays/README"), "readme");
    put(&tree.join("slots/a/overlays/ov.dtbo"), "");
    let (out, dirs) = run(&tree, None);
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));
    assert_eq!(dirs.len(), 1);
    assert_eq!(dirs[0].dir, tree.join("slots/a/overlays"));
    assert!(
        dirs[0].label.ends_with(": slots/a/overlays/"),
        "{}",
        dirs[0].label
    );
}

#[test]
fn a_slot_without_a_readme_is_ignored_and_a_missing_overlay_names_the_slot_copy() {
    let scratch = TempDir::new("boot-no-readme");
    let tree = scratch.join("tree");
    put(
        &tree.join("config.txt"),
        "os_prefix=slots/a/\ndtoverlay=ov9782\ndtoverlay=root\n",
    );
    put(&tree.join("slots/a/overlays/ov9782.dtbo"), "");
    put(&tree.join("overlays/root.dtbo"), "");
    let (out, dirs) = run(&tree, None);
    assert_eq!(dirs[0].dir, tree.join("overlays"));
    assert_error_containing(
        &out,
        "slots/a/overlays/ has no README, so the firmware ignores it and reads overlays/",
    );
    assert_error_containing(&out, "config.txt enables overlay ov9782");
    assert_error_containing(
        &out,
        "slots/a/overlays/ has ov9782.dtbo but no README, so the firmware reads overlays/",
    );
    assert_eq!(errors(&out).len(), 2, "{:?}", errors(&out));
}

#[test]
fn an_os_prefix_and_an_overlay_from_an_included_file_apply() {
    let scratch = TempDir::new("boot-include");
    let tree = scratch.join("tree");
    put(&tree.join("config.txt"), "include a.txt\n");
    put(
        &tree.join("a.txt"),
        "os_prefix=slots/a/\ninclude sub/b.txt\n",
    );
    put(&tree.join("sub/b.txt"), "dtoverlay=deep,param=1\n");
    put(&tree.join("slots/a/overlays/README"), "readme");
    put(&tree.join("slots/a/overlays/deep.dtbo"), "");
    let (out, _) = run(&tree, None);
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));

    // Without the slot's copy the included overlay is missing, named.
    std::fs::remove_file(tree.join("slots/a/overlays/deep.dtbo")).expect("remove");
    let (out, _) = run(&tree, None);
    assert_error_containing(&out, "config.txt enables overlay deep");
}

#[test]
fn a_missing_include_and_an_include_cycle_are_errors_not_panics() {
    let scratch = TempDir::new("boot-cycle");
    let tree = scratch.join("tree");
    put(
        &tree.join("config.txt"),
        "include a.txt\ninclude gone.txt\n",
    );
    put(&tree.join("a.txt"), "include b.txt\n");
    put(&tree.join("b.txt"), "include a.txt\n");
    let (out, _) = run(&tree, None);
    assert!(
        codes(&out).contains(&"boot_include_cycle"),
        "{:?}",
        codes(&out)
    );
    assert_error_containing(&out, "includes gone.txt, which is missing");
}

#[test]
fn tryboot_is_read_on_its_own_with_its_own_os_prefix() {
    let scratch = TempDir::new("boot-tryboot");
    let tree = scratch.join("tree");
    put(
        &tree.join("config.txt"),
        "os_prefix=slots/a/\ndtoverlay=ov\n",
    );
    put(
        &tree.join("tryboot.txt"),
        "os_prefix=slots/b/\ndtoverlay=ov\n",
    );
    put(&tree.join("slots/a/overlays/README"), "readme");
    put(&tree.join("slots/a/overlays/ov.dtbo"), "");
    put(&tree.join("slots/b/overlays/other.dtbo"), "");
    let (out, dirs) = run(&tree, None);
    // config.txt is satisfied by its slot; the trial slot has no README.
    assert_eq!(dirs.len(), 1, "only config.txt's path is a normal boot");
    assert_eq!(errors(&out).len(), 2, "{:?}", errors(&out));
    assert!(
        errors(&out)
            .iter()
            .all(|message| message.contains("tryboot.txt")),
        "{:?}",
        errors(&out)
    );
    assert_error_containing(&out, "tryboot.txt enables overlay ov");
    assert_error_containing(&out, "slots/b/overlays/ has no README");
}

#[test]
fn without_a_model_every_section_counts_and_with_one_the_filters_are_evaluated() {
    let scratch = TempDir::new("boot-filters");
    let tree = scratch.join("tree");
    let body = "[pi4]\ndtoverlay=p4\n[cm5]\ndtoverlay=c5\n[none]\ndtoverlay=n\n[all]\ndtoverlay=a\n[tryboot]\ndtoverlay=t\n";
    put(&tree.join("config.txt"), body);
    put(&tree.join("tryboot.txt"), body);
    put(&tree.join("overlays/c5.dtbo"), "");
    put(&tree.join("overlays/a.dtbo"), "");
    put(&tree.join("overlays/t.dtbo"), "");

    // No model named: p4 and n count too, so the missing ones are errors.
    let (out, _) = run(&tree, None);
    assert_error_containing(&out, "enables overlay p4");
    assert_error_containing(&out, "enables overlay n");

    // cm5: its own section and [all] count; [none] and pi4 do not; [tryboot]
    // only on the trial path (where t is present).
    let (out, _) = run(&tree, Some("cm5"));
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));

    // pi4: the cm5 section is off, so c5 is not needed; p4 is.
    let (out, _) = run(&tree, Some("pi4"));
    assert_error_containing(&out, "enables overlay p4");
    assert!(
        !errors(&out)
            .iter()
            .any(|message| message.contains("overlay c5")),
        "{:?}",
        errors(&out)
    );
    assert!(
        !errors(&out)
            .iter()
            .any(|message| message.contains("config.txt enables overlay t")),
        "{:?}",
        errors(&out)
    );
}

#[test]
fn overlay_prefix_moves_the_directory_with_the_same_readme_rule() {
    let scratch = TempDir::new("boot-overlay-prefix");
    let tree = scratch.join("tree");
    put(
        &tree.join("config.txt"),
        "os_prefix=slots/a/\noverlay_prefix=ovl/\ndtoverlay=ov\n",
    );
    put(&tree.join("slots/a/ovl/README"), "readme");
    put(&tree.join("slots/a/ovl/ov.dtbo"), "");
    let (out, dirs) = run(&tree, None);
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));
    assert_eq!(dirs[0].dir, tree.join("slots/a/ovl"));

    // Without the README the root's ovl/ is read.
    std::fs::remove_file(tree.join("slots/a/ovl/README")).expect("remove");
    put(&tree.join("ovl/ov.dtbo"), "");
    let (out, dirs) = run(&tree, None);
    assert_eq!(dirs[0].dir, tree.join("ovl"));
    assert_error_containing(&out, "slots/a/ovl/ has no README");
    assert!(
        !errors(&out)
            .iter()
            .any(|message| message.contains("enables overlay ov")),
        "{:?}",
        errors(&out)
    );
}

#[test]
fn the_root_fallback_must_hold_every_overlay_the_slot_ships() {
    let scratch = TempDir::new("boot-fallback");
    let tree = scratch.join("tree");
    put(
        &tree.join("config.txt"),
        "os_prefix=slots/a/\ndtoverlay=ov\n",
    );
    put(&tree.join("slots/a/overlays/README"), "readme");
    put(&tree.join("slots/a/overlays/ov.dtbo"), "");
    put(&tree.join("slots/a/overlays/extra.dtbo"), "");
    put(&tree.join("overlays/ov.dtbo"), "");
    let (out, _) = run(&tree, None);
    assert_error_containing(&out, "overlays/ lacks");
    assert_error_containing(&out, "extra.dtbo");
}

/// The real Raze FAT tree (read only). Both boot paths read the slot's
/// overlays/, which has no README: the firmware reads the root overlays/,
/// which lacks ov9782 and raze-fan. The other five overlays are in the root.
#[test]
#[ignore]
fn the_real_raze_fat_tree_reports_the_camera_and_fan_overlays() {
    let tree = PathBuf::from(
        "/run/media/sozo/bd1d96d9-fa81-4fac-b25e-193cfcac2dcb/Github/HeliOS-architecture-overhaul/gaia/build/helios-full-raze/assembly/slots/fat",
    );
    if !tree.join("config.txt").is_file() {
        eprintln!("no real Raze FAT tree; nothing checked");
        return;
    }
    let (out, dirs) = run(&tree, None);
    for message in errors(&out) {
        eprintln!("{message}");
    }
    let messages = errors(&out);
    // Rule 1 on each path, and the two missing overlays on each path.
    assert_eq!(messages.len(), 6, "{messages:#?}");
    for file in ["config.txt", "tryboot.txt"] {
        assert_error_containing(&out, &format!("{file} enables overlay ov9782"));
        assert_error_containing(&out, &format!("{file} enables overlay raze-fan"));
    }
    assert_error_containing(
        &out,
        "slots/helios-2026.2.0-styx-4ed03e19/overlays/ has ov9782.dtbo but no README, so the firmware reads overlays/",
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.contains("has no README, so the firmware ignores it"))
            .count(),
        2,
        "{messages:#?}"
    );
    for other in [
        "i2c1-pi5",
        "ws2812-pio",
        "vc4-kms-v3d-pi5",
        "ramoops-pi5",
        "dwc2",
    ] {
        assert!(
            !messages.iter().any(|message| message.contains(other)),
            "{other} is reported: {messages:#?}"
        );
    }
    assert_eq!(dirs.len(), 1);
    assert_eq!(dirs[0].dir, tree.join("overlays"));
}
