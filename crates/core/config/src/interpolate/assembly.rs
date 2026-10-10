use crate::env::ResolvedEnvironment;
use crate::raw::RawBuildConfig;
use crate::raw_assembly::RawImageAssemblyConfig;

use super::assembly_archives;
use super::resolver;

pub(super) fn interpolate_image_assembly(
    mut assembly: RawImageAssemblyConfig,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawImageAssemblyConfig {
    assembly.work_dir = assembly
        .work_dir
        .map(|value| resolver::interpolate_string(value, raw, env));
    assembly.out_dir = assembly
        .out_dir
        .map(|value| resolver::interpolate_string(value, raw, env));
    assembly.trees = assembly
        .trees
        .into_iter()
        .map(|mut tree| {
            tree.id = resolver::interpolate_string(tree.id, raw, env);
            tree.path = resolver::interpolate_string(tree.path, raw, env);
            tree
        })
        .collect();
    assembly.dirs = assembly
        .dirs
        .into_iter()
        .map(|mut dir| {
            dir.tree = resolver::interpolate_string(dir.tree, raw, env);
            dir.path = resolver::interpolate_string(dir.path, raw, env);
            dir.mode = dir
                .mode
                .map(|value| resolver::interpolate_string(value, raw, env));
            dir
        })
        .collect();
    assembly.symlinks = assembly
        .symlinks
        .into_iter()
        .map(|mut symlink| {
            symlink.tree = resolver::interpolate_string(symlink.tree, raw, env);
            symlink.path = resolver::interpolate_string(symlink.path, raw, env);
            symlink.target = resolver::interpolate_string(symlink.target, raw, env);
            symlink
        })
        .collect();
    assembly.files = assembly
        .files
        .into_iter()
        .map(|mut file| {
            file.tree = resolver::interpolate_string(file.tree, raw, env);
            file.src = file
                .src
                .map(|value| resolver::interpolate_string(value, raw, env));
            file.src_glob = file
                .src_glob
                .map(|value| resolver::interpolate_string(value, raw, env));
            file.dest = resolver::interpolate_string(file.dest, raw, env);
            file.mode = file
                .mode
                .map(|value| resolver::interpolate_string(value, raw, env));
            file
        })
        .collect();
    assembly.transforms = assembly
        .transforms
        .into_iter()
        .map(|mut transform| {
            transform.src = transform
                .src
                .map(|value| resolver::interpolate_string(value, raw, env));
            transform.dest = resolver::interpolate_string(transform.dest, raw, env);
            transform
        })
        .collect();
    assembly.filesystems = assembly
        .filesystems
        .into_iter()
        .map(|mut filesystem| {
            filesystem.id = resolver::interpolate_string(filesystem.id, raw, env);
            filesystem.source_tree = resolver::interpolate_string(filesystem.source_tree, raw, env);
            filesystem.output = resolver::interpolate_string(filesystem.output, raw, env);
            filesystem.size = filesystem
                .size
                .map(|value| resolver::interpolate_string(value, raw, env));
            filesystem
        })
        .collect();
    assembly.disks = assembly
        .disks
        .into_iter()
        .map(|mut disk| {
            disk.id = resolver::interpolate_string(disk.id, raw, env);
            disk.output = resolver::interpolate_string(disk.output, raw, env);
            disk.signature = disk
                .signature
                .map(|value| resolver::interpolate_string(value, raw, env));
            disk.signature_text = disk
                .signature_text
                .map(|value| resolver::interpolate_string(value, raw, env));
            disk.partitions = disk
                .partitions
                .into_iter()
                .map(|mut partition| {
                    partition.name = resolver::interpolate_string(partition.name, raw, env);
                    partition.kind = partition
                        .kind
                        .map(|value| resolver::interpolate_string(value, raw, env));
                    partition.type_alias = partition
                        .type_alias
                        .map(|value| resolver::interpolate_string(value, raw, env));
                    partition.image = partition
                        .image
                        .map(|value| resolver::interpolate_string(value, raw, env));
                    partition.size = partition
                        .size
                        .map(|value| resolver::interpolate_string(value, raw, env));
                    partition
                })
                .collect();
            disk
        })
        .collect();
    assembly.archives = assembly
        .archives
        .into_iter()
        .map(|archive| assembly_archives::interpolate_assembly_archive(archive, raw, env))
        .collect();
    assembly.busybox_initramfs = assembly
        .busybox_initramfs
        .into_iter()
        .map(|mut initramfs| {
            initramfs.tree = resolver::interpolate_string(initramfs.tree, raw, env);
            initramfs.busybox = resolver::interpolate_string(initramfs.busybox, raw, env);
            initramfs.sysroot = initramfs
                .sysroot
                .map(|value| resolver::interpolate_string(value, raw, env));
            initramfs.applets = initramfs
                .applets
                .into_iter()
                .map(|value| resolver::interpolate_string(value, raw, env))
                .collect();
            initramfs
        })
        .collect();
    assembly.kernel_modules = assembly
        .kernel_modules
        .into_iter()
        .map(|mut modules| {
            modules.tree = resolver::interpolate_string(modules.tree, raw, env);
            modules.from = resolver::interpolate_string(modules.from, raw, env);
            modules.kernel_version = modules
                .kernel_version
                .map(|value| resolver::interpolate_string(value, raw, env));
            modules.modules = modules
                .modules
                .into_iter()
                .map(|value| resolver::interpolate_string(value, raw, env))
                .collect();
            modules.depmod = modules
                .depmod
                .map(|value| resolver::interpolate_string(value, raw, env));
            modules
        })
        .collect();
    assembly
}
