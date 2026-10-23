//! The hardware list checks (`[[image.hardware]]`) on synthesized Buildroot
//! trees: overlays, board DTBs, rootfs files and firmware are written here,
//! as device trees are built below. The last test runs against the real Raze
//! output with Atlas's hardware list, and is ignored by default.

use super::super::fdt::{self, Fragment};
use super::*;
use gaia_spec::ImageHardwareSpec;
use std::time::Instant;

/// A device tree blob built node by node: enough of the format for the reader.
#[derive(Default)]
struct Dtb {
    structure: Vec<u8>,
    strings: Vec<u8>,
}

impl Dtb {
    fn begin(&mut self, name: &str) {
        self.word(1);
        self.structure.extend_from_slice(name.as_bytes());
        self.structure.push(0);
        self.align();
    }

    fn end(&mut self) {
        self.word(2);
    }

    fn strings(&mut self, name: &str, values: &[&str]) {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        self.prop(name, &bytes);
    }

    fn prop(&mut self, name: &str, value: &[u8]) {
        let offset = self.name_offset(name);
        self.word(3);
        self.word(value.len() as u32);
        self.word(offset);
        self.structure.extend_from_slice(value);
        self.align();
    }

    fn word(&mut self, value: u32) {
        self.structure.extend_from_slice(&value.to_be_bytes());
    }

    fn align(&mut self) {
        while !self.structure.len().is_multiple_of(4) {
            self.structure.push(0);
        }
    }

    fn name_offset(&mut self, name: &str) -> u32 {
        let mut needle = name.as_bytes().to_vec();
        needle.push(0);
        if let Some(found) = self
            .strings
            .windows(needle.len())
            .position(|window| window == needle.as_slice())
        {
            return found as u32;
        }
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(&needle);
        offset
    }

    fn finish(mut self) -> Vec<u8> {
        self.word(9);
        // The header, then one empty memory reservation entry.
        let struct_offset = 40 + 16;
        let strings_offset = struct_offset + self.structure.len();
        let total = strings_offset + self.strings.len();
        let mut out = Vec::with_capacity(total);
        for word in [
            0xd00d_feed,
            total as u32,
            struct_offset as u32,
            strings_offset as u32,
            40,
            17,
            16,
            0,
            self.strings.len() as u32,
            self.structure.len() as u32,
        ] {
            out.extend_from_slice(&word.to_be_bytes());
        }
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&self.structure);
        out.extend_from_slice(&self.strings);
        out
    }
}

/// An overlay whose one fragment, by path, sets `compatible` on a node.
fn overlay_dtb(compatible: &str) -> Vec<u8> {
    let mut dtb = Dtb::default();
    dtb.begin("");
    dtb.begin("fragment@0");
    dtb.strings("target-path", &["/soc"]);
    dtb.begin("__overlay__");
    dtb.begin("widget@0");
    dtb.strings("compatible", &[compatible]);
    dtb.end();
    dtb.end();
    dtb.end();
    dtb.end();
    dtb.finish()
}

/// A board DTB: its root names a board, and a child node names each compatible.
fn board_dtb(compatibles: &[&str]) -> Vec<u8> {
    let mut dtb = Dtb::default();
    dtb.begin("");
    dtb.strings("compatible", &["acme,board"]);
    dtb.begin("soc");
    for (index, compatible) in compatibles.iter().enumerate() {
        dtb.begin(&format!("device@{index}"));
        dtb.strings("compatible", &[compatible]);
        dtb.end();
    }
    dtb.end();
    dtb.end();
    dtb.finish()
}

/// A tree that passes every check the widget entry makes: the kernel build,
/// a module `good` claiming `acme,widget`, an overlay naming it, and a rootfs
/// file and firmware.
fn passing_tree() -> Tree {
    let tree = Tree::new();
    tree.write(".config", b"BR2_LINUX_KERNEL=y\n");
    tree.good_module(&ModuleFixture {
        aliases: &["of:N*T*Cacme,widget"],
        ..Tree::good_fixture()
    });
    tree.write(
        "images/rpi-firmware/overlays/widget.dtbo",
        &overlay_dtb("acme,widget"),
    );
    tree.write("target/usr/share/widget.json", b"{}");
    tree
}

fn widget_entry() -> ImageHardwareSpec {
    ImageHardwareSpec {
        id: "widget".into(),
        kind: "sensor".into(),
        modules: vec!["good".into()],
        compatible: vec!["acme,widget".into()],
        overlays: vec!["widget".into()],
        files: vec!["/usr/share/widget.json".into()],
        firmware: vec!["good.bin".into()],
    }
}

/// Runs the operation on the tree with these hardware entries.
fn run_with(
    tree: &Tree,
    entries: Vec<ImageHardwareSpec>,
) -> crate::operations::OperationExecutionResult {
    let mut spec = tree.spec();
    spec.image.hardware = entries;
    super::super::execute(&spec, &gaia_plan::OperationId::image_check())
}

/// The messages of the report's errors.
fn error_messages(tree: &Tree) -> Vec<String> {
    report_of(tree)["errors"]
        .as_array()
        .expect("errors")
        .iter()
        .map(|finding| finding["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn assert_error_containing(tree: &Tree, needle: &str) {
    let messages = error_messages(tree);
    assert!(
        messages.iter().any(|message| message.contains(needle)),
        "no error contains {needle:?}: {messages:#?}"
    );
}

#[test]
fn a_hardware_entry_whose_whole_chain_holds_passes() {
    let tree = passing_tree();
    let result = run_with(&tree, vec![widget_entry()]);
    assert!(result.error.is_none(), "{:?}", result.error);
    let report = report_of(&tree);
    assert_eq!(report["status"], "passed");
    assert_eq!(report["hardware_entries"], 1);
    assert!(
        error_messages(&tree).is_empty(),
        "{:?}",
        error_messages(&tree)
    );
}

#[test]
fn a_compatible_no_driver_claims_is_an_error_naming_the_build_and_entry() {
    let tree = passing_tree();
    let mut entry = widget_entry();
    entry.compatible = vec!["acme,nothing".into()];
    run_with(&tree, vec![entry]);
    assert_error_containing(
        &tree,
        "image-check-test: widget: compatible acme,nothing is claimed by no shipped or built-in driver",
    );
}

#[test]
fn a_compatible_that_no_overlay_or_board_names_is_an_error() {
    let tree = passing_tree();
    tree.write(
        "images/rpi-firmware/overlays/widget.dtbo",
        &overlay_dtb("acme,other"),
    );
    run_with(&tree, vec![widget_entry()]);
    assert_error_containing(
        &tree,
        "widget: compatible acme,widget is named by no overlay of this entry and no shipped board dtb",
    );
}

#[test]
fn a_board_dtb_that_names_the_compatible_satisfies_it_without_an_overlay() {
    let tree = passing_tree();
    tree.write("images/board.dtb", &board_dtb(&["acme,widget"]));
    let mut entry = widget_entry();
    entry.overlays = Vec::new();
    let result = run_with(&tree, vec![entry]);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(report_of(&tree)["status"], "passed");
}

#[test]
fn an_empty_module_list_lets_any_shipped_driver_claim_the_compatible() {
    let tree = passing_tree();
    let mut entry = widget_entry();
    entry.modules = Vec::new();
    let result = run_with(&tree, vec![entry]);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        error_messages(&tree).is_empty(),
        "{:?}",
        error_messages(&tree)
    );
}

#[test]
fn a_built_in_driver_is_found_in_modules_builtin_and_its_modinfo_aliases() {
    let tree = passing_tree();
    tree.write(
        &format!("target/lib/modules/{RELEASE}/modules.builtin"),
        b"kernel/drivers/usb/dwc2/dwc2.ko\n",
    );
    tree.write(
        &format!("target/lib/modules/{RELEASE}/modules.builtin.modinfo"),
        b"dwc2.license=GPL\0dwc2.alias=of:N*T*Cbrcm,bcm2835-usb\0",
    );
    tree.write("images/board.dtb", &board_dtb(&["brcm,bcm2835-usb"]));
    let entry = ImageHardwareSpec {
        id: "usb".into(),
        kind: "usb-device".into(),
        modules: vec!["dwc2".into()],
        compatible: vec!["brcm,bcm2835-usb".into()],
        ..ImageHardwareSpec::default()
    };
    let result = run_with(&tree, vec![entry]);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        error_messages(&tree).is_empty(),
        "{:?}",
        error_messages(&tree)
    );
}

#[test]
fn a_named_module_that_fails_its_kernel_checks_is_an_error_on_the_entry() {
    let tree = passing_tree();
    tree.good_module(&ModuleFixture {
        vermagic: Some("7.1.0 SMP preempt mod_unload modversions aarch64"),
        aliases: &["of:N*T*Cacme,widget"],
        ..Tree::good_fixture()
    });
    run_with(&tree, vec![widget_entry()]);
    assert_error_containing(
        &tree,
        "widget: module good (kernel/drivers/good.ko) fails its kernel checks: vermagic_mismatch",
    );
}

#[test]
fn a_named_module_nobody_ships_is_an_error() {
    let tree = passing_tree();
    let mut entry = widget_entry();
    entry.modules = vec!["nosuch".into()];
    run_with(&tree, vec![entry]);
    assert_error_containing(
        &tree,
        "widget: module nosuch is neither shipped nor built in",
    );
}

#[test]
fn a_duplicate_module_is_checked_as_the_copy_modprobe_loads() {
    let tree = passing_tree();
    // The updates copy has no alias. Without modules.dep, depmod searches
    // `updates` first, so modprobe loads it and it claims nothing.
    tree.write(
        &format!("target/lib/modules/{RELEASE}/updates/good.ko"),
        &build_module(&Tree::good_fixture()),
    );
    run_with(&tree, vec![widget_entry()]);
    assert_error_containing(
        &tree,
        "widget: compatible acme,widget is not claimed by good (the module modprobe loads)",
    );

    // When modules.dep lists the kernel copy, that is the one modprobe loads.
    tree.write(
        &format!("target/lib/modules/{RELEASE}/modules.dep"),
        b"kernel/drivers/good.ko:\nupdates/good.ko:\n",
    );
    let result = run_with(&tree, vec![widget_entry()]);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        error_messages(&tree).is_empty(),
        "{:?}",
        error_messages(&tree)
    );
}

#[test]
fn an_overlay_that_does_not_ship_is_an_error() {
    let tree = passing_tree();
    let mut entry = widget_entry();
    entry.overlays = vec!["gone".into()];
    run_with(&tree, vec![entry]);
    assert_error_containing(
        &tree,
        "widget: overlay gone.dtbo is in no boot overlays dir",
    );
}

#[test]
fn a_missing_rootfs_file_or_firmware_is_an_error() {
    let tree = passing_tree();
    let mut entry = widget_entry();
    entry.files = vec!["/usr/share/missing.json".into()];
    entry.firmware = vec!["nope.bin".into()];
    run_with(&tree, vec![entry]);
    assert_error_containing(
        &tree,
        "widget: file /usr/share/missing.json is missing from the rootfs",
    );
    assert_error_containing(
        &tree,
        "widget: firmware nope.bin is missing from lib/firmware",
    );
}

#[test]
fn a_dtoverlay_in_config_txt_without_its_dtbo_is_an_error() {
    let tree = passing_tree();
    tree.write(
        "images/rpi-firmware/config.txt",
        b"kernel=kernel8.img\ndtoverlay=widget,param=1\ndtoverlay=gone\n# dtoverlay=commented\n",
    );
    let result = run_with(&tree, Vec::new());
    assert!(result.error.is_some());
    let messages = error_messages(&tree);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("config.txt enables overlay gone"),
        "{messages:?}"
    );
}

#[test]
fn with_the_kernel_module_checks_off_the_hardware_list_is_still_checked() {
    let tree = passing_tree();
    // A faulty module the module checks would reject.
    tree.good_module(&ModuleFixture {
        vermagic: Some("7.1.0 SMP preempt mod_unload modversions aarch64"),
        aliases: &["of:N*T*Cacme,widget"],
        ..Tree::good_fixture()
    });
    let mut spec = tree.spec();
    spec.image.hardware = vec![widget_entry()];
    spec.image.checks.kernel_modules = Some(false);
    let result = super::super::execute(&spec, &gaia_plan::OperationId::image_check());
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(report_of(&tree)["status"], "passed");
}

#[test]
fn a_hardware_list_without_a_kernel_is_an_error_not_a_skip() {
    let tree = passing_tree();
    tree.write(".config", b"BR2_LINUX_KERNEL=n\n");
    let result = run_with(&tree, vec![widget_entry()]);
    let error = result.error.expect("the run fails");
    assert!(
        error.message.contains("cannot be checked"),
        "{}",
        error.message
    );
}

#[test]
fn a_device_tree_reads_back_its_fragments_and_compatibles() {
    let bytes = overlay_dtb("acme,widget");
    let root = fdt::parse(&bytes).expect("the overlay parses");
    assert_eq!(
        fdt::overlay_fragments(&root),
        vec![Fragment {
            name: "fragment@0".into(),
            target_path: Some("/soc".into()),
            compatibles: vec!["acme,widget".into()],
        }]
    );
    assert_eq!(root.compatibles(), vec!["acme,widget".to_string()]);
    let board = fdt::parse(&board_dtb(&["a,one", "b,two"])).expect("the board parses");
    assert_eq!(board.compatibles(), vec!["acme,board", "a,one", "b,two"]);
}

#[test]
fn a_truncated_or_corrupt_device_tree_is_an_error_not_a_panic() {
    let bytes = overlay_dtb("acme,widget");
    for length in 0..bytes.len() {
        assert!(
            fdt::parse(&bytes[..length]).is_err(),
            "length {length} parses"
        );
    }
    for index in 0..bytes.len() {
        let mut corrupt = bytes.clone();
        corrupt[index] ^= 0xa5;
        let _ = fdt::parse(&corrupt);
    }
    let mut wrong_magic = bytes.clone();
    wrong_magic[0] ^= 0xff;
    assert!(fdt::parse(&wrong_magic).is_err());
}

/// The `[[image.hardware]]` tables of a hardware list. A small reader for the
/// shape Atlas writes: string values and string arrays, `#` comment lines.
fn parse_hardware_list(text: &str) -> Vec<ImageHardwareSpec> {
    let mut entries: Vec<ImageHardwareSpec> = Vec::new();
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    while let Some(line) = lines.next() {
        if line == "[[image.hardware]]" {
            entries.push(ImageHardwareSpec::default());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_string();
        // An array that runs over several lines: join them up to the bracket.
        while value.starts_with('[') && !value.contains(']') {
            let Some(next) = lines.next() else {
                break;
            };
            value.push_str(next);
        }
        // Top-level keys come before the first table.
        let Some(entry) = entries.last_mut() else {
            continue;
        };
        let strings: Vec<String> = value
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        match key.trim() {
            "id" => entry.id = strings.first().cloned().unwrap_or_default(),
            "kind" => entry.kind = strings.first().cloned().unwrap_or_default(),
            "modules" => entry.modules = strings,
            "compatible" => entry.compatible = strings,
            "overlays" => entry.overlays = strings,
            "files" => entry.files = strings,
            "firmware" => entry.firmware = strings,
            other => panic!("unknown key {other} in the hardware list"),
        }
    }
    entries
}

/// The module checks and the hardware list, run on the tree with these
/// assembly trees and rootfs exclusions.
fn check_with_sources(
    tree: &Tree,
    entries: &[ImageHardwareSpec],
    assembly: &[PathBuf],
    removed: &std::collections::HashSet<String>,
) -> Outcome {
    let kernel = tree.path("images/Image");
    let symvers_path = tree.path("build/linux-custom/Module.symvers");
    let modules_root = tree.path("target/lib/modules");
    let firmware = tree.path("target/lib/firmware");
    let images = tree.path("images");
    let rootfs = tree.path("target");
    check(&Inputs {
        kernel_image: &kernel,
        modules_root: &modules_root,
        modules_dir: None,
        modules_dir_set: false,
        symvers: Some(symvers_path.as_path()),
        symvers_set: false,
        firmware_dir: Some(&firmware),
        module_checks: true,
        hardware: hardware::Sources {
            build: "image-check-test",
            entries,
            images_dir: &images,
            rootfs: &rootfs,
            assembly_trees: assembly,
            removed,
        },
    })
}

/// The messages of the hardware findings.
fn hardware_findings(outcome: &Outcome) -> Vec<String> {
    outcome
        .findings
        .iter()
        .filter(|finding| finding.code.starts_with("hardware_"))
        .map(|finding| finding.message.clone())
        .collect()
}

#[test]
fn an_entry_overlay_in_an_assembly_tree_is_found() {
    let tree = passing_tree();
    let boot = tree.scratch.join("assembly/boot");
    std::fs::create_dir_all(boot.join("overlays")).expect("dirs");
    std::fs::write(boot.join("overlays/asm.dtbo"), overlay_dtb("acme,widget")).expect("write");
    let mut entry = widget_entry();
    entry.overlays = vec!["asm".into()];
    let outcome = check_with_sources(&tree, &[entry], &[boot], &Default::default());
    assert!(
        hardware_findings(&outcome).is_empty(),
        "{:?}",
        hardware_findings(&outcome)
    );
}

#[test]
fn a_config_txt_in_an_assembly_tree_is_checked_against_its_own_overlays_dir() {
    let tree = passing_tree();
    let boot = tree.scratch.join("assembly/boot");
    std::fs::create_dir_all(boot.join("overlays")).expect("dirs");
    std::fs::write(boot.join("overlays/asm.dtbo"), overlay_dtb("acme,widget")).expect("write");
    std::fs::write(
        boot.join("config.txt"),
        b"dtoverlay=asm\ndtoverlay=nothere\n",
    )
    .expect("write");
    let outcome = check_with_sources(&tree, &[], std::slice::from_ref(&boot), &Default::default());
    let messages = hardware_findings(&outcome);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("enables overlay nothere"),
        "{messages:?}"
    );
    assert!(
        messages[0].contains("assembly/boot/config.txt"),
        "{messages:?}"
    );
}

#[test]
fn leaving_a_package_out_removes_its_files_unless_a_kept_package_lists_them() {
    let tree = passing_tree();
    tree.write(
        "build/packages-file-list.txt",
        b"gone,./usr/share/gone.txt\ngone,./usr/share/widget.json\nkeep,./usr/share/widget.json\nkeep,./usr/bin/keep\n",
    );
    tree.write("target/usr/share/gone.txt", b"gone");
    let removed = hardware::rootfs_exclusions(&tree.root, &["gone".to_string()]);
    assert_eq!(
        removed,
        std::collections::HashSet::from(["usr/share/gone.txt".to_string()])
    );
    let mut entry = widget_entry();
    entry.files = vec![
        "/usr/share/gone.txt".into(),
        "/usr/share/widget.json".into(),
    ];
    let outcome = check_with_sources(&tree, &[entry], &[], &removed);
    let messages = hardware_findings(&outcome);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("file /usr/share/gone.txt is missing from the rootfs"),
        "{messages:?}"
    );
}

/// The real Raze output and Atlas's hardware list. Read only. Prints every
/// hardware finding, and how long the checks took.
#[test]
#[ignore]
fn the_real_raze_output_checks_atlas_hardware_list() {
    let root = PathBuf::from("/dev/shm/gaia-sozo/ca5bb1fdc719/buildroot-output");
    let list = PathBuf::from(
        "/run/media/sozo/bd1d96d9-fa81-4fac-b25e-193cfcac2dcb/Github/Atlas-Hardware-Manager/devices/raze/gaia/hardware.toml",
    );
    if !root.join("images/Image").is_file() || !list.is_file() {
        eprintln!("no real Raze output or no hardware list; nothing checked");
        return;
    }
    let text = std::fs::read_to_string(&list).expect("the hardware list is readable");
    let entries = parse_hardware_list(&text);
    assert!(!entries.is_empty(), "the hardware list has entries");
    // The assembly's boot tree (the board's config.txt, overlays and DTBs).
    let boot = PathBuf::from(
        "/run/media/sozo/bd1d96d9-fa81-4fac-b25e-193cfcac2dcb/Github/HeliOS-architecture-overhaul/gaia/build/helios-full-raze/assembly/boot",
    );
    let assembly_trees: Vec<PathBuf> = if boot.is_dir() {
        vec![boot]
    } else {
        Vec::new()
    };
    let symvers = super::super::find_symvers(&root.join("build"));
    let kernel = root.join("images/Image");
    let modules_root = root.join("target/lib/modules");
    let firmware = root.join("target/lib/firmware");
    let images = root.join("images");
    let rootfs = root.join("target");
    let started = Instant::now();
    let outcome = check(&Inputs {
        kernel_image: &kernel,
        modules_root: &modules_root,
        modules_dir: None,
        modules_dir_set: false,
        symvers: symvers.as_deref(),
        symvers_set: false,
        firmware_dir: Some(&firmware),
        module_checks: true,
        hardware: hardware::Sources {
            build: "raze",
            entries: &entries,
            images_dir: &images,
            rootfs: &rootfs,
            assembly_trees: &assembly_trees,
            removed: &std::collections::HashSet::new(),
        },
    });
    let elapsed = started.elapsed();
    eprintln!(
        "atlas hardware list: {} entries, {} findings in {elapsed:?}",
        entries.len(),
        outcome.findings.len()
    );
    for finding in &outcome.findings {
        eprintln!(
            "{:?} {}: {}",
            finding.severity, finding.code, finding.message
        );
    }
    assert_eq!(outcome.release.as_deref(), Some(RELEASE));
}
