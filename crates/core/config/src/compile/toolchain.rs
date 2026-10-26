use crate::raw::{RawToolchainConfig, RawToolchainKind};
use gaia_spec::{ArtifactSpec, SourceId, ToolchainKindSpec, ToolchainSpec};

/// A `[[toolchains]]` entry as compiled. Missing or conflicting fields are
/// left for the validator to report.
pub(crate) fn compile_toolchain(raw: RawToolchainConfig) -> ToolchainSpec {
    ToolchainSpec {
        id: raw.id,
        kind: match raw.kind {
            RawToolchainKind::BuildrootSdk => ToolchainKindSpec::BuildrootSdk,
        },
        root_source: raw.source.map(SourceId::new),
        root_path: raw.path,
        relocate: raw.relocate,
        prefix: raw.prefix,
        triple: raw.triple.unwrap_or_default(),
        sysroot_path: raw.sysroot,
        sysroot_source: raw.sysroot_source.map(SourceId::new),
        rust_target: raw.rust_target,
        page_size: raw.page_size,
    }
}

/// Adds the sources of each rust artifact's toolchain (the SDK's tree and its
/// sysroot's tree) to the artifact's extra sources: the artifact depends on
/// them, so it waits for their materialization and rebuilds when they change.
pub(crate) fn add_toolchain_sources(artifacts: &mut [ArtifactSpec], toolchains: &[ToolchainSpec]) {
    for artifact in artifacts {
        let Some(id) = &artifact.toolchain else {
            continue;
        };
        let Some(toolchain) = toolchains.iter().find(|toolchain| toolchain.id == *id) else {
            continue;
        };
        let more = toolchain
            .root_source
            .iter()
            .chain(toolchain.sysroot_source.iter())
            .cloned()
            .collect::<Vec<_>>();
        if more.is_empty() {
            continue;
        }
        let own = artifact
            .source
            .as_ref()
            .map(|source| source.id.as_str().to_string());
        artifact.extra_sources = merge_sources(
            std::mem::take(&mut artifact.extra_sources),
            more,
            own.as_deref(),
        );
    }
}

/// Sorted, without duplicates, and without `own`.
fn merge_sources(extra: Vec<SourceId>, more: Vec<SourceId>, own: Option<&str>) -> Vec<SourceId> {
    let mut merged = extra
        .into_iter()
        .chain(more)
        .filter(|id| Some(id.as_str()) != own)
        .collect::<Vec<_>>();
    merged.sort();
    merged.dedup();
    merged
}
