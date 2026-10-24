//! Kernel module checks on synthesized trees: tiny ELF objects written from
//! their fields, not committed binaries. The last test runs against a real
//! Buildroot output when one is present.

use super::elf::ElfObject;
use super::hardware;
use super::kernel_modules::{Finding, Inputs, Outcome, Severity, check};
use super::module::{module_name_of, parse_module};
use gaia_plan::OperationId;
use gaia_spec::ResolvedBuildSpec;
use gaia_testkit::TempDir;
use std::path::{Path, PathBuf};

const RELEASE: &str = "7.2.9-v8-16k";
const FLAGS: &str = "SMP preempt mod_unload modversions aarch64";
const SYMVERS_CRC_PRINTK: u32 = 0x1111_1111;
const SYMVERS_CRC_KMALLOC: u32 = 0x2222_2222;
const SYMVERS_CRC_HELPER_FN: u32 = 0x3333_3333;

/// One section of a synthesized object.
struct Section {
    name: &'static str,
    kind: u32,
    data: Vec<u8>,
    link: u32,
    entsize: u64,
}

fn section(name: &'static str, kind: u32, data: Vec<u8>) -> Section {
    Section {
        name,
        kind,
        data,
        link: 0,
        entsize: 0,
    }
}

/// A relocatable ELF64 little-endian object with the given sections (after
/// the null section, which it adds).
fn build_elf(sections: Vec<Section>) -> Vec<u8> {
    let mut names = vec![0u8];
    let mut name_offsets = Vec::new();
    let mut all: Vec<Section> = vec![section("", 0, Vec::new())];
    all.extend(sections);
    all.push(section(".shstrtab", 3, Vec::new()));
    for item in &all {
        name_offsets.push(names.len() as u32);
        names.extend_from_slice(item.name.as_bytes());
        names.push(0);
    }
    let shstrndx = all.len() - 1;
    let last = all.len() - 1;
    all[last].data = names;
    let mut body = Vec::new();
    let mut headers = Vec::new();
    for (index, item) in all.iter().enumerate() {
        while body.len() % 8 != 0 {
            body.push(0);
        }
        let offset = 64 + body.len() as u64;
        body.extend_from_slice(&item.data);
        headers.push((name_offsets[index], item, offset));
    }
    while body.len() % 8 != 0 {
        body.push(0);
    }
    let table = 64 + body.len() as u64;
    let mut out = Vec::new();
    out.extend_from_slice(b"\x7fELF\x02\x01\x01");
    out.extend_from_slice(&[0; 9]);
    out.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
    out.extend_from_slice(&183u16.to_le_bytes()); // EM_AARCH64
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // entry
    out.extend_from_slice(&0u64.to_le_bytes()); // phoff
    out.extend_from_slice(&table.to_le_bytes()); // shoff
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.extend_from_slice(&64u16.to_le_bytes()); // ehsize
    out.extend_from_slice(&0u16.to_le_bytes()); // phentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // phnum
    out.extend_from_slice(&64u16.to_le_bytes()); // shentsize
    out.extend_from_slice(&(all.len() as u16).to_le_bytes());
    out.extend_from_slice(&(shstrndx as u16).to_le_bytes());
    out.extend_from_slice(&body);
    out.resize(table as usize, 0);
    out.extend_from_slice(&[0; 64]); // the null section header
    for (index, item) in all.iter().enumerate().skip(1) {
        let (name, item_ref, offset) = &headers[index];
        out.extend_from_slice(&name.to_le_bytes());
        out.extend_from_slice(&item_ref.kind.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes()); // flags
        out.extend_from_slice(&0u64.to_le_bytes()); // addr
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(item.data.len() as u64).to_le_bytes());
        out.extend_from_slice(&item.link.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // info
        out.extend_from_slice(&8u64.to_le_bytes()); // addralign
        out.extend_from_slice(&item.entsize.to_le_bytes());
    }
    out
}

/// What a fixture module contains.
#[derive(Default)]
struct ModuleFixture<'a> {
    vermagic: Option<&'a str>,
    depends: &'a str,
    firmware: &'a [&'a str],
    /// `alias=` entries of `.modinfo` (device-table aliases).
    aliases: &'a [&'a str],
    /// `(name, crc)` entries of `__versions`.
    versions: Vec<(&'a str, u64)>,
    /// Undefined symbols; `true` marks a weak one.
    imports: Vec<(&'a str, bool)>,
    exports: Vec<&'a str>,
}

fn build_module(fixture: &ModuleFixture<'_>) -> Vec<u8> {
    let mut modinfo = Vec::new();
    let vermagic = fixture
        .vermagic
        .map(str::to_string)
        .unwrap_or_else(|| format!("{RELEASE} {FLAGS}"));
    for entry in [
        format!("vermagic={vermagic}"),
        format!("depends={}", fixture.depends),
        "license=GPL".to_string(),
    ]
    .into_iter()
    .chain(
        fixture
            .firmware
            .iter()
            .map(|firmware| format!("firmware={firmware}")),
    )
    .chain(fixture.aliases.iter().map(|alias| format!("alias={alias}")))
    {
        modinfo.extend_from_slice(entry.as_bytes());
        modinfo.push(0);
    }
    let mut versions = Vec::new();
    for (name, crc) in &fixture.versions {
        versions.extend_from_slice(&crc.to_le_bytes());
        let mut field = [0u8; 56];
        field[..name.len()].copy_from_slice(name.as_bytes());
        versions.extend_from_slice(&field);
    }
    let mut strings = vec![0u8];
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 24];
    for (name, weak) in &fixture.imports {
        let offset = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.push(if *weak { 0x20 } else { 0x10 }); // GLOBAL, weak or not; NOTYPE
        symtab.push(0);
        symtab.extend_from_slice(&0u16.to_le_bytes()); // SHN_UNDEF
        symtab.extend_from_slice(&[0; 16]);
    }
    for name in &fixture.exports {
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
    }
    // Section indices: 1 .modinfo, 2 __versions, 3 __ksymtab_strings,
    // 4 .strtab, 5 .symtab (linked to .strtab).
    build_elf(vec![
        section(".modinfo", 1, modinfo),
        section("__versions", 1, versions),
        section("__ksymtab_strings", 1, strings),
        section(".strtab", 3, strtab),
        Section {
            name: ".symtab",
            kind: 2,
            data: symtab,
            link: 4,
            entsize: 24,
        },
    ])
}

/// A kernel image as a shipped `Image` carries its banner.
fn kernel_image_bytes(release: &str) -> Vec<u8> {
    let mut bytes = vec![0u8; 64];
    bytes.extend_from_slice(
        format!("Linux version {release} (builder@host) #1 SMP PREEMPT\0").as_bytes(),
    );
    bytes.extend_from_slice(&[7u8; 32]);
    bytes
}

fn symvers_text() -> String {
    format!(
        "0x{SYMVERS_CRC_PRINTK:08x}\tprintk\tvmlinux\tEXPORT_SYMBOL\t\n\
         0x{SYMVERS_CRC_KMALLOC:08x}\tkmalloc\tvmlinux\tEXPORT_SYMBOL_GPL\t\n\
         0x{SYMVERS_CRC_HELPER_FN:08x}\thelper_fn\tkernel/drivers/helper\tEXPORT_SYMBOL\t\n"
    )
}

/// A tree shaped like Buildroot's output (`images/Image`,
/// `target/lib/modules/<release>`, `build/linux-custom/Module.symvers`) under
/// `<scratch>/build/image/buildroot-output`, so a spec whose build dir is
/// `<scratch>/build` finds it.
struct Tree {
    _scratch: TempDir,
    scratch: PathBuf,
    root: PathBuf,
}

impl Tree {
    fn new() -> Self {
        let scratch = TempDir::new("image-check");
        let root = scratch.join("build/image/buildroot-output");
        let tree = Self {
            scratch: scratch.path().to_path_buf(),
            _scratch: scratch,
            root,
        };
        tree.write("images/Image", &kernel_image_bytes(RELEASE));
        tree.write(
            "build/linux-custom/Module.symvers",
            symvers_text().as_bytes(),
        );
        tree.write(
            &format!("target/lib/modules/{RELEASE}/modules.builtin"),
            b"",
        );
        tree.write(
            &format!("target/lib/modules/{RELEASE}/kernel/drivers/helper.ko"),
            &build_module(&ModuleFixture {
                exports: vec!["helper_fn"],
                ..ModuleFixture::default()
            }),
        );
        tree.good_module(&ModuleFixture::default());
        tree.write(
            &format!("target/lib/modules/{RELEASE}/kernel/drivers/third.ko"),
            &build_module(&ModuleFixture::default()),
        );
        tree.write("target/lib/firmware/good.bin", b"firmware");
        tree
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, bytes).expect("write");
    }

    /// A spec whose workspace holds this tree.
    fn spec(&self) -> ResolvedBuildSpec {
        let mut spec = ResolvedBuildSpec::new("image-check-test");
        spec.workspace.root_dir = self.scratch.display().to_string();
        spec.workspace.build_dir = self.scratch.join("build").display().to_string();
        spec.workspace.out_dir = self.scratch.join("out").display().to_string();
        spec
    }

    /// `kernel/drivers/good.ko`: imports `printk`, a weak `kmalloc` and
    /// `helper_fn`, and depends on `helper`.
    fn good_module(&self, fixture: &ModuleFixture<'_>) {
        self.write(
            &format!("target/lib/modules/{RELEASE}/kernel/drivers/good.ko"),
            &build_module(fixture),
        );
    }

    fn good_fixture() -> ModuleFixture<'static> {
        ModuleFixture {
            depends: "helper",
            firmware: &["good.bin"],
            versions: vec![
                ("printk", u64::from(SYMVERS_CRC_PRINTK)),
                ("helper_fn", u64::from(SYMVERS_CRC_HELPER_FN)),
            ],
            imports: vec![("printk", false), ("kmalloc", true), ("helper_fn", false)],
            ..ModuleFixture::default()
        }
    }

    fn check(&self) -> Outcome {
        check_with(self, None, true)
    }
}

fn check_with(tree: &Tree, modules_dir: Option<&Path>, symvers: bool) -> Outcome {
    let kernel = tree.path("images/Image");
    let symvers_path = tree.path("build/linux-custom/Module.symvers");
    let modules_root = tree.path("target/lib/modules");
    let firmware = tree.path("target/lib/firmware");
    let images = tree.path("images");
    let rootfs = tree.path("target");
    check(&Inputs {
        kernel_image: &kernel,
        modules_root: &modules_root,
        modules_dir,
        modules_dir_set: modules_dir.is_some(),
        symvers: symvers.then_some(symvers_path.as_path()),
        symvers_set: false,
        firmware_dir: Some(&firmware),
        module_checks: true,
        hardware: hardware::Sources {
            build: "image-check-test",
            entries: &[],
            images_dir: &images,
            rootfs: &rootfs,
            assembly_trees: &[],
            removed: &std::collections::HashSet::new(),
            pi_model: None,
        },
    })
}

fn codes(findings: &[Finding], severity: Severity) -> Vec<(&'static str, Option<String>)> {
    findings
        .iter()
        .filter(|finding| finding.severity == severity)
        .map(|finding| (finding.code, finding.module.clone()))
        .collect()
}

fn errors_of(outcome: &Outcome) -> Vec<(&'static str, Option<String>)> {
    codes(&outcome.findings, Severity::Error)
}

fn warnings_of(outcome: &Outcome) -> Vec<(&'static str, Option<String>)> {
    codes(&outcome.findings, Severity::Warning)
}

fn message_of<'a>(outcome: &'a Outcome, code: &str) -> &'a str {
    outcome
        .findings
        .iter()
        .find(|finding| finding.code == code)
        .map(|finding| finding.message.as_str())
        .unwrap_or_else(|| panic!("no {code} finding in {:?}", outcome.findings))
}

#[test]
fn a_consistent_tree_has_no_errors_and_counts_what_it_checked() {
    let tree = Tree::new();
    tree.good_module(&Tree::good_fixture());
    let outcome = tree.check();
    assert_eq!(errors_of(&outcome), Vec::new(), "{:?}", outcome.findings);
    assert_eq!(warnings_of(&outcome), Vec::new(), "{:?}", outcome.findings);
    assert_eq!(outcome.release.as_deref(), Some(RELEASE));
    assert_eq!(outcome.modules, 3);
    // The weak `kmalloc` import is not counted: a weak import may stay unresolved.
    assert_eq!(outcome.imports, 2);
    assert!(outcome.symvers_checked);
}

#[test]
fn a_module_with_another_kernel_release_is_a_vermagic_error_naming_both() {
    let tree = Tree::new();
    tree.good_module(&ModuleFixture {
        vermagic: Some("7.1.0 SMP preempt mod_unload modversions aarch64"),
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert_eq!(
        errors_of(&outcome),
        vec![("vermagic_mismatch", Some("kernel/drivers/good.ko".into()))],
        "{:?}",
        outcome.findings
    );
    let message = message_of(&outcome, "vermagic_mismatch");
    assert!(message.contains("7.1.0"), "{message}");
    assert!(message.contains(RELEASE), "{message}");
}

#[test]
fn modules_disagreeing_on_vermagic_flags_name_the_odd_one_out() {
    let tree = Tree::new();
    tree.good_module(&ModuleFixture {
        vermagic: Some(&format!("{RELEASE} SMP mod_unload aarch64")),
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert_eq!(
        errors_of(&outcome),
        vec![(
            "vermagic_flags_differ",
            Some("kernel/drivers/good.ko".into())
        )],
        "{:?}",
        outcome.findings
    );
}

#[test]
fn an_import_nobody_exports_is_an_undefined_symbol() {
    let tree = Tree::new();
    tree.good_module(&ModuleFixture {
        imports: vec![("printk", false), ("nobody_exports_this", false)],
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert_eq!(
        errors_of(&outcome),
        vec![("undefined_symbol", Some("kernel/drivers/good.ko".into()))],
        "{:?}",
        outcome.findings
    );
    assert!(message_of(&outcome, "undefined_symbol").contains("nobody_exports_this"));
}

#[test]
fn a_kernel_export_by_a_module_that_is_not_shipped_is_an_error() {
    let tree = Tree::new();
    let symvers = format!(
        "{}0x44444444\tgone_fn\tdrivers/gone\tEXPORT_SYMBOL\t\n",
        symvers_text()
    );
    tree.write("build/linux-custom/Module.symvers", symvers.as_bytes());
    tree.good_module(&ModuleFixture {
        imports: vec![("gone_fn", false)],
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert_eq!(
        errors_of(&outcome),
        vec![("export_not_shipped", Some("kernel/drivers/good.ko".into()))],
        "{:?}",
        outcome.findings
    );
}

#[test]
fn a_crc_that_differs_from_the_kernel_is_an_abi_mismatch() {
    let tree = Tree::new();
    tree.good_module(&ModuleFixture {
        versions: vec![
            ("printk", 0x9999_9999),
            ("helper_fn", u64::from(SYMVERS_CRC_HELPER_FN)),
        ],
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert_eq!(
        errors_of(&outcome),
        vec![("abi_mismatch", Some("kernel/drivers/good.ko".into()))],
        "{:?}",
        outcome.findings
    );
    let message = message_of(&outcome, "abi_mismatch");
    assert!(
        message.contains("printk") && message.contains("ABI mismatch"),
        "{message}"
    );
}

#[test]
fn a_dependency_neither_shipped_nor_built_in_is_an_error() {
    let tree = Tree::new();
    tree.good_module(&ModuleFixture {
        depends: "absent_helper",
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert_eq!(
        errors_of(&outcome),
        vec![("dependency_missing", Some("kernel/drivers/good.ko".into()))],
        "{:?}",
        outcome.findings
    );
}

#[test]
fn a_dependency_listed_in_modules_builtin_is_satisfied() {
    let tree = Tree::new();
    tree.write(
        &format!("target/lib/modules/{RELEASE}/modules.builtin"),
        b"kernel/crypto/absent_helper.ko\n",
    );
    tree.good_module(&ModuleFixture {
        depends: "absent_helper",
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert!(errors_of(&outcome).is_empty(), "{:?}", outcome.findings);
}

#[test]
fn a_module_shipped_twice_warns_and_names_the_one_modprobe_loads() {
    let tree = Tree::new();
    tree.good_module(&Tree::good_fixture());
    let copy = build_module(&Tree::good_fixture());
    tree.write(
        &format!("target/lib/modules/{RELEASE}/updates/good.ko"),
        &copy,
    );
    tree.write(
        &format!("target/lib/modules/{RELEASE}/modules.dep"),
        b"kernel/drivers/helper.ko:\nupdates/good.ko: kernel/drivers/helper.ko\n",
    );
    let outcome = tree.check();
    assert!(errors_of(&outcome).is_empty(), "{:?}", outcome.findings);
    let message = message_of(&outcome, "module_duplicate");
    assert!(message.contains("updates/good.ko"), "{message}");
    assert!(message.contains("modules.dep"), "{message}");
    assert!(message.contains("kernel/drivers/good.ko"), "{message}");
}

#[test]
fn a_missing_firmware_file_is_a_warning() {
    let tree = Tree::new();
    tree.good_module(&ModuleFixture {
        firmware: &["absent.bin"],
        ..Tree::good_fixture()
    });
    let outcome = tree.check();
    assert!(errors_of(&outcome).is_empty(), "{:?}", outcome.findings);
    assert_eq!(
        warnings_of(&outcome),
        vec![("firmware_missing", Some("kernel/drivers/good.ko".into()))],
        "{:?}",
        outcome.findings
    );
}

#[test]
fn a_file_that_is_not_an_object_is_an_error_not_a_panic() {
    let tree = Tree::new();
    tree.write(
        &format!("target/lib/modules/{RELEASE}/kernel/drivers/junk.ko"),
        b"this is not an elf file",
    );
    let outcome = tree.check();
    assert_eq!(
        errors_of(&outcome),
        vec![("module_invalid", Some("kernel/drivers/junk.ko".into()))],
        "{:?}",
        outcome.findings
    );
}

#[test]
fn without_symvers_the_symbols_are_not_checked_and_that_is_a_warning() {
    let tree = Tree::new();
    tree.good_module(&ModuleFixture {
        imports: vec![("nobody_exports_this", false)],
        ..Tree::good_fixture()
    });
    let outcome = check_with(&tree, None, false);
    assert!(errors_of(&outcome).is_empty(), "{:?}", outcome.findings);
    assert_eq!(warnings_of(&outcome)[0].0, "symvers_missing");
    assert!(!outcome.symvers_checked);
}

#[test]
fn a_kernel_image_without_a_banner_is_an_error() {
    let tree = Tree::new();
    tree.write("images/Image", &[1u8; 128]);
    let outcome = tree.check();
    assert_eq!(errors_of(&outcome)[0].0, "kernel_release");
}

#[test]
fn a_modules_dir_that_is_set_but_missing_is_an_error() {
    let tree = Tree::new();
    let missing = tree.path("nowhere");
    let outcome = check_with(&tree, Some(&missing), true);
    assert_eq!(errors_of(&outcome)[0].0, "modules_dir_missing");
}

#[test]
fn an_xz_compressed_module_is_decompressed_and_checked() {
    if std::process::Command::new("xz")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let tree = Tree::new();
    let raw = tree.path("raw.ko");
    std::fs::write(&raw, build_module(&Tree::good_fixture())).expect("raw");
    let compressed = std::process::Command::new("xz")
        .arg("-c")
        .arg(&raw)
        .output()
        .expect("xz");
    assert!(compressed.status.success());
    std::fs::remove_file(tree.path(&format!(
        "target/lib/modules/{RELEASE}/kernel/drivers/good.ko"
    )))
    .expect("drop the plain copy");
    tree.write(
        &format!("target/lib/modules/{RELEASE}/kernel/drivers/good.ko.xz"),
        &compressed.stdout,
    );
    let outcome = tree.check();
    assert!(errors_of(&outcome).is_empty(), "{:?}", outcome.findings);
    assert_eq!(outcome.modules, 3);
}

#[test]
fn every_truncation_of_a_module_is_an_error_not_a_panic() {
    let bytes = build_module(&Tree::good_fixture());
    for length in 0..bytes.len() {
        let slice = &bytes[..length];
        let _ = ElfObject::parse(slice).and_then(|elf| elf.symbols().map(|_| ()));
        let _ = parse_module(slice, "x.ko".into(), "x".into());
    }
    assert!(parse_module(&bytes, "good.ko".into(), "good".into()).is_ok());
}

#[test]
fn module_names_drop_every_module_suffix_and_read_dash_as_underscore() {
    assert_eq!(module_name_of("ov9282.ko").as_deref(), Some("ov9282"));
    assert_eq!(module_name_of("ov-9282.ko.xz").as_deref(), Some("ov_9282"));
    assert_eq!(module_name_of("ov9282.ko.zst").as_deref(), Some("ov9282"));
    assert_eq!(module_name_of("ov9282.txt"), None);
}

/// The real Raze output, when this machine has it. Read only. Prints what it
/// found and how long the checks took.
#[test]
#[ignore]
fn the_real_buildroot_output_checks_in_well_under_a_second() {
    let root = PathBuf::from("/dev/shm/gaia-sozo/ca5bb1fdc719/buildroot-output");
    if !root.join("images/Image").is_file() {
        eprintln!("no real buildroot output at {}", root.display());
        return;
    }
    // The kernel's own `linux-*` build; out-of-tree packages have their own symvers.
    let symvers = super::find_symvers(&root.join("build"));
    let kernel = root.join("images/Image");
    let modules_root = root.join("target/lib/modules");
    let firmware = root.join("target/lib/firmware");
    let images = root.join("images");
    let rootfs = root.join("target");
    let started = std::time::Instant::now();
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
            entries: &[],
            images_dir: &images,
            rootfs: &rootfs,
            assembly_trees: &[],
            removed: &std::collections::HashSet::new(),
            pi_model: None,
        },
    });
    let elapsed = started.elapsed();
    eprintln!(
        "real tree: release {:?}, {} modules, {} imports, {:?}",
        outcome.release, outcome.modules, outcome.imports, elapsed
    );
    for finding in &outcome.findings {
        eprintln!(
            "{:?} {} {:?}: {}",
            finding.severity, finding.code, finding.module, finding.message
        );
    }
    assert_eq!(outcome.release.as_deref(), Some(RELEASE));
    assert_eq!(outcome.modules, 252);
}

/// The operation, run on the tree: the skip, pass, fail and warning paths.
fn execute_on(tree: &Tree) -> crate::operations::OperationExecutionResult {
    super::execute(&tree.spec(), &OperationId::image_check())
}

fn report_of(tree: &Tree) -> serde_json::Value {
    let text = std::fs::read_to_string(gaia_spec::image_checks_report_path(&tree.spec()))
        .expect("the report is written");
    serde_json::from_str(&text).expect("the report is JSON")
}

#[test]
fn a_buildroot_config_without_a_kernel_skips_the_checks_and_says_so() {
    let tree = Tree::new();
    tree.write(".config", b"BR2_LINUX_KERNEL=n\n");
    let result = execute_on(&tree);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(report_of(&tree)["status"], "skipped");
}

#[test]
fn a_clean_kernel_build_passes_and_reports_its_release() {
    let tree = Tree::new();
    tree.write(".config", b"BR2_LINUX_KERNEL=y\n");
    tree.good_module(&Tree::good_fixture());
    let result = execute_on(&tree);
    assert!(result.error.is_none(), "{:?}", result.error);
    let report = report_of(&tree);
    assert_eq!(report["status"], "passed");
    assert_eq!(report["kernel_modules"]["release"], RELEASE);
    assert_eq!(report["kernel_modules"]["modules"], 3);
    assert!(report["errors"].as_array().is_some_and(Vec::is_empty));
}

#[test]
fn an_error_fails_the_operation_and_the_report_keeps_it() {
    let tree = Tree::new();
    tree.write(".config", b"BR2_LINUX_KERNEL=y\n");
    tree.good_module(&ModuleFixture {
        vermagic: Some("7.1.0 SMP preempt mod_unload modversions aarch64"),
        ..Tree::good_fixture()
    });
    let result = execute_on(&tree);
    let error = result.error.expect("the run fails");
    assert!(
        error.message.contains("image checks failed with 1 error"),
        "{}",
        error.message
    );
    assert!(error.message.contains("vermagic"), "{}", error.message);
    let report = report_of(&tree);
    assert_eq!(report["status"], "failed");
    assert_eq!(report["errors"][0]["code"], "vermagic_mismatch");
}

#[test]
fn warnings_reach_the_run_through_the_image_result() {
    let tree = Tree::new();
    tree.write(".config", b"BR2_LINUX_KERNEL=y\n");
    tree.good_module(&Tree::good_fixture());
    tree.write(
        &format!("target/lib/modules/{RELEASE}/updates/good.ko"),
        &build_module(&Tree::good_fixture()),
    );
    let result = execute_on(&tree);
    assert!(result.error.is_none(), "{:?}", result.error);
    let warnings: Vec<&String> = result
        .image_results
        .iter()
        .flat_map(|image| &image.warnings)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("updates/good.ko"), "{warnings:?}");
    assert!(!result.image_results[0].emit_report);
}

/// The hardware list checks, in their own file.
#[path = "hardware_tests.rs"]
mod hardware_tests;
