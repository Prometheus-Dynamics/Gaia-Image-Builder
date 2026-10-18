//! The options of `squashfs` and `erofs` filesystems: the compressor, its
//! level, the block size and the erofs layout options. Other kinds accept
//! none of them.

use std::ops::RangeInclusive;

use gaia_spec::{
    AssemblyFilesystemKindSpec, AssemblyFilesystemSpec, EROFS_BLOCK_SIZES, ErofsCompression,
    ImageAssemblySpec, SQUASHFS_BLOCK_SIZES, SquashfsCompression,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

/// The erofs block size when the spec sets none.
const EROFS_DEFAULT_BLOCK_SIZE: u32 = 4096;

pub(crate) fn validate_compressed_filesystems(
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for filesystem in &assembly.filesystems {
        let erofs_only = erofs_only_options(filesystem);
        match filesystem.kind {
            AssemblyFilesystemKindSpec::Squashfs => {
                for option in erofs_only {
                    diagnostics.push(unsupported(filesystem, option, "'erofs'"));
                }
                validate_common(filesystem, &SQUASHFS_BLOCK_SIZES, diagnostics);
                match SquashfsCompression::parse(filesystem.compressed.compression.as_deref()) {
                    Some(compression) => validate_level(
                        filesystem,
                        compression.as_str(),
                        compression.levels(),
                        diagnostics,
                    ),
                    None => diagnostics
                        .push(invalid_compression(filesystem, SquashfsCompression::NAMES)),
                }
            }
            AssemblyFilesystemKindSpec::Erofs => {
                validate_common(filesystem, &EROFS_BLOCK_SIZES, diagnostics);
                let Some(compression) =
                    ErofsCompression::parse(filesystem.compressed.compression.as_deref())
                else {
                    diagnostics.push(invalid_compression(filesystem, ErofsCompression::NAMES));
                    continue;
                };
                validate_level(
                    filesystem,
                    compression.as_str(),
                    compression.levels(),
                    diagnostics,
                );
                if compression == ErofsCompression::None {
                    for option in erofs_only {
                        diagnostics.push(error(
                            "assembly_filesystem_option_unsupported",
                            format!(
                                "assembly filesystem '{}' sets {option}, which needs compression; compression is 'none'",
                                filesystem.id
                            ),
                            location(),
                        ));
                    }
                }
                let block_size = filesystem
                    .compressed
                    .block_size
                    .unwrap_or(EROFS_DEFAULT_BLOCK_SIZE);
                if let Some(pcluster_size) = filesystem.compressed.pcluster_size
                    && (pcluster_size == 0 || !pcluster_size.is_multiple_of(block_size))
                {
                    diagnostics.push(error(
                        "assembly_filesystem_pcluster_size_invalid",
                        format!(
                            "assembly filesystem '{}' pcluster_size {pcluster_size} must be a multiple of the block size {block_size}",
                            filesystem.id
                        ),
                        location(),
                    ));
                }
            }
            _ => {
                let compressed = &filesystem.compressed;
                let common = [
                    (compressed.compression.is_some(), "compression"),
                    (compressed.block_size.is_some(), "block_size"),
                ];
                for (set, option) in common {
                    if set {
                        diagnostics.push(unsupported(filesystem, option, "'squashfs' and 'erofs'"));
                    }
                }
                for option in erofs_only {
                    diagnostics.push(unsupported(filesystem, option, "'erofs'"));
                }
            }
        }
    }
}

fn location() -> Option<String> {
    Some("image.assembly.filesystems".into())
}

/// The erofs-only options the filesystem sets.
fn erofs_only_options(filesystem: &AssemblyFilesystemSpec) -> Vec<&'static str> {
    let compressed = &filesystem.compressed;
    [
        (compressed.pcluster_size.is_some(), "pcluster_size"),
        (compressed.dedupe, "dedupe"),
        (compressed.fragments, "fragments"),
        (compressed.ztailpacking, "ztailpacking"),
    ]
    .into_iter()
    .filter_map(|(set, option)| set.then_some(option))
    .collect()
}

/// What both kinds check: no `size` (the image is as large as its content)
/// and a block size the tool accepts.
fn validate_common(
    filesystem: &AssemblyFilesystemSpec,
    block_sizes: &RangeInclusive<u32>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if filesystem.size.is_some() && filesystem.size.as_deref() != Some("auto") {
        diagnostics.push(error(
            "assembly_filesystem_option_unsupported",
            format!(
                "assembly filesystem '{}' kind '{}' does not accept size; the image is as large as its content",
                filesystem.id,
                filesystem.kind.as_str()
            ),
            location(),
        ));
    }
    if let Some(block_size) = filesystem.compressed.block_size
        && !(block_size.is_power_of_two() && block_sizes.contains(&block_size))
    {
        diagnostics.push(error(
            "assembly_filesystem_block_size_invalid",
            format!(
                "assembly filesystem '{}' block_size {block_size} must be a power of two from {} to {}",
                filesystem.id,
                block_sizes.start(),
                block_sizes.end()
            ),
            location(),
        ));
    }
}

fn validate_level(
    filesystem: &AssemblyFilesystemSpec,
    compression: &str,
    levels: Option<RangeInclusive<u32>>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let Some(level) = filesystem.compression_level else {
        return;
    };
    match levels {
        None => diagnostics.push(error(
            "assembly_filesystem_compression_level_unsupported",
            format!(
                "assembly filesystem '{}' compression '{compression}' does not accept compression_level",
                filesystem.id
            ),
            location(),
        )),
        Some(levels) if !levels.contains(&level) => diagnostics.push(error(
            "assembly_filesystem_compression_level_invalid",
            format!(
                "assembly filesystem '{}' compression_level {level} must be between {} and {} for '{compression}'",
                filesystem.id,
                levels.start(),
                levels.end()
            ),
            location(),
        )),
        Some(_) => {}
    }
}

fn invalid_compression(filesystem: &AssemblyFilesystemSpec, names: &str) -> ValidationDiagnostic {
    error(
        "assembly_filesystem_compression_invalid",
        format!(
            "assembly filesystem '{}' compression '{}' is not one of {names}",
            filesystem.id,
            filesystem
                .compressed
                .compression
                .as_deref()
                .unwrap_or_default()
        ),
        location(),
    )
}

fn unsupported(
    filesystem: &AssemblyFilesystemSpec,
    option: &str,
    kinds: &str,
) -> ValidationDiagnostic {
    error(
        "assembly_filesystem_option_unsupported",
        format!(
            "assembly filesystem '{}' kind '{}' does not accept {option}; only {kinds} do",
            filesystem.id,
            filesystem.kind.as_str()
        ),
        location(),
    )
}
