pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{AssemblyCheckSpec, ImageAssemblySpec, ResolvedBuildSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;
use support::{assembly_file, assembly_tree, provider_catalogs, test_spec};

/// An assembly with one tree, `boot`, holding `config.txt`, and `checks`.
fn spec_with_checks(scratch: &TempDir, checks: Vec<AssemblyCheckSpec>) -> ResolvedBuildSpec {
    let mut spec = test_spec(scratch);
    let source = Path::new(&spec.workspace.build_dir).join("sources/config.txt");
    fs::create_dir_all(source.parent().expect("source dir")).expect("source dir");
    fs::write(&source, "dtoverlay=ov9782\n").expect("source");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("disk".into()),
        trees: vec![assembly_tree("boot", "$assembly.work/boot")],
        files: vec![assembly_file(
            "boot",
            source.display().to_string(),
            "config.txt",
        )],
        checks,
        ..ImageAssemblySpec::default()
    });
    spec
}

fn check(id: &str, command: &[&str]) -> AssemblyCheckSpec {
    AssemblyCheckSpec {
        id: id.into(),
        command: command.iter().map(|arg| arg.to_string()).collect(),
        timeout_secs: Some(30),
    }
}

/// Runs the assembly; the error messages, empty when it passed.
fn assemble(spec: &ResolvedBuildSpec) -> Vec<String> {
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    outcome
        .errors
        .iter()
        .map(|error| format!("{error:?}"))
        .collect()
}

#[test]
fn a_check_sees_the_finished_tree_through_its_token() {
    let scratch = TempDir::new("gaia-assembly-checks-pass");
    let spec = spec_with_checks(
        &scratch,
        vec![check(
            "boot-config",
            &[
                "grep",
                "-q",
                "dtoverlay=ov9782",
                "$assembly.tree.boot/config.txt",
            ],
        )],
    );
    let errors = assemble(&spec);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_failing_check_fails_the_assembly_with_its_output() {
    let scratch = TempDir::new("gaia-assembly-checks-fail");
    let spec = spec_with_checks(
        &scratch,
        vec![check(
            "boot-overlays",
            &[
                "sh",
                "-c",
                "echo 'overlays/ov9782.dtbo missing where the firmware reads' >&2; exit 3",
            ],
        )],
    );
    let errors = assemble(&spec).join("\n");
    assert!(
        errors.contains("assembly check 'boot-overlays' failed"),
        "{errors}"
    );
    assert!(
        errors.contains("missing where the firmware reads"),
        "{errors}"
    );
}

/// Points RAM work dirs at a scratch base, so the test never touches the
/// real /dev/shm/gaia-<user>.
fn scratch_ram_base() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let base = std::env::temp_dir().join("gaia-test-ram-base");
        fs::create_dir_all(&base).expect("scratch RAM base");
        // SAFETY: set once, before any assembly of this test binary reads it.
        unsafe { std::env::set_var("GAIA_RAM_BASE", base) };
    });
}

#[test]
fn a_ram_assembly_leaves_its_boot_files_for_image_check() {
    scratch_ram_base();
    let scratch = TempDir::new("gaia-assembly-boot-snapshot");
    let mut spec = test_spec(&scratch);
    let sources = Path::new(&spec.workspace.build_dir).join("sources");
    fs::create_dir_all(&sources).expect("sources");
    for (name, body) in [
        ("config.txt", "include default.txt\ndtoverlay=ov9782\n"),
        ("default.txt", "os_prefix=slots/a/\n"),
        ("ov9782.dtbo", "dtbo"),
        ("README", "overlays\n"),
        ("kernel8.img", "a kernel the snapshot leaves out"),
    ] {
        fs::write(sources.join(name), body).expect("source file");
    }
    let src = |name: &str| sources.join(name).display().to_string();
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("ram".into()),
        trees: vec![assembly_tree("slots-fat", "$assembly.work/fat")],
        files: vec![
            assembly_file("slots-fat", src("config.txt"), "config.txt"),
            assembly_file("slots-fat", src("default.txt"), "default.txt"),
            assembly_file(
                "slots-fat",
                src("ov9782.dtbo"),
                "slots/a/overlays/ov9782.dtbo",
            ),
            assembly_file("slots-fat", src("README"), "slots/a/overlays/README"),
            assembly_file("slots-fat", src("kernel8.img"), "slots/a/kernel8.img"),
        ],
        ..ImageAssemblySpec::default()
    });
    let errors = assemble(&spec);
    assert!(errors.is_empty(), "{errors:?}");

    let copy = gaia_spec::boot_tree_snapshot_dir(&spec).join("slots-fat");
    for kept in [
        "config.txt",
        "default.txt",
        "slots/a/overlays/ov9782.dtbo",
        "slots/a/overlays/README",
    ] {
        assert!(copy.join(kept).is_file(), "{kept} is not in the snapshot");
    }
    assert!(!copy.join("slots/a/kernel8.img").exists());
}

/// Atlas's slots.toml chain: a transform writes `boot/overlays/ov9782.dtbo`,
/// a file copies it into `payload/overlays/`, and the FAT tree's glob over
/// `payload/overlays/*.dtbo` (declared first) must see it.
fn chain_spec(scratch: &TempDir, work_dir: &str) -> ResolvedBuildSpec {
    let mut spec = test_spec(scratch);
    let sources = Path::new(&spec.workspace.build_dir).join("sources");
    fs::create_dir_all(&sources).expect("sources");
    fs::write(sources.join("ov9782.dtbo"), "compiled overlay").expect("overlay");
    fs::write(sources.join("stock.dtbo"), "stock overlay").expect("stock");
    fs::write(sources.join("config.txt"), "dtoverlay=ov9782\n").expect("config");
    let src = |name: &str| sources.join(name).display().to_string();
    let mut glob = assembly_file("fat", "", "overlays/");
    glob.src = None;
    glob.src_glob = Some("$assembly.tree.payload/overlays/*.dtbo".into());
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(work_dir.into()),
        trees: vec![
            assembly_tree("boot", "$assembly.work/boot"),
            assembly_tree("payload", "$assembly.work/payload"),
            assembly_tree("fat", "$assembly.work/fat"),
        ],
        files: vec![
            glob,
            assembly_file("fat", src("config.txt"), "config.txt"),
            assembly_file(
                "payload",
                "$assembly.tree.boot/overlays/ov9782.dtbo",
                "overlays/ov9782.dtbo",
            ),
            assembly_file("payload", src("stock.dtbo"), "overlays/stock.dtbo"),
        ],
        transforms: vec![support::assembly_transform(
            gaia_spec::AssemblyTransformKindSpec::Copy,
            src("ov9782.dtbo"),
            "$assembly.tree.boot/overlays/ov9782.dtbo",
        )],
        ..ImageAssemblySpec::default()
    });
    spec
}

#[test]
fn a_glob_sees_a_file_copied_from_a_transforms_output() {
    scratch_ram_base();
    for work_dir in ["disk", "ram"] {
        let scratch = TempDir::new("gaia-assembly-glob-chain");
        let spec = chain_spec(&scratch, work_dir);
        let errors = assemble(&spec);
        assert!(errors.is_empty(), "{work_dir}: {errors:?}");
        let fat = gaia_spec::boot_tree_snapshot_dir(&spec).join("fat/overlays");
        for overlay in ["stock.dtbo", "ov9782.dtbo"] {
            assert!(
                fat.join(overlay).is_file(),
                "{work_dir}: the FAT tree's glob missed {overlay}"
            );
        }
    }
}

#[test]
fn a_tree_digest_lists_files_copied_from_a_transforms_output() {
    use gaia_spec::AssemblyJsonValue;
    scratch_ram_base();
    for work_dir in ["disk", "ram"] {
        let scratch = TempDir::new("gaia-assembly-digest-chain");
        let mut spec = chain_spec(&scratch, work_dir);
        let assembly = spec.image.assembly.as_mut().expect("assembly");
        // slot.json, declared before the payload's files, digests the payload.
        let mut slot = support::assembly_file("fat", "", "slot.json");
        slot.src = None;
        slot.json = Some(AssemblyJsonValue::Object(vec![(
            "boot".into(),
            AssemblyJsonValue::Object(vec![(
                "files".into(),
                AssemblyJsonValue::String("${assembly.sha256_tree:payload}".into()),
            )]),
        )]));
        assembly.files.insert(0, slot);
        // The finished slot.json, copied out by a check (a RAM tree is gone
        // after the assembly).
        let copied = scratch.join("slot.json");
        assembly.checks.push(check(
            "copy-slot-json",
            &[
                "cp",
                "$assembly.tree.fat/slot.json",
                copied.to_str().expect("utf-8 path"),
            ],
        ));
        let errors = assemble(&spec);
        assert!(errors.is_empty(), "{work_dir}: {errors:?}");
        let slot = fs::read_to_string(&copied).expect("slot.json copied by the check");
        for listed in ["overlays/ov9782.dtbo", "overlays/stock.dtbo"] {
            assert!(
                slot.contains(listed),
                "{work_dir}: slot.json misses {listed}: {slot}"
            );
        }
    }
}
