//! Image assembly fingerprints track the inputs of transforms: the source
//! file a transform reads, for the gzip and verity kinds. Split from
//! `plan_graph.rs` for its length.

pub mod support;

use gaia_config::resolve_config;
use gaia_plan::plan_build;
use gaia_spec::{AssemblyTransformKindSpec, AssemblyTransformSpec, ImageAssemblySpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::PathBuf;
use support::{default_config_path, provider_catalogs, rebase_workspace_root, scratch_child};

#[test]
fn image_assembly_fingerprint_tracks_transform_inputs() {
    let scratch = TempDir::new("gaia-plan-test");
    let root_dir = scratch_child(
        &scratch,
        "gaia-plan-image-assembly-transform-fingerprint-root",
    );
    fs::create_dir_all(&root_dir).expect("root dir");
    let source_path = PathBuf::from(&root_dir).join("kernel");
    fs::write(&source_path, "one").expect("source");
    let mut spec = resolve_config(&default_config_path());
    rebase_workspace_root(&mut spec, &root_dir);
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Gzip,
            src: Some(source_path.display().to_string().into()),
            dest: "$assembly.work/kernel.img".into(),
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
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let first = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("first assembly operation")
        .fingerprint;

    fs::write(&source_path, "two").expect("updated source");
    let second = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog)
        .operations
        .into_iter()
        .find(|operation| operation.id.as_str() == "image:assembly")
        .expect("second assembly operation")
        .fingerprint;

    assert_ne!(first, second);
}

#[test]
fn image_assembly_fingerprint_tracks_verity_data_inputs() {
    let scratch = TempDir::new("gaia-plan-test");
    let root_dir = scratch_child(&scratch, "gaia-plan-image-assembly-verity-fingerprint-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let source_path = PathBuf::from(&root_dir).join("rootfs.erofs");
    fs::write(&source_path, "one").expect("source");
    let mut spec = resolve_config(&default_config_path());
    rebase_workspace_root(&mut spec, &root_dir);
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Verity,
            src: Some(source_path.display().to_string().into()),
            dest: "$assembly.work/rootfs.verity.erofs".into(),
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
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let assembly_fingerprint = |spec: &gaia_spec::ResolvedBuildSpec| {
        plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog)
            .operations
            .into_iter()
            .find(|operation| operation.id.as_str() == "image:assembly")
            .expect("assembly operation")
            .fingerprint
    };
    let first = assembly_fingerprint(&spec);
    fs::write(&source_path, "two").expect("updated source");
    assert_ne!(first, assembly_fingerprint(&spec));
}
