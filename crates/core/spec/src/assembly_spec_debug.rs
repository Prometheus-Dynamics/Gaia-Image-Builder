//! Debug output of the assembly specs that fingerprints hash. Each prints the
//! text its derived impl did, plus the options that were added later only
//! when they are set, so configs that do not set them keep their
//! fingerprints.

use crate::assembly_spec::*;

// `metadata` prints only when set, so kernel module steps that keep the
// default metadata set keep their fingerprints.
impl std::fmt::Debug for AssemblyKernelModulesSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AssemblyKernelModulesSpec");
        debug
            .field("tree", &self.tree)
            .field("from", &self.from)
            .field("kernel_version", &self.kernel_version)
            .field("modules", &self.modules)
            .field("depmod", &self.depmod);
        if let Some(metadata) = &self.metadata {
            debug.field("metadata", metadata);
        }
        debug.finish()
    }
}

// Operation fingerprints hash the Debug output of the assembly. This prints
// the same text as the derived impl did, plus `assembly_sources` and
// `strip_tool` only when set, so assemblies without them keep their
// fingerprints.
impl std::fmt::Debug for ImageAssemblySpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("ImageAssemblySpec");
        debug
            .field("work_dir", &self.work_dir)
            .field("out_dir", &self.out_dir)
            .field("trees", &self.trees)
            .field("dirs", &self.dirs)
            .field("symlinks", &self.symlinks)
            .field("files", &self.files)
            .field("transforms", &self.transforms)
            .field("filesystems", &self.filesystems)
            .field("disks", &self.disks)
            .field("archives", &self.archives)
            .field("busybox_initramfs", &self.busybox_initramfs)
            .field("binaries", &self.binaries)
            .field("kernel_modules", &self.kernel_modules);
        if let Some(strip_tool) = &self.strip_tool {
            debug.field("strip_tool", strip_tool);
        }
        if !self.assembly_sources.is_empty() {
            debug.field("assembly_sources", &self.assembly_sources);
        }
        debug.finish()
    }
}

// `strip` prints only when set, so trees that do not set it keep their
// fingerprints.
impl std::fmt::Debug for AssemblyTreeSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AssemblyTreeSpec");
        debug.field("id", &self.id).field("path", &self.path);
        if self.strip {
            debug.field("strip", &self.strip);
        }
        debug.finish()
    }
}

// Operation fingerprints hash the Debug output of the assembly. This prints
// the same text as the derived impl did, plus `allow_missing_libs`, `include`
// and `strip` only when set, so entries that do not set them keep their
// fingerprints.
impl std::fmt::Debug for AssemblyFileSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AssemblyFileSpec");
        debug
            .field("tree", &self.tree)
            .field("src", &self.src)
            .field("src_glob", &self.src_glob)
            .field("exclude", &self.exclude)
            .field("content", &self.content)
            .field("json", &self.json)
            .field("dest", &self.dest)
            .field("mode", &self.mode)
            .field("optional", &self.optional)
            .field("preserve_symlink", &self.preserve_symlink);
        if self.allow_missing_libs {
            debug.field("allow_missing_libs", &self.allow_missing_libs);
        }
        if !self.include.is_empty() {
            debug.field("include", &self.include);
        }
        if self.strip {
            debug.field("strip", &self.strip);
        }
        debug.finish()
    }
}

// `strip` prints only when set, so binaries that do not set it keep their
// fingerprints.
impl std::fmt::Debug for AssemblyBinarySpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AssemblyBinarySpec");
        debug
            .field("tree", &self.tree)
            .field("src", &self.src)
            .field("dest", &self.dest)
            .field("sysroot", &self.sysroot)
            .field("mode", &self.mode);
        if self.strip {
            debug.field("strip", &self.strip);
        }
        debug.finish()
    }
}

// The squashfs and erofs options and `fat_size` print only when set, so
// filesystems that do not set them keep their fingerprints.
impl std::fmt::Debug for crate::AssemblyFilesystemSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AssemblyFilesystemSpec");
        debug
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("source_tree", &self.source_tree)
            .field("output", &self.output)
            .field("size", &self.size)
            .field("deterministic", &self.deterministic)
            .field("compression_level", &self.compression_level)
            .field("publish", &self.publish)
            .field("label", &self.label)
            .field("uuid", &self.uuid)
            .field("journal", &self.journal)
            .field("free", &self.free);
        if self.compressed != crate::AssemblyCompressedFsSpec::default() {
            debug.field("compressed", &self.compressed);
        }
        if let Some(fat_size) = &self.fat_size {
            debug.field("fat_size", fat_size);
        }
        debug.finish()
    }
}

// The GPT disk GUID prints only when set, so disks that do not set it keep
// their fingerprints.
impl std::fmt::Debug for AssemblyDiskSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AssemblyDiskSpec");
        debug
            .field("id", &self.id)
            .field("output", &self.output)
            .field("partition_table", &self.partition_table)
            .field("signature", &self.signature)
            .field("signature_text", &self.signature_text)
            .field("first_lba", &self.first_lba)
            .field("alignment_lba", &self.alignment_lba)
            .field("truncate", &self.truncate)
            .field("ebr_placement", &self.ebr_placement)
            .field("partitions", &self.partitions);
        if self.gpt != crate::AssemblyGptDiskSpec::default() {
            debug.field("gpt", &self.gpt);
        }
        debug.finish()
    }
}

// The GPT partition GUID prints only when set, so partitions that do not set
// it keep their fingerprints.
impl std::fmt::Debug for AssemblyDiskPartitionSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AssemblyDiskPartitionSpec");
        debug
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("type_alias", &self.type_alias)
            .field("bootable", &self.bootable)
            .field("image", &self.image)
            .field("size", &self.size)
            .field("wipe", &self.wipe)
            .field("materialize", &self.materialize);
        if self.gpt != crate::AssemblyGptPartitionSpec::default() {
            debug.field("gpt", &self.gpt);
        }
        debug.finish()
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;
    use crate::SourceId;

    /// Fingerprints hash this text: sources appear only when named.
    #[test]
    fn assembly_sources_print_only_when_set() {
        let mut spec = ImageAssemblySpec::default();
        let plain = format!("{spec:?}");
        assert!(!plain.contains("assembly_sources"), "{plain}");
        assert!(plain.starts_with("ImageAssemblySpec { work_dir: None, out_dir: None, trees: []"));
        spec.assembly_sources = vec![SourceId::new("app")];
        let named = format!("{spec:?}");
        assert!(
            named.ends_with("assembly_sources: [SourceId(\"app\")] }"),
            "{named}"
        );
    }

    /// The squashfs/erofs and GPT options print only when set.
    #[test]
    fn compressed_and_gpt_options_print_only_when_set() {
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
        assert_eq!(
            format!("{partition:?}"),
            "AssemblyDiskPartitionSpec { name: \"root\", kind: None, type_alias: None, bootable: false, image: None, size: Some(\"1MiB\"), wipe: false, materialize: true }"
        );
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
        assert!(!format!("{disk:?}").contains("gpt:"));
        disk.gpt.disk_guid = Some("01234567-89ab-4def-8123-456789abcdef".into());
        disk.partitions[0].gpt.guid = Some("11111111-2222-4333-8444-555555555555".into());
        let set = format!("{disk:?}");
        assert!(
            set.contains("disk_guid") && set.contains("guid: Some"),
            "{set}"
        );
        let mut filesystem = crate::AssemblyFilesystemSpec {
            id: "root".into(),
            kind: crate::AssemblyFilesystemKindSpec::Squashfs,
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
        };
        assert!(format!("{filesystem:?}").ends_with("journal: None, free: None }"));
        filesystem.compressed.compression = Some("zstd".into());
        assert!(format!("{filesystem:?}").contains("compression: Some(\"zstd\")"));
    }

    /// The options added with `include`, `strip` and `strip_tool` print only
    /// when set, so the configs that do not set them keep their fingerprints.
    #[test]
    fn strip_and_include_options_print_only_when_set() {
        let file = |include: Vec<String>, strip: bool| AssemblyFileSpec {
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
            optional: false,
            preserve_symlink: false,
            allow_missing_libs: false,
        };
        let plain = format!("{:?}", file(Vec::new(), false));
        assert!(
            !plain.contains("include") && !plain.contains("strip"),
            "{plain}"
        );
        let set = format!("{:?}", file(vec!["a.dtbo".into()], true));
        assert!(
            set.contains("include: [\"a.dtbo\"]") && set.contains("strip: true"),
            "{set}"
        );

        let tree = |strip: bool| AssemblyTreeSpec {
            id: "rootfs".into(),
            path: AssemblyPathTemplate::new("$assembly.work/rootfs"),
            strip,
        };
        assert!(!format!("{:?}", tree(false)).contains("strip"));
        assert!(format!("{:?}", tree(true)).ends_with("strip: true }"));

        let binary = |strip: bool| AssemblyBinarySpec {
            tree: "rootfs".into(),
            src: AssemblyPathTemplate::new("$provider.images/tool"),
            dest: "usr/bin/tool".into(),
            sysroot: None,
            mode: None,
            strip,
        };
        assert!(!format!("{:?}", binary(false)).contains("strip"));
        assert!(format!("{:?}", binary(true)).ends_with("strip: true }"));

        let assembly = ImageAssemblySpec {
            strip_tool: Some(AssemblyPathTemplate::new("llvm-strip")),
            ..ImageAssemblySpec::default()
        };
        assert!(
            format!("{assembly:?}").ends_with("strip_tool: AssemblyPathTemplate(\"llvm-strip\") }")
        );
    }

    /// `fat_size` prints only when set; unset, the text is the derived one.
    #[test]
    fn fat_size_prints_only_when_set() {
        let filesystem = |fat_size: Option<u32>| crate::AssemblyFilesystemSpec {
            id: "boot".into(),
            kind: crate::AssemblyFilesystemKindSpec::Vfat,
            source_tree: "boot".into(),
            output: AssemblyPathTemplate::new("$assembly.work/boot.vfat"),
            size: Some("auto".into()),
            deterministic: false,
            compression_level: None,
            publish: false,
            label: None,
            uuid: None,
            journal: None,
            free: None,
            compressed: Default::default(),
            fat_size,
        };
        assert_eq!(
            format!("{:?}", filesystem(None)),
            "AssemblyFilesystemSpec { id: AssemblyFilesystemId(\"boot\"), kind: Vfat, \
             source_tree: AssemblyTreeId(\"boot\"), \
             output: AssemblyPathTemplate(\"$assembly.work/boot.vfat\"), size: Some(\"auto\"), \
             deterministic: false, compression_level: None, publish: false, label: None, \
             uuid: None, journal: None, free: None }"
        );
        assert!(format!("{:?}", filesystem(Some(12))).ends_with("free: None, fat_size: 12 }"));
    }
}
