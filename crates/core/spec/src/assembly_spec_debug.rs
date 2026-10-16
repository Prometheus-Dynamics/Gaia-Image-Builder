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
}
