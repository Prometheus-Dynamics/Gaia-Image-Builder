//! Every string of `[image.assembly]`, visited for `${source.<id>...}`
//! tokens: paths, tree ids, file and archive entries, JSON string values,
//! archive outputs and the rest. Keys of JSON objects and entries are not
//! visited, since tokens are only rendered in values.

use gaia_spec::{
    AssemblyFilesystemId, AssemblyJsonValue, AssemblyPathTemplate, AssemblyTreeId,
    ImageAssemblySpec,
};

/// Receives each visited string with its location in the config
/// (`image.assembly.files.dest`, ...) and may rewrite it in place.
pub(crate) type Visit<'a> = dyn FnMut(&'static str, &mut String) + 'a;

/// Visits every string value of the assembly.
pub(crate) fn visit_assembly_strings(assembly: &mut ImageAssemblySpec, visit: &mut Visit<'_>) {
    path(&mut assembly.work_dir, "image.assembly.work_dir", visit);
    path(&mut assembly.out_dir, "image.assembly.out_dir", visit);
    for tree in &mut assembly.trees {
        tree_id(&mut tree.id, "image.assembly.trees.id", visit);
        path_value(&mut tree.path, "image.assembly.trees.path", visit);
    }
    for dir in &mut assembly.dirs {
        tree_id(&mut dir.tree, "image.assembly.dirs.tree", visit);
        text(&mut dir.path, "image.assembly.dirs.path", visit);
        opt_text(&mut dir.mode, "image.assembly.dirs.mode", visit);
    }
    for symlink in &mut assembly.symlinks {
        tree_id(&mut symlink.tree, "image.assembly.symlinks.tree", visit);
        text(&mut symlink.path, "image.assembly.symlinks.path", visit);
        text(&mut symlink.target, "image.assembly.symlinks.target", visit);
    }
    for file in &mut assembly.files {
        tree_id(&mut file.tree, "image.assembly.files.tree", visit);
        path(&mut file.src, "image.assembly.files.src", visit);
        path(&mut file.src_glob, "image.assembly.files.src_glob", visit);
        path(&mut file.src_dir, "image.assembly.files.src_dir", visit);
        texts(&mut file.exclude, "image.assembly.files.exclude", visit);
        opt_text(&mut file.content, "image.assembly.files.content", visit);
        if let Some(json) = &mut file.json {
            json_strings(json, "image.assembly.files.json", visit);
        }
        text(&mut file.dest, "image.assembly.files.dest", visit);
        opt_text(&mut file.mode, "image.assembly.files.mode", visit);
    }
    for transform in &mut assembly.transforms {
        path(&mut transform.src, "image.assembly.transforms.src", visit);
        path_value(&mut transform.dest, "image.assembly.transforms.dest", visit);
        path(
            &mut transform.hash_dest,
            "image.assembly.transforms.hash_dest",
            visit,
        );
        opt_text(&mut transform.salt, "image.assembly.transforms.salt", visit);
        opt_text(&mut transform.uuid, "image.assembly.transforms.uuid", visit);
    }
    for filesystem in &mut assembly.filesystems {
        let mut id = filesystem.id.as_str().to_string();
        visit("image.assembly.filesystems.id", &mut id);
        filesystem.id = AssemblyFilesystemId::new(id);
        tree_id(
            &mut filesystem.source_tree,
            "image.assembly.filesystems.source_tree",
            visit,
        );
        path_value(
            &mut filesystem.output,
            "image.assembly.filesystems.output",
            visit,
        );
        opt_text(
            &mut filesystem.size,
            "image.assembly.filesystems.size",
            visit,
        );
        opt_text(
            &mut filesystem.label,
            "image.assembly.filesystems.label",
            visit,
        );
        opt_text(
            &mut filesystem.uuid,
            "image.assembly.filesystems.uuid",
            visit,
        );
        opt_text(
            &mut filesystem.free,
            "image.assembly.filesystems.free",
            visit,
        );
    }
    for ubi in &mut assembly.ubis {
        text(&mut ubi.id, "image.assembly.ubis.id", visit);
        path_value(&mut ubi.output, "image.assembly.ubis.output", visit);
        for volume in &mut ubi.volumes {
            text(&mut volume.name, "image.assembly.ubis.volumes.name", visit);
            path(
                &mut volume.image,
                "image.assembly.ubis.volumes.image",
                visit,
            );
            opt_text(&mut volume.size, "image.assembly.ubis.volumes.size", visit);
        }
    }
    for disk in &mut assembly.disks {
        text(&mut disk.id, "image.assembly.disks.id", visit);
        path_value(&mut disk.output, "image.assembly.disks.output", visit);
        opt_text(&mut disk.signature, "image.assembly.disks.signature", visit);
        opt_text(
            &mut disk.signature_text,
            "image.assembly.disks.signature_text",
            visit,
        );
        for partition in &mut disk.partitions {
            text(
                &mut partition.name,
                "image.assembly.disks.partitions.name",
                visit,
            );
            opt_text(
                &mut partition.kind,
                "image.assembly.disks.partitions.kind",
                visit,
            );
            opt_text(
                &mut partition.type_alias,
                "image.assembly.disks.partitions.type",
                visit,
            );
            path(
                &mut partition.image,
                "image.assembly.disks.partitions.image",
                visit,
            );
            opt_text(
                &mut partition.size,
                "image.assembly.disks.partitions.size",
                visit,
            );
        }
    }
    for archive in &mut assembly.archives {
        text(&mut archive.id, "image.assembly.archives.id", visit);
        path_value(&mut archive.output, "image.assembly.archives.output", visit);
        for member in &mut archive.members {
            text(
                &mut member.name,
                "image.assembly.archives.members.name",
                visit,
            );
            path(
                &mut member.src,
                "image.assembly.archives.members.src",
                visit,
            );
            if let Some(tree) = &mut member.tree {
                tree_id(tree, "image.assembly.archives.members.tree", visit);
            }
            path(
                &mut member.src_dir,
                "image.assembly.archives.members.src_dir",
                visit,
            );
            path(
                &mut member.src_glob,
                "image.assembly.archives.members.src_glob",
                visit,
            );
            text(
                &mut member.prefix,
                "image.assembly.archives.members.prefix",
                visit,
            );
            texts(
                &mut member.exclude,
                "image.assembly.archives.members.exclude",
                visit,
            );
            if let Some(entries) = &mut member.entries {
                for (_, value) in entries.iter_mut() {
                    text(value, "image.assembly.archives.members.entries", visit);
                }
            }
            if let Some(json) = &mut member.json {
                json_strings(json, "image.assembly.archives.members.json", visit);
            }
        }
    }
    for package in &mut assembly.packages {
        text(&mut package.id, "image.assembly.packages.id", visit);
        tree_id(&mut package.tree, "image.assembly.packages.tree", visit);
        path_value(&mut package.dest, "image.assembly.packages.dest", visit);
        path_value(&mut package.output, "image.assembly.packages.dest", visit);
        texts(
            &mut package.allowed_prefixes,
            "image.assembly.packages.allowed_prefixes",
            visit,
        );
        let control = &mut package.control;
        text(
            &mut control.package,
            "image.assembly.packages.control",
            visit,
        );
        text(
            &mut control.version,
            "image.assembly.packages.control",
            visit,
        );
        text(
            &mut control.architecture,
            "image.assembly.packages.control",
            visit,
        );
        text(
            &mut control.maintainer,
            "image.assembly.packages.control",
            visit,
        );
        text(
            &mut control.description,
            "image.assembly.packages.control",
            visit,
        );
        texts(
            &mut control.depends,
            "image.assembly.packages.control",
            visit,
        );
        opt_text(
            &mut control.section,
            "image.assembly.packages.control",
            visit,
        );
        opt_text(
            &mut control.priority,
            "image.assembly.packages.control",
            visit,
        );
        opt_text(
            &mut control.source,
            "image.assembly.packages.control",
            visit,
        );
        opt_text(
            &mut control.homepage,
            "image.assembly.packages.control",
            visit,
        );
        for (_, value) in &mut control.extra {
            text(value, "image.assembly.packages.control.extra", visit);
        }
        let scripts = &mut package.scripts;
        texts(
            &mut scripts.services,
            "image.assembly.packages.scripts",
            visit,
        );
        path(
            &mut scripts.postinst,
            "image.assembly.packages.scripts",
            visit,
        );
        path(
            &mut scripts.preinst,
            "image.assembly.packages.scripts",
            visit,
        );
        path(&mut scripts.prerm, "image.assembly.packages.scripts", visit);
        path(
            &mut scripts.postrm,
            "image.assembly.packages.scripts",
            visit,
        );
        texts(
            &mut scripts.conffiles,
            "image.assembly.packages.scripts",
            visit,
        );
    }
    for oci in &mut assembly.ocis {
        text(&mut oci.id, "image.assembly.oci.id", visit);
        path_value(&mut oci.dest, "image.assembly.oci.dest", visit);
        text(&mut oci.format, "image.assembly.oci.format", visit);
        text(
            &mut oci.compression,
            "image.assembly.oci.compression",
            visit,
        );
        opt_text(&mut oci.reference, "image.assembly.oci.ref", visit);
        for layer in &mut oci.layers {
            path(&mut layer.tar, "image.assembly.oci.layers.tar", visit);
            if let Some(tree) = &mut layer.tree {
                tree_id(tree, "image.assembly.oci.layers.tree", visit);
            }
        }
        let config = &mut oci.config;
        text(
            &mut config.architecture,
            "image.assembly.oci.config.architecture",
            visit,
        );
        text(&mut config.os, "image.assembly.oci.config.os", visit);
        texts(
            &mut config.entrypoint,
            "image.assembly.oci.config.entrypoint",
            visit,
        );
        texts(&mut config.cmd, "image.assembly.oci.config.cmd", visit);
        texts(&mut config.env, "image.assembly.oci.config.env", visit);
        opt_text(
            &mut config.working_dir,
            "image.assembly.oci.config.working_dir",
            visit,
        );
        opt_text(
            &mut config.stop_signal,
            "image.assembly.oci.config.stop_signal",
            visit,
        );
        // Label keys are names, not values: only the values take tokens.
        for (_, value) in &mut config.labels {
            text(value, "image.assembly.oci.config.labels", visit);
        }
    }
    for initramfs in &mut assembly.busybox_initramfs {
        tree_id(
            &mut initramfs.tree,
            "image.assembly.busybox_initramfs.tree",
            visit,
        );
        path_value(
            &mut initramfs.busybox,
            "image.assembly.busybox_initramfs.busybox",
            visit,
        );
        path(
            &mut initramfs.sysroot,
            "image.assembly.busybox_initramfs.sysroot",
            visit,
        );
        texts(
            &mut initramfs.applets,
            "image.assembly.busybox_initramfs.applets",
            visit,
        );
    }
    for binary in &mut assembly.binaries {
        tree_id(&mut binary.tree, "image.assembly.binaries.tree", visit);
        path_value(&mut binary.src, "image.assembly.binaries.src", visit);
        text(&mut binary.dest, "image.assembly.binaries.dest", visit);
        path(
            &mut binary.sysroot,
            "image.assembly.binaries.sysroot",
            visit,
        );
        opt_text(&mut binary.mode, "image.assembly.binaries.mode", visit);
    }
    for modules in &mut assembly.kernel_modules {
        tree_id(
            &mut modules.tree,
            "image.assembly.kernel_modules.tree",
            visit,
        );
        path_value(
            &mut modules.from,
            "image.assembly.kernel_modules.from",
            visit,
        );
        opt_text(
            &mut modules.kernel_version,
            "image.assembly.kernel_modules.kernel_version",
            visit,
        );
        texts(
            &mut modules.modules,
            "image.assembly.kernel_modules.modules",
            visit,
        );
        path(
            &mut modules.depmod,
            "image.assembly.kernel_modules.depmod",
            visit,
        );
    }
}

fn text(value: &mut String, location: &'static str, visit: &mut Visit<'_>) {
    visit(location, value);
}

fn opt_text(value: &mut Option<String>, location: &'static str, visit: &mut Visit<'_>) {
    if let Some(value) = value {
        visit(location, value);
    }
}

fn texts(values: &mut [String], location: &'static str, visit: &mut Visit<'_>) {
    for value in values {
        visit(location, value);
    }
}

fn path_value(value: &mut AssemblyPathTemplate, location: &'static str, visit: &mut Visit<'_>) {
    let mut text = value.as_str().to_string();
    visit(location, &mut text);
    *value = AssemblyPathTemplate::new(text);
}

fn path(value: &mut Option<AssemblyPathTemplate>, location: &'static str, visit: &mut Visit<'_>) {
    if let Some(value) = value {
        path_value(value, location, visit);
    }
}

fn tree_id(value: &mut AssemblyTreeId, location: &'static str, visit: &mut Visit<'_>) {
    let mut text = value.as_str().to_string();
    visit(location, &mut text);
    *value = AssemblyTreeId::new(text);
}

/// Every string value of a JSON document, at any depth; keys are skipped.
fn json_strings(value: &mut AssemblyJsonValue, location: &'static str, visit: &mut Visit<'_>) {
    match value {
        AssemblyJsonValue::String(text_value) => visit(location, text_value),
        AssemblyJsonValue::Integer(_) | AssemblyJsonValue::Bool(_) => {}
        AssemblyJsonValue::Array(items) => {
            for item in items {
                json_strings(item, location, visit);
            }
        }
        AssemblyJsonValue::Object(members) => {
            for (_, item) in members {
                json_strings(item, location, visit);
            }
        }
    }
}
