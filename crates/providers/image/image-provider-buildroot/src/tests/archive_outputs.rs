use super::*;

fn raw_buildroot_image(names: &[&str]) -> ImageSpec {
    ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        expected_images: names
            .iter()
            .map(|name| BuildrootExpectedImageSpec {
                name: (*name).into(),
                format: BuildrootExpectedImageFormatSpec::Raw,
                required: true,
            })
            .collect(),
        ..BuildrootImageSpec::default()
    }))
}

#[test]
fn a_non_tar_archive_name_with_several_images_is_an_error_not_a_tar_of_the_output() {
    let scratch = TempDir::new("gaia-archive-several-images");
    let collect = scratch.join("collect");
    fs::create_dir_all(&collect).expect("collect dir");
    fs::write(collect.join("rootfs.erofs"), "erofs").expect("erofs");
    fs::write(collect.join("rootfs.ext4"), "ext4").expect("ext4");
    let archive = collect.join("bundle.bin");
    let image = raw_buildroot_image(&["rootfs.erofs", "rootfs.ext4"]);
    let matched = vec!["rootfs.erofs".to_string(), "rootfs.ext4".to_string()];
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let mut reuse_details = Vec::new();

    let error = archive_buildroot_output(BuildrootArchiveRequest {
        image: &image,
        collect_dir: &collect,
        matched_expected_images: &matched,
        archive_path: &archive,
        reuse_details: &mut reuse_details,
        command: test_command_context(&execution, &policy),
    })
    .expect_err("a non-tar archive over several images must not tar the output");

    assert!(error.message.contains("'bundle.bin'"), "{}", error.message);
    assert!(
        error.message.contains("rootfs.erofs, rootfs.ext4"),
        "{}",
        error.message
    );
    assert!(error.message.contains("To fix"), "{}", error.message);
    assert!(!archive.exists());
}

#[test]
fn a_deferred_raw_archive_leaves_no_placeholder_or_digest_behind() {
    let scratch = TempDir::new("gaia-archive-deferred-placeholder");
    let collect = scratch.join("collect");
    let archive = collect.join("sdcard.img.xz");
    let result = ImageExecutionResult {
        provider_id: "image.buildroot".into(),
        collect_dir: Some(collect.clone()),
        archive_path: Some(archive.clone()),
        emit_report: true,
        reused: false,
        reuse_details: Vec::new(),
        messages: Vec::new(),
        warnings: Vec::new(),
        notes: Vec::new(),
        state_details: Vec::new(),
        disk_images: Vec::new(),
    };

    materialize_image_output(&result).expect("materialize");

    assert!(!archive.exists(), "a placeholder was written");
    let state = fs::read_to_string(collect.join(".gaia-image-state.txt")).expect("state");
    assert!(!state.contains("archive_sha256="), "{state}");
    assert!(!state.contains("archive_bytes="), "{state}");

    // A placeholder an earlier build wrote is removed.
    fs::write(&archive, "provider=image.buildroot\narchive=true\n").expect("old placeholder");
    materialize_image_output(&result).expect("materialize again");
    assert!(!archive.exists(), "the old placeholder stayed");

    // A real archive is left alone and recorded.
    fs::write(&archive, "real compressed bytes").expect("real archive");
    materialize_image_output(&result).expect("materialize with an archive");
    assert!(archive.is_file());
    let state = fs::read_to_string(collect.join(".gaia-image-state.txt")).expect("state");
    assert!(state.contains("archive_sha256="), "{state}");
}

#[test]
fn a_compressed_raw_archive_is_skipped_while_its_image_and_settings_are_unchanged() {
    if Command::new("xz").arg("--version").output().is_err() {
        eprintln!("skipping compressed raw archive reuse test: 'xz' is not installed");
        return;
    }
    let scratch = TempDir::new("gaia-archive-raw-reuse");
    let collect = scratch.join("collect");
    fs::create_dir_all(&collect).expect("collect dir");
    fs::write(collect.join("sdcard.img"), "raw image ".repeat(4096)).expect("raw image");
    let archive = collect.join("sdcard.img.xz");
    let image = raw_buildroot_image(&["sdcard.img"]);
    let matched = vec!["sdcard.img".to_string()];
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let run = |reuse_details: &mut Vec<String>| {
        archive_buildroot_output(BuildrootArchiveRequest {
            image: &image,
            collect_dir: &collect,
            matched_expected_images: &matched,
            archive_path: &archive,
            reuse_details,
            command: test_command_context(&execution, &policy),
        })
        .expect("archive")
    };

    let mut first_reuse = Vec::new();
    let first = run(&mut first_reuse);
    assert!(first_reuse.is_empty(), "first build reused nothing");
    assert!(
        first
            .iter()
            .any(|m| m.starts_with("archived sdcard.img.xz"))
    );
    assert!(archive.is_file());

    let mut second_reuse = Vec::new();
    let second = run(&mut second_reuse);
    assert_eq!(second_reuse, ["image-archive"]);
    assert!(
        second
            .iter()
            .any(|m| m.starts_with("reused compressed image archive")),
        "{second:?}"
    );

    // A changed image is compressed again.
    fs::write(collect.join("sdcard.img"), "raw image v2 ".repeat(4096)).expect("changed image");
    let mut third_reuse = Vec::new();
    run(&mut third_reuse);
    assert!(third_reuse.is_empty(), "a changed image was reused");
}
