use super::*;
use crate::ImageProviderKind;

#[test]
fn mbr_partition_type_parses_aliases_and_raw_hex() {
    assert_eq!(
        MbrPartitionType::from_alias("fat32-lba")
            .expect("fat32-lba")
            .byte(),
        0x0c
    );
    assert_eq!(
        MbrPartitionType::from_alias("linux").expect("linux").byte(),
        0x83
    );
    assert_eq!(
        MbrPartitionType::from_raw_hex("0x0C").expect("raw").byte(),
        0x0c
    );
    assert!(MbrPartitionType::from_alias("unknown").is_err());
    assert!(MbrPartitionType::from_raw_hex("0x100").is_err());
}

#[test]
fn assembly_path_template_validates_known_tokens() {
    let template = AssemblyPathTemplate::new(
        "$provider.images/$provider.buildroot_output/$assembly.work/$assembly.out/$assembly.tree.boot/Image",
    );

    assert_eq!(
        template.validate_tokens(ImageProviderKind::Buildroot, |id| id == "boot"),
        Ok(())
    );
    let template = AssemblyPathTemplate::new(
        "$provider.images/$assembly.work/$assembly.out/$assembly.tree.boot/Image",
    );
    assert_eq!(
        template.validate_tokens(ImageProviderKind::StartingPoint, |id| id == "boot"),
        Ok(())
    );
}

#[test]
fn assembly_path_template_rejects_unknown_and_unavailable_tokens() {
    assert_eq!(
        AssemblyPathTemplate::new("$assembly.tree.")
            .validate_tokens(ImageProviderKind::Buildroot, |_| true),
        Err(AssemblyPathTemplateError::EmptyTreeReference)
    );
    assert_eq!(
        AssemblyPathTemplate::new("$assembly.tree.root")
            .validate_tokens(ImageProviderKind::Buildroot, |_| false),
        Err(AssemblyPathTemplateError::UnknownTree("root".into()))
    );
    assert_eq!(
        AssemblyPathTemplate::new("$assembly.unknown")
            .validate_tokens(ImageProviderKind::Buildroot, |_| true),
        Err(AssemblyPathTemplateError::UnknownAssemblyVariable(
            "$assembly.unknown".into()
        ))
    );
    assert_eq!(
        AssemblyPathTemplate::new("$provider.host")
            .validate_tokens(ImageProviderKind::StartingPoint, |_| true),
        Err(AssemblyPathTemplateError::UnavailableProviderVariable(
            "$provider.host".into()
        ))
    );
    assert_eq!(
        AssemblyPathTemplate::new("$provider.buildroot_output")
            .validate_tokens(ImageProviderKind::StartingPoint, |_| true),
        Err(AssemblyPathTemplateError::UnavailableProviderVariable(
            "$provider.buildroot_output".into()
        ))
    );
    assert_eq!(
        AssemblyPathTemplate::new("$provider.sdk")
            .validate_tokens(ImageProviderKind::Buildroot, |_| true),
        Err(AssemblyPathTemplateError::UnknownProviderVariable(
            "$provider.sdk".into()
        ))
    );
}

#[test]
fn assembly_path_template_resolves_known_tokens() {
    let mut trees = BTreeMap::new();
    trees.insert(
        AssemblyTreeId::from("boot"),
        PathBuf::from("/tmp/work/boot"),
    );
    let context = AssemblyPathTemplateContext {
        provider_kind: ImageProviderKind::Buildroot,
        provider_images: Path::new("/tmp/images"),
        provider_buildroot_output: Some(Path::new("/tmp/build/image/buildroot-output")),
        provider_target: Path::new("/tmp/images/buildroot-output/target"),
        provider_host: Some(Path::new("/tmp/images/buildroot-output/host")),
        provider_staging: Some(Path::new("/tmp/images/buildroot-output/staging")),
        assembly_work: Path::new("/tmp/work"),
        assembly_out: Path::new("/tmp/images"),
        trees: &trees,
    };

    assert_eq!(
        AssemblyPathTemplate::new(
            "$provider.images:$provider.buildroot_output:$provider.target:$provider.host:$provider.staging:$assembly.work:$assembly.out:$assembly.tree.boot/Image"
        )
        .resolve(&context)
        .expect("resolved"),
        "/tmp/images:/tmp/build/image/buildroot-output:/tmp/images/buildroot-output/target:/tmp/images/buildroot-output/host:/tmp/images/buildroot-output/staging:/tmp/work:/tmp/images:/tmp/work/boot/Image"
    );
}

#[test]
fn assembly_path_template_resolution_rejects_unknown_and_unavailable_tokens() {
    let trees = BTreeMap::new();
    let context = AssemblyPathTemplateContext {
        provider_kind: ImageProviderKind::StartingPoint,
        provider_images: Path::new("/tmp/images"),
        provider_buildroot_output: None,
        provider_target: Path::new("/tmp/images/rootfs"),
        provider_host: None,
        provider_staging: None,
        assembly_work: Path::new("/tmp/work"),
        assembly_out: Path::new("/tmp/images"),
        trees: &trees,
    };

    assert_eq!(
        AssemblyPathTemplate::new("$provider.host").resolve(&context),
        Err(AssemblyPathTemplateError::UnavailableProviderVariable(
            "$provider.host".into()
        ))
    );
    assert_eq!(
        AssemblyPathTemplate::new("$assembly.tree.boot").resolve(&context),
        Err(AssemblyPathTemplateError::UnknownTree("boot".into()))
    );
}

#[test]
fn assembly_path_template_exposes_referenced_tree_ids() {
    assert_eq!(
        AssemblyPathTemplate::new(
            "$assembly.tree.boot/Image:$assembly.tree.rootfs:$assembly.tree."
        )
        .referenced_tree_ids(),
        vec!["boot", "rootfs"]
    );
}

#[test]
fn assembly_path_template_rejects_provider_tokens_without_an_image_provider() {
    let template = AssemblyPathTemplate::new("$provider.images/pkg.ipk");
    let error = template
        .validate_tokens(ImageProviderKind::None, |_| true)
        .expect_err("no provider tokens");
    assert_eq!(
        error,
        AssemblyPathTemplateError::NoImageProvider("$provider.images".into())
    );
    assert!(error.to_string().contains("kind = \"none\""), "{error}");
    assert!(
        AssemblyPathTemplate::new("$assembly.work/pkg")
            .validate_tokens(ImageProviderKind::None, |_| true)
            .is_ok()
    );
}
