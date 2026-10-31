use super::*;

fn step(step: AssemblyStep, reads: &[&str], writes: &[&str]) -> AssemblyStepPaths {
    AssemblyStepPaths {
        step,
        reads: reads.iter().map(PathBuf::from).collect(),
        writes: writes.iter().map(PathBuf::from).collect(),
    }
}

#[test]
fn readers_run_after_writers_whatever_their_kind() {
    let steps = [
        step(
            AssemblyStep::File(0),
            &["/src/cmdline.txt"],
            &["/work/boot/cmdline.txt"],
        ),
        step(
            AssemblyStep::Transform(0),
            &["/images/boot.vfat"],
            &["/work/boot.vfat.zst"],
        ),
        step(
            AssemblyStep::Filesystem(0),
            &["/work/boot"],
            &["/images/boot.vfat"],
        ),
        step(
            AssemblyStep::Disk(0),
            &["/images/boot.vfat"],
            &["/out/disk.img"],
        ),
        step(
            AssemblyStep::Archive(0),
            &["/work/boot.vfat.zst"],
            &["/out/update.tar"],
        ),
    ];
    assert_eq!(
        order_assembly_steps(&steps),
        Ok(vec![
            AssemblyStep::File(0),
            AssemblyStep::Filesystem(0),
            AssemblyStep::Transform(0),
            AssemblyStep::Disk(0),
            AssemblyStep::Archive(0),
        ])
    );
}

#[test]
fn independent_steps_keep_the_classic_order() {
    let steps = [
        step(
            AssemblyStep::Disk(0),
            &["/images/rootfs.ext4"],
            &["/out/disk.img"],
        ),
        step(AssemblyStep::Transform(1), &["/a"], &["/b"]),
        step(AssemblyStep::Transform(0), &["/c"], &["/d"]),
    ];
    assert_eq!(
        order_assembly_steps(&steps),
        Ok(vec![
            AssemblyStep::Transform(0),
            AssemblyStep::Transform(1),
            AssemblyStep::Disk(0),
        ])
    );
}

#[test]
fn an_oci_image_runs_after_the_steps_that_write_its_layers() {
    let steps = vec![
        step(
            AssemblyStep::Oci(0),
            &["/out/base.tar", "/work/app"],
            &["/out/image.oci"],
        ),
        step(AssemblyStep::Transform(0), &[], &["/out/base.tar"]),
        step(AssemblyStep::File(0), &[], &["/work/app/bin/app"]),
    ];
    let order = order_assembly_steps(&steps).expect("order");
    assert_eq!(order.last(), Some(&AssemblyStep::Oci(0)));
    assert!(
        order
            .iter()
            .position(|step| *step == AssemblyStep::Transform(0))
            < order.iter().position(|step| *step == AssemblyStep::Oci(0))
    );
}

#[test]
fn cycles_are_refused() {
    let steps = [
        step(AssemblyStep::Transform(0), &["/b"], &["/a"]),
        step(AssemblyStep::Transform(1), &["/a"], &["/b"]),
        step(AssemblyStep::Dir(0), &[], &["/tree/x"]),
    ];
    assert_eq!(
        order_assembly_steps(&steps),
        Err(AssemblyCycle {
            steps: vec![AssemblyStep::Transform(0), AssemblyStep::Transform(1)]
        })
    );
}

#[test]
fn globs_read_only_what_they_match() {
    let dtbs = Path::new("$provider.images/*.dtb");
    assert!(overlaps(dtbs, Path::new("$provider.images/evk.dtb")));
    assert!(!overlaps(dtbs, Path::new("$provider.images/boot.vfat")));
    // The writer fills the directory the glob scans.
    assert!(overlaps(dtbs, Path::new("$provider.images")));
    assert!(overlaps(
        Path::new("$assembly.work/overlays/*/x"),
        Path::new("$assembly.work/overlays/a/x/y")
    ));
    // Plain paths: either inside the other.
    assert!(overlaps(
        Path::new("/work/boot"),
        Path::new("/work/boot/cmdline.txt")
    ));
    assert!(overlaps(
        Path::new("/work/boot/cmdline.txt"),
        Path::new("/work/boot")
    ));
    assert!(!overlaps(
        Path::new("/work/boot"),
        Path::new("/work/bootfs")
    ));
}

#[test]
fn a_boot_tree_filled_from_images_is_not_a_cycle() {
    // Boot files come from the provider's images; the boot filesystem
    // is written there too, under a name the glob does not match.
    let steps = [
        step(AssemblyStep::File(0), &["/images/*.dtb"], &["/work/boot"]),
        step(
            AssemblyStep::Filesystem(0),
            &["/work/boot"],
            &["/images/boot.vfat"],
        ),
    ];
    assert_eq!(
        order_assembly_steps(&steps),
        Ok(vec![AssemblyStep::File(0), AssemblyStep::Filesystem(0)])
    );
}

#[test]
fn kernel_modules_fill_the_tree_before_it_is_packed_and_published_copies_are_writes() {
    use crate::{
        AssemblyFileSpec, AssemblyFilesystemKindSpec, AssemblyFilesystemSpec,
        AssemblyKernelModulesSpec, AssemblyTreeSpec,
    };
    let assembly = ImageAssemblySpec {
        trees: vec![
            AssemblyTreeSpec {
                strip: false,
                id: "initramfs".into(),
                path: "/work/initramfs".into(),
            },
            AssemblyTreeSpec {
                strip: false,
                id: "boot".into(),
                path: "/work/boot".into(),
            },
        ],
        files: vec![AssemblyFileSpec {
            include: Vec::new(),
            strip: false,
            exclude: Vec::new(),
            tree: "boot".into(),
            src: Some("$assembly.out/boot.img".into()),
            src_glob: None,
            dest: "boot.img".into(),
            mode: None,
            dir_mode: None,
            optional: false,
            src_dir: None,
            preserve_symlink: false,
            allow_missing_libs: false,
            content: None,
            json: None,
        }],
        kernel_modules: vec![AssemblyKernelModulesSpec {
            metadata: None,
            tree: "initramfs".into(),
            from: "/provider/target/lib/modules".into(),
            kernel_version: None,
            modules: vec!["libcomposite".into()],
            depmod: None,
        }],
        filesystems: vec![AssemblyFilesystemSpec {
            free: None,
            id: "boot".into(),
            kind: AssemblyFilesystemKindSpec::CpioZstd,
            source_tree: "initramfs".into(),
            output: "/work/boot.img".into(),
            size: None,
            deterministic: true,
            compression_level: None,
            publish: true,
            label: None,
            uuid: None,
            journal: None,
            compressed: Default::default(),
            fat_size: None,
            reserved_percent: None,
        }],
        ..ImageAssemblySpec::default()
    };
    let paths = assembly_step_paths(
        &assembly,
        &|template| Some(PathBuf::from(template.as_str())),
        &|id| match id {
            "initramfs" => Some(PathBuf::from("/work/initramfs")),
            "boot" => Some(PathBuf::from("/work/boot")),
            _ => None,
        },
    );
    // The filesystem reads the tree the modules fill, and the file in
    // another tree reads the published copy the filesystem writes.
    assert_eq!(
        order_assembly_steps(&paths),
        Ok(vec![
            AssemblyStep::KernelModules(0),
            AssemblyStep::Filesystem(0),
            AssemblyStep::File(0),
        ])
    );
    assert!(
        AssemblyStep::KernelModules(0)
            .describe(&assembly)
            .contains("initramfs")
    );
}

#[test]
fn published_filesystem_images_go_to_the_output_dir_under_their_file_name() {
    use crate::{AssemblyFilesystemKindSpec, AssemblyFilesystemSpec};
    let mut filesystem = AssemblyFilesystemSpec {
        free: None,
        id: "boot".into(),
        kind: AssemblyFilesystemKindSpec::Vfat,
        source_tree: "boot".into(),
        output: "$assembly.work/sub/boot.img".into(),
        size: None,
        deterministic: false,
        compression_level: None,
        publish: false,
        label: None,
        uuid: None,
        journal: None,
        compressed: Default::default(),
        fat_size: None,
        reserved_percent: None,
    };
    assert_eq!(filesystem.publish_template(), None);
    filesystem.publish = true;
    assert_eq!(
        filesystem.publish_template().expect("published").as_str(),
        "$assembly.out/boot.img"
    );
}

/// A JSON tree digest reads its tree: the file waits for every step that
/// fills the tree, however it is declared.
#[test]
fn a_tree_digest_runs_after_the_steps_that_fill_its_tree() {
    use crate::{AssemblyFileSpec, AssemblyJsonValue, AssemblyTreeSpec};
    let file = |tree: &str, dest: &str, content: Option<&str>, json: Option<AssemblyJsonValue>| {
        AssemblyFileSpec {
            include: Vec::new(),
            strip: false,
            exclude: Vec::new(),
            tree: tree.into(),
            src: None,
            src_glob: None,
            content: content.map(str::to_string),
            json,
            dest: dest.into(),
            mode: None,
            dir_mode: None,
            optional: false,
            src_dir: None,
            preserve_symlink: false,
            allow_missing_libs: false,
        }
    };
    let assembly = ImageAssemblySpec {
        trees: vec![
            AssemblyTreeSpec {
                strip: false,
                id: "boot".into(),
                path: "/build/boot".into(),
            },
            AssemblyTreeSpec {
                strip: false,
                id: "payload".into(),
                path: "/build/payload".into(),
            },
        ],
        files: vec![
            file(
                "boot",
                "slot.json",
                None,
                Some(AssemblyJsonValue::Object(vec![(
                    "files".into(),
                    AssemblyJsonValue::String("${assembly.sha256_tree:payload}".into()),
                )])),
            ),
            file("payload", "a.txt", Some("alpha"), None),
        ],
        ..ImageAssemblySpec::default()
    };
    let paths = assembly_step_paths(
        &assembly,
        &|template| Some(PathBuf::from(template.as_str())),
        &|id| match id {
            "boot" => Some(PathBuf::from("/build/boot")),
            "payload" => Some(PathBuf::from("/build/payload")),
            _ => None,
        },
    );
    assert_eq!(
        order_assembly_steps(&paths),
        Ok(vec![AssemblyStep::File(1), AssemblyStep::File(0)])
    );
}

/// Generated text and JSON read the paths their tokens name, so a file
/// declared before the transform that writes its `${assembly.size:...}`
/// path still runs after it.
#[test]
fn generated_content_runs_after_the_steps_whose_outputs_its_tokens_read() {
    use crate::{
        AssemblyFileSpec, AssemblyJsonValue, AssemblyTransformKindSpec, AssemblyTransformSpec,
        AssemblyTreeSpec,
    };
    let assembly = ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![
            AssemblyFileSpec {
                include: Vec::new(),
                strip: false,
                exclude: Vec::new(),
                tree: "boot".into(),
                src: None,
                src_glob: None,
                content: Some(
                    "root=${assembly.verity:$assembly.work/rootfs.erofs:root_hash}".into(),
                ),
                json: None,
                dest: "cmdline.txt".into(),
                mode: None,
                dir_mode: None,
                optional: false,
                src_dir: None,
                preserve_symlink: false,
                allow_missing_libs: false,
            },
            AssemblyFileSpec {
                include: Vec::new(),
                strip: false,
                exclude: Vec::new(),
                tree: "boot".into(),
                src: None,
                src_glob: None,
                content: None,
                json: Some(AssemblyJsonValue::Object(vec![(
                    "size".into(),
                    AssemblyJsonValue::String(
                        "${assembly.size:$assembly.work/rootfs.erofs}".into(),
                    ),
                )])),
                dest: "state.json".into(),
                mode: None,
                dir_mode: None,
                optional: false,
                src_dir: None,
                preserve_symlink: false,
                allow_missing_libs: false,
            },
        ],
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Verity,
            src: Some("$provider.images/rootfs.erofs".into()),
            dest: "$assembly.work/rootfs.erofs".into(),
            deterministic: true,
            level: None,
            hash_dest: None,
            data_block_size: None,
            hash_block_size: None,
            salt: None,
            uuid: None,
            long: false,
            env_size: None,
            redundant: false,
        }],
        ..ImageAssemblySpec::default()
    };
    let paths = assembly_step_paths(
        &assembly,
        &|template| Some(PathBuf::from(template.as_str())),
        &|tree| Some(PathBuf::from(format!("/build/{tree}"))),
    );
    let order = order_assembly_steps(&paths).expect("no cycle");
    let transform = order
        .iter()
        .position(|step| *step == AssemblyStep::Transform(0))
        .expect("transform");
    assert!(
        order
            .iter()
            .position(|step| *step == AssemblyStep::File(0))
            .expect("file")
            > transform
    );
    assert!(
        order
            .iter()
            .position(|step| *step == AssemblyStep::File(1))
            .expect("json")
            > transform
    );
}

#[test]
fn a_glob_runs_after_a_file_copied_from_a_transforms_output() {
    // Atlas's slots.toml chain: the transform compiles boot/overlays/x.dtbo,
    // a file copies it into payload/overlays/, and the FAT tree's glob over
    // payload/overlays/*.dtbo is declared first.
    let steps = [
        step(
            AssemblyStep::File(0),
            &["/payload/overlays/*.dtbo"],
            &["/fat/overlays"],
        ),
        step(
            AssemblyStep::File(1),
            &["/src/config.txt"],
            &["/fat/config.txt"],
        ),
        step(
            AssemblyStep::File(2),
            &["/boot/overlays/x.dtbo"],
            &["/payload/overlays/x.dtbo"],
        ),
        step(
            AssemblyStep::File(3),
            &["/src/stock.dtbo"],
            &["/payload/overlays/stock.dtbo"],
        ),
        step(
            AssemblyStep::Transform(0),
            &["/src/x.dts"],
            &["/boot/overlays/x.dtbo"],
        ),
    ];
    let order = order_assembly_steps(&steps).expect("order");
    let at = |step: AssemblyStep| order.iter().position(|s| *s == step).expect("step");
    assert!(
        at(AssemblyStep::Transform(0)) < at(AssemblyStep::File(2)),
        "{order:?}"
    );
    assert!(
        at(AssemblyStep::File(2)) < at(AssemblyStep::File(0)),
        "{order:?}"
    );
    assert!(
        at(AssemblyStep::File(3)) < at(AssemblyStep::File(0)),
        "{order:?}"
    );
}

#[test]
fn a_src_dir_copy_runs_after_the_steps_writing_inside_its_directory() {
    // The copy reads its directory, so a file written into it runs first.
    let steps = [
        step(AssemblyStep::File(0), &["/work/site"], &["/out/web"]),
        step(
            AssemblyStep::File(1),
            &["/src/index.html"],
            &["/work/site/index.html"],
        ),
    ];
    assert_eq!(
        order_assembly_steps(&steps),
        Ok(vec![AssemblyStep::File(1), AssemblyStep::File(0)])
    );
}
