pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{ExecutionPlan, OperationId, OperationKind, OperationReuse, PlannedOperation};
use gaia_report::{
    OutputSizeRecord, ReportBundle, SizeChange, generate_report, write_report_bundle,
};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyPartitionTableSpec, ImageAssemblySpec,
    ResolvedBuildSpec,
};
use gaia_testkit::TempDir;
use gaia_validate::ValidationReport;
use std::fs;
use std::path::{Path, PathBuf};
use support::{provider_catalogs, test_spec};

/// A spec whose assembly builds one MBR disk, `sdcard.img`, in the collect
/// dir, from a rootfs image of `rootfs_bytes` bytes. Returns the disk's path.
fn spec_with_disk(scratch: &TempDir, rootfs_bytes: usize) -> (ResolvedBuildSpec, PathBuf) {
    let mut spec = test_spec(scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let collect_dir = Path::new(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    let rootfs_image = build_dir.join("rootfs.ext4");
    fs::create_dir_all(&build_dir).expect("build dir");
    fs::write(&rootfs_image, vec![0x22; rootfs_bytes]).expect("rootfs image");
    let sdcard = collect_dir.join("sdcard.img");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: sdcard.display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: Some("GAIA".into()),
            first_lba: None,
            alignment_lba: None,
            truncate: None,
            ebr_placement: gaia_spec::AssemblyEbrPlacementSpec::Default,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: Some("0x83".into()),
                type_alias: None,
                bootable: false,
                image: Some(rootfs_image.display().to_string().into()),
                size: None,
                wipe: false,
                materialize: true,
                gpt: Default::default(),
            }],
            gpt: Default::default(),
        }],
        ..ImageAssemblySpec::default()
    });
    (spec, sdcard)
}

/// Plans and runs the image assembly (or a reused one), then writes the
/// report bundle the way `gaia run` does.
fn run(spec: &ResolvedBuildSpec, reused: bool) -> ReportBundle {
    run_removing(spec, reused, &[])
}

/// As [`run`], but `removed` files are deleted once the run has ended, as a
/// RAM work dir's copies are.
fn run_removing(spec: &ResolvedBuildSpec, reused: bool, removed: &[&Path]) -> ReportBundle {
    let mut operation =
        PlannedOperation::new(OperationId::image_assembly(), OperationKind::AssembleImage);
    if reused {
        operation.reuse = OperationReuse::Reuse {
            source: "test: inputs unchanged".into(),
        };
    }
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![operation],
    };
    // The fixture is not a full build, so it is not validated: validation
    // errors would (correctly) stop a real run from keeping a baseline.
    let validation = ValidationReport::default();
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
    for path in removed {
        fs::remove_file(path).expect("removed output");
    }
    let report = generate_report(spec, &validation, &plan, &outcome);
    write_report_bundle(spec, &report).expect("report bundle written");
    report
}

fn size<'a>(report: &'a ReportBundle, name: &str) -> &'a OutputSizeRecord {
    report
        .summary
        .sizes
        .iter()
        .find(|record| record.name == name)
        .unwrap_or_else(|| panic!("no size record for {name}: {:?}", report.summary.sizes))
}

#[test]
fn second_run_reports_the_growth_since_the_first() {
    let scratch = TempDir::new("gaia-report-sizes");
    let (spec, sdcard) = spec_with_disk(&scratch, 2048);
    let first = run(&spec, false);
    let first_bytes = fs::metadata(&sdcard).expect("first disk").len();

    let first_record = size(&first, "sdcard.img");
    assert_eq!(first_record.kind, "disk");
    assert_eq!(first_record.change, SizeChange::New);
    assert_eq!(first_record.bytes, first_bytes);
    assert!(
        spec.workspace
            .out_path()
            .join(".gaia")
            .join("reports")
            .join(format!("{}.sizes.json", spec.report_name()))
            .is_file(),
        "a clean run writes the baseline"
    );

    let (spec, _) = spec_with_disk(&scratch, 64 * 1024);
    let second = run(&spec, false);
    let second_bytes = fs::metadata(&sdcard).expect("second disk").len();
    let second_record = size(&second, "sdcard.img");
    assert_eq!(
        second_record.change,
        SizeChange::Changed {
            previous_bytes: first_bytes
        }
    );
    assert_eq!(
        second_record.delta_bytes(),
        Some(second_bytes as i64 - first_bytes as i64)
    );
    assert!(second_record.delta_bytes().unwrap_or_default() > 0);
}

#[test]
fn reused_assembly_still_reports_its_outputs_unchanged() {
    let scratch = TempDir::new("gaia-report-sizes");
    let (spec, sdcard) = spec_with_disk(&scratch, 2048);
    let first = run(&spec, false);
    let bytes = fs::metadata(&sdcard).expect("disk").len();

    let reused = run(&spec, true);
    let record = size(&reused, "sdcard.img");
    assert_eq!(record.bytes, bytes);
    assert_eq!(record.change, SizeChange::Unchanged);
    assert_eq!(reused.summary.reused_operations, 1);
    // The outputs carry over, so `--export` still has an image to copy.
    assert!(first.summary.primary_image_output.is_some());
    assert_eq!(
        reused.summary.primary_image_output,
        first.summary.primary_image_output
    );
    assert_eq!(
        reused.summary.published_outputs,
        first.summary.published_outputs
    );
}

#[test]
fn failed_run_keeps_the_previous_baseline() {
    let scratch = TempDir::new("gaia-report-sizes");
    let (spec, sdcard) = spec_with_disk(&scratch, 2048);
    run(&spec, false);
    let baseline_bytes = fs::metadata(&sdcard).expect("disk").len();

    // Without its rootfs the assembly fails; the failed run must not become
    // the baseline the next clean run is compared with.
    let rootfs = PathBuf::from(&spec.workspace.build_dir).join("rootfs.ext4");
    fs::remove_file(&rootfs).expect("remove rootfs");
    let failed = run(&spec, false);
    assert!(failed.summary.error_count > 0, "{:?}", failed.summary);

    // The same rootfs rebuilds the same disk, so the clean run is compared with
    // the baseline from before the failure and is unchanged.
    let (spec, _) = spec_with_disk(&scratch, 2048);
    let clean = run(&spec, false);
    let record = size(&clean, "sdcard.img");
    assert_eq!(record.bytes, baseline_bytes);
    assert_eq!(record.change, SizeChange::Unchanged);
}

#[test]
fn reused_assembly_reports_its_outputs_before_any_baseline_exists() {
    let scratch = TempDir::new("gaia-report-sizes");
    let (spec, sdcard) = spec_with_disk(&scratch, 2048);
    run(&spec, false);
    let bytes = fs::metadata(&sdcard).expect("disk").len();

    // A run from before the baseline existed left only its summary behind.
    let baseline = spec
        .workspace
        .out_path()
        .join(".gaia")
        .join("reports")
        .join(format!("{}.sizes.json", spec.report_name()));
    fs::remove_file(&baseline).expect("baseline");

    let reused = run(&spec, true);
    let record = size(&reused, "sdcard.img");
    assert_eq!(record.bytes, bytes);
    assert_eq!(record.change, SizeChange::New);
}

/// Pseudo-random bytes of `len`, so a gzip of them grows with `len`.
fn noise(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u32).wrapping_mul(2_654_435_761) as u8)
        .collect()
}

/// An assembly whose initramfs (a cpio filesystem of the tree's `init` file)
/// and gzip transform of a kernel image both land in the assembly out dir.
/// The sources are written at the given sizes each time it is called.
fn spec_with_filesystem_and_transform(
    scratch: &TempDir,
    init_bytes: usize,
    kernel_bytes: usize,
) -> ResolvedBuildSpec {
    let mut spec = test_spec(scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let source_dir = build_dir.join("sources");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("init"), noise(init_bytes)).expect("init source");
    fs::write(source_dir.join("kernel"), noise(kernel_bytes)).expect("kernel source");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        trees: vec![gaia_spec::AssemblyTreeSpec {
            id: "initramfs".into(),
            path: "$assembly.work/initramfs".into(),
            strip: false,
        }],
        files: vec![gaia_spec::AssemblyFileSpec {
            exclude: Vec::new(),
            tree: "initramfs".into(),
            src: Some(source_dir.join("init").display().to_string().into()),
            src_glob: None,
            dest: "init".into(),
            mode: Some("0755".into()),
            dir_mode: None,
            optional: false,
            src_dir: None,
            preserve_symlink: false,
            allow_missing_libs: false,
            include: Vec::new(),
            strip: false,
            content: None,
            json: None,
        }],
        filesystems: vec![gaia_spec::AssemblyFilesystemSpec {
            free: None,
            id: "initramfs-cpio".into(),
            kind: gaia_spec::AssemblyFilesystemKindSpec::Cpio,
            source_tree: "initramfs".into(),
            output: "$assembly.out/initramfs.cpio".into(),
            size: None,
            deterministic: true,
            compression_level: None,
            publish: false,
            label: None,
            uuid: None,
            journal: None,
            compressed: Default::default(),
            fat_size: None,
            reserved_percent: None,
        }],
        transforms: vec![gaia_spec::AssemblyTransformSpec {
            kind: gaia_spec::AssemblyTransformKindSpec::Gzip,
            src: Some(source_dir.join("kernel").display().to_string().into()),
            // A kernel image by name, so the file-name heuristic alone would
            // call it a disk; the spec makes it a transform.
            dest: "$assembly.out/kernel8.img".into(),
            deterministic: true,
            level: None,
            hash_dest: None,
            data_block_size: None,
            hash_block_size: None,
            long: false,
            salt: None,
            uuid: None,
        }],
        ..ImageAssemblySpec::default()
    });
    spec
}

#[test]
fn filesystems_and_transforms_report_their_spec_kinds_and_growth() {
    let scratch = TempDir::new("gaia-report-sizes");
    let spec = spec_with_filesystem_and_transform(&scratch, 16 * 1024, 16 * 1024);
    let first = run(&spec, false);
    let cpio = size(&first, "initramfs.cpio");
    assert_eq!(cpio.kind, "filesystem");
    assert_eq!(cpio.change, SizeChange::New);
    let kernel = size(&first, "kernel8.img");
    assert_eq!(kernel.kind, "transform");
    assert_eq!(kernel.change, SizeChange::New);
    // Published under the out dir and named by the spec, each appears once.
    assert_eq!(
        first
            .summary
            .sizes
            .iter()
            .filter(|record| record.name == "kernel8.img")
            .count(),
        1,
        "{:?}",
        first.summary.sizes
    );

    let spec = spec_with_filesystem_and_transform(&scratch, 64 * 1024, 64 * 1024);
    let second = run(&spec, false);
    for name in ["initramfs.cpio", "kernel8.img"] {
        let record = size(&second, name);
        assert!(
            matches!(record.change, SizeChange::Changed { .. }),
            "{record:?}"
        );
        assert!(record.delta_bytes().unwrap_or_default() > 0, "{record:?}");
    }

    // A reused assembly still reports both outputs, unchanged.
    let reused = run(&spec, true);
    for name in ["initramfs.cpio", "kernel8.img"] {
        assert_eq!(size(&reused, name).change, SizeChange::Unchanged, "{name}");
    }
}

#[test]
fn outputs_removed_after_the_run_keep_their_recorded_sizes() {
    let scratch = TempDir::new("gaia-report-sizes");
    let spec = spec_with_filesystem_and_transform(&scratch, 16 * 1024, 16 * 1024);
    let first = run(&spec, false);
    let cpio = PathBuf::from(&size(&first, "initramfs.cpio").path);
    let kernel = PathBuf::from(&size(&first, "kernel8.img").path);

    // The run's outputs are gone by the time the report is made, as a RAM
    // work dir's copies are; their sizes were recorded as they were written.
    let spec = spec_with_filesystem_and_transform(&scratch, 64 * 1024, 64 * 1024);
    let second = run_removing(&spec, false, &[&cpio, &kernel]);
    assert!(!cpio.exists() && !kernel.exists());
    for name in ["initramfs.cpio", "kernel8.img"] {
        let record = size(&second, name);
        assert!(record.bytes > 0, "{record:?}");
        assert!(
            matches!(record.change, SizeChange::Changed { .. }),
            "{record:?}"
        );
        assert!(record.delta_bytes().unwrap_or_default() > 0, "{record:?}");
    }
    let cpio_bytes = size(&second, "initramfs.cpio").bytes;

    // A reused assembly writes nothing: its recorded sizes are reported
    // unchanged, though the files are still gone.
    let reused = run_removing(&spec, true, &[]);
    for name in ["initramfs.cpio", "kernel8.img"] {
        let record = size(&reused, name);
        assert_eq!(record.change, SizeChange::Unchanged, "{record:?}");
        assert_eq!(
            record.kind,
            if name == "kernel8.img" {
                "transform"
            } else {
                "filesystem"
            }
        );
    }
    assert_eq!(size(&reused, "initramfs.cpio").bytes, cpio_bytes);
}
