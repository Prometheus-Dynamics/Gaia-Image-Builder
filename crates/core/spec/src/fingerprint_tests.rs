use super::*;
use crate::{
    AssemblyBinarySpec, AssemblyFileSpec, AssemblyPathTemplate, AssemblyTreeSpec, GitBackendSpec,
    GitSourceSpec, ImageAssemblySpec, SourceId, SourcePinPolicySpec, SourceRefreshPolicySpec,
};

/// The encoding is part of the persisted format: this digest must only
/// change together with a [`FINGERPRINT_SCHEME`] bump.
#[test]
fn the_encoding_is_pinned() {
    let mut fingerprint = Fingerprint::new("test");
    fingerprint
        .str("a")
        .u64(7)
        .bool(true)
        .optional_str(None)
        .field("name", "value")
        .value("list", &["x", "y"]);
    assert_eq!(
        fingerprint.finish().hex(),
        "3d39538f8082173e2a132e100e615fde7fd427085694924be346b28aeb0f330e"
    );
}

#[test]
fn values_are_length_prefixed_and_typed() {
    let digest = |build: fn(&mut Fingerprint)| {
        let mut fingerprint = Fingerprint::new("test");
        build(&mut fingerprint);
        fingerprint.finish()
    };
    assert_ne!(
        digest(|fp| {
            fp.str("ab");
        }),
        digest(|fp| {
            fp.str("a").str("b");
        })
    );
    assert_ne!(
        digest(|fp| {
            fp.optional_str(None);
        }),
        digest(|fp| {
            fp.optional_str(Some(""));
        })
    );
    assert_ne!(
        digest(|fp| {
            fp.str("1");
        }),
        digest(|fp| {
            fp.u64(1);
        })
    );
    assert_ne!(
        Fingerprint::new("a").finish(),
        Fingerprint::new("b").finish()
    );
}

#[test]
fn canonical_json_sorts_keys_at_every_level() {
    let value = serde_json::json!({"b": 1, "a": {"d": [true, null], "c": "x"}});
    assert_eq!(
        canonical_json(&value),
        r#"{"a":{"c":"x","d":[true,null]},"b":1}"#
    );
}

#[test]
fn short_forms_are_prefixes_of_the_digest() {
    let digest = Fingerprint::new("test").finish();
    assert_eq!(digest.short_hex().len(), 16);
    assert!(digest.hex().starts_with(&digest.short_hex()));
    assert_eq!(format!("{:016x}", digest.as_u64()), digest.short_hex());
}

fn git(locked_commit: Option<&str>, git_backend: GitBackendSpec) -> GitSourceSpec {
    GitSourceSpec {
        repo: "https://example.invalid/repo.git".into(),
        branch: Some("main".into()),
        tag: None,
        rev: None,
        subdir: None,
        update: false,
        refresh_policy: SourceRefreshPolicySpec::Auto,
        pin_policy: SourcePinPolicySpec::Floating,
        locked_commit: locked_commit.map(Into::into),
        git_backend,
    }
}

/// Where git runs never changes the checkout, so it is not fingerprinted;
/// an unset lockfile commit is absent rather than `null`.
#[test]
fn git_sources_leave_out_the_backend_and_an_unset_locked_commit() {
    let host = canonical_json(&git(None, GitBackendSpec::Host));
    assert_eq!(host, canonical_json(&git(None, GitBackendSpec::Execution)));
    assert!(!host.contains("locked_commit") && !host.contains("git_backend"));
    assert!(canonical_json(&git(Some("abc"), GitBackendSpec::Host)).contains("locked_commit"));
}

/// Options added after the fields an entry always had are only encoded when
/// set, so entries that do not set them keep their fingerprints when the
/// next one is added.
#[test]
fn optional_assembly_options_are_encoded_only_when_set() {
    let file = |include: Vec<String>, strip: bool, allow_missing_libs: bool| AssemblyFileSpec {
        tree: "rootfs".into(),
        src: None,
        src_glob: Some(AssemblyPathTemplate::new("@assets/*.dtbo")),
        exclude: Vec::new(),
        include,
        strip,
        content: None,
        json: None,
        dest: "overlays/".into(),
        mode: None,
        dir_mode: None,
        optional: false,
        src_dir: None,
        preserve_symlink: false,
        allow_missing_libs,
    };
    let plain = canonical_json(&file(Vec::new(), false, false));
    for name in ["include", "strip", "allow_missing_libs"] {
        assert!(!plain.contains(name), "{plain}");
    }
    let set = canonical_json(&file(vec!["a.dtbo".into()], true, true));
    assert!(
        set.contains(r#""include":["a.dtbo"]"#)
            && set.contains(r#""strip":true"#)
            && set.contains(r#""allow_missing_libs":true"#),
        "{set}"
    );

    let tree = |strip: bool| AssemblyTreeSpec {
        id: "rootfs".into(),
        path: AssemblyPathTemplate::new("$assembly.work/rootfs"),
        strip,
    };
    assert!(!canonical_json(&tree(false)).contains("strip"));
    assert!(canonical_json(&tree(true)).contains("strip"));

    let binary = |strip: bool| AssemblyBinarySpec {
        tree: "rootfs".into(),
        src: AssemblyPathTemplate::new("$provider.images/tool"),
        dest: "usr/bin/tool".into(),
        sysroot: None,
        mode: None,
        strip,
    };
    assert!(!canonical_json(&binary(false)).contains("strip"));
    assert!(canonical_json(&binary(true)).contains("strip"));

    let mut assembly = ImageAssemblySpec::default();
    let plain = canonical_json(&assembly);
    assert!(
        !plain.contains("strip_tool") && !plain.contains("assembly_sources"),
        "{plain}"
    );
    assembly.strip_tool = Some(AssemblyPathTemplate::new("llvm-strip"));
    assembly.assembly_sources = vec![SourceId::new("app")];
    let set = canonical_json(&assembly);
    assert!(
        set.contains(r#""strip_tool":"llvm-strip""#)
            && set.contains(r#""assembly_sources":["app"]"#),
        "{set}"
    );
}

/// The squashfs/EROFS and GPT options are encoded only when set.
#[test]
fn compressed_and_gpt_options_are_encoded_only_when_set() {
    use crate::{
        AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyEbrPlacementSpec,
        AssemblyFilesystemKindSpec, AssemblyFilesystemSpec, AssemblyPartitionTableSpec,
    };
    let partition = AssemblyDiskPartitionSpec {
        name: "root".into(),
        kind: None,
        type_alias: None,
        bootable: false,
        image: None,
        size: Some("1MiB".into()),
        wipe: false,
        materialize: true,
        gpt: Default::default(),
    };
    let mut disk = AssemblyDiskSpec {
        id: "disk".into(),
        output: AssemblyPathTemplate::new("$assembly.out/disk.img"),
        partition_table: AssemblyPartitionTableSpec::Gpt,
        signature: None,
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        truncate: None,
        ebr_placement: AssemblyEbrPlacementSpec::Default,
        gpt: Default::default(),
        partitions: vec![partition],
    };
    let plain = canonical_json(&disk);
    assert!(!plain.contains("gpt"), "{plain}");
    disk.gpt.disk_guid = Some("01234567-89ab-4def-8123-456789abcdef".into());
    disk.partitions[0].gpt.guid = Some("11111111-2222-4333-8444-555555555555".into());
    let set = canonical_json(&disk);
    assert!(
        set.contains("01234567-89ab-4def-8123-456789abcdef")
            && set.contains("11111111-2222-4333-8444-555555555555"),
        "{set}"
    );

    let mut filesystem = AssemblyFilesystemSpec {
        id: "root".into(),
        kind: AssemblyFilesystemKindSpec::Squashfs,
        source_tree: "root".into(),
        output: AssemblyPathTemplate::new("$assembly.work/root.img"),
        size: None,
        deterministic: true,
        compression_level: None,
        publish: false,
        label: None,
        uuid: None,
        journal: None,
        free: None,
        compressed: Default::default(),
        fat_size: None,
        reserved_percent: None,
    };
    assert!(!canonical_json(&filesystem).contains("compressed"));
    assert!(!canonical_json(&filesystem).contains("fat_size"));
    filesystem.compressed.compression = Some("zstd".into());
    assert!(canonical_json(&filesystem).contains(r#""compression":"zstd""#));
    filesystem.fat_size = Some(16);
    assert!(canonical_json(&filesystem).contains(r#""fat_size":16"#));
}
