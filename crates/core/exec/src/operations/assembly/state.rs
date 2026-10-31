use super::*;

pub(crate) fn assembly_state_path(spec: &ResolvedBuildSpec) -> PathBuf {
    runtime_state_dir(spec).join(gaia_spec::IMAGE_ASSEMBLY_STATE_FILE_NAME)
}

/// Writes each produced output's record into the assembly state, under
/// `produced_output.<n>.<field>`, so a reused assembly can report them.
pub(crate) fn record_produced_outputs(state: &mut KeyValueState, outputs: &[ProducedOutput]) {
    state.insert(PRODUCED_OUTPUT_COUNT_KEY, outputs.len());
    for (index, output) in outputs.iter().enumerate() {
        let number = index + 1;
        state.insert(
            format!("produced_output.{number}.kind"),
            output.kind.as_str(),
        );
        state.insert(format!("produced_output.{number}.name"), &output.name);
        state.insert(
            format!("produced_output.{number}.path"),
            output.path.display().to_string(),
        );
        state.insert(format!("produced_output.{number}.bytes"), output.bytes);
    }
}

/// The produced outputs the assembly state of `spec` recorded, or none when
/// the state is missing or predates them.
pub(crate) fn recorded_produced_outputs(spec: &ResolvedBuildSpec) -> Vec<ProducedOutput> {
    std::fs::read_to_string(assembly_state_path(spec))
        .map(|contents| parse_produced_outputs(&contents))
        .unwrap_or_default()
}

fn parse_produced_outputs(contents: &str) -> Vec<ProducedOutput> {
    let fields = KeyValueState::parse(contents).into_map();
    let count = fields
        .get(PRODUCED_OUTPUT_COUNT_KEY)
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(0);
    (1..=count)
        .filter_map(|number| {
            let field = |name: &str| fields.get(&format!("produced_output.{number}.{name}"));
            Some(ProducedOutput {
                kind: ProducedOutputKind::parse(field("kind")?)?,
                name: field("name")?.clone(),
                path: PathBuf::from(field("path")?),
                bytes: field("bytes")?.parse().ok()?,
            })
        })
        .collect()
}

const PRODUCED_OUTPUT_COUNT_KEY: &str = "produced_output_count";

pub(crate) fn image_assembly_cleanup_paths(spec: &ResolvedBuildSpec) -> Vec<PathBuf> {
    let mut paths = vec![assembly_state_path(spec)];
    let Some(assembly) = &spec.image.assembly else {
        return paths;
    };
    if let Ok(roots) = AssemblyRoots::new(spec, assembly) {
        paths.extend(image_assembly_output_cleanup_paths(spec, assembly, &roots));
    }
    dedupe_paths(paths)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct AssemblyExecutionContext {
    cleanup_paths: Vec<PathBuf>,
}

impl AssemblyExecutionContext {
    pub(super) fn new(
        spec: &ResolvedBuildSpec,
        assembly: &gaia_spec::ImageAssemblySpec,
        roots: &AssemblyRoots,
    ) -> Self {
        Self {
            cleanup_paths: image_assembly_output_cleanup_paths(spec, assembly, roots),
        }
    }

    pub(super) fn cleanup_paths(self) -> Vec<PathBuf> {
        dedupe_paths(self.cleanup_paths)
    }
}

fn image_assembly_output_cleanup_paths(
    spec: &ResolvedBuildSpec,
    assembly: &gaia_spec::ImageAssemblySpec,
    roots: &AssemblyRoots,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for tree in &assembly.trees {
        if let Ok(path) = roots.tree_path(&tree.id) {
            paths.push(path.to_path_buf());
        }
    }
    for transform in &assembly.transforms {
        let outputs = std::iter::once(&transform.dest).chain(transform.hash_dest.as_ref());
        for output in outputs {
            if let Ok(path) = roots.resolve_path(spec, output) {
                paths.push(temporary_assembly_output_path(&path));
                paths.push(temporary_assembly_backup_path(&path));
                paths.push(path);
            }
        }
    }
    for filesystem in &assembly.filesystems {
        if let Ok(path) = roots.resolve_path(spec, &filesystem.output) {
            let temp = temporary_assembly_output_path(&path);
            if matches!(
                filesystem.kind,
                gaia_spec::AssemblyFilesystemKindSpec::CpioGzip
                    | gaia_spec::AssemblyFilesystemKindSpec::CpioZstd
                    | gaia_spec::AssemblyFilesystemKindSpec::CpioXz
            ) {
                paths.push(temp.with_extension("cpio.tmp"));
            }
            paths.push(temp);
            paths.push(temporary_assembly_backup_path(&path));
            paths.push(path);
        }
        if let Some(template) = filesystem.publish_template()
            && let Ok(path) = roots.resolve_path(spec, template.as_str())
        {
            paths.push(temporary_assembly_output_path(&path));
            paths.push(temporary_assembly_backup_path(&path));
            paths.push(path);
        }
    }
    for output in assembly
        .disks
        .iter()
        .map(|disk| &disk.output)
        .chain(assembly.ubis.iter().map(|ubi| &ubi.output))
        .chain(assembly.archives.iter().map(|archive| &archive.output))
        .chain(assembly.ocis.iter().map(|oci| &oci.dest))
        .chain(assembly.packages.iter().map(|package| &package.output))
    {
        if let Ok(path) = roots.resolve_path(spec, output) {
            paths.push(temporary_assembly_output_path(&path));
            paths.push(temporary_assembly_backup_path(&path));
            paths.push(path);
        }
    }
    dedupe_paths(paths)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.iter().any(|existing| existing == &path) {
            deduped.push(path);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produced_outputs_round_trip_through_the_state_file() {
        let outputs = vec![
            ProducedOutput {
                kind: ProducedOutputKind::Filesystem,
                name: "initramfs.cpio.xz".into(),
                path: PathBuf::from("/out/images/initramfs.cpio.xz"),
                bytes: 4096,
            },
            ProducedOutput {
                kind: ProducedOutputKind::Disk,
                name: "sdcard.img".into(),
                path: PathBuf::from("/out/images/sdcard=a.img"),
                bytes: 1 << 30,
            },
        ];
        let mut state = KeyValueState::new();
        record_produced_outputs(&mut state, &outputs);
        assert_eq!(parse_produced_outputs(&state.render()), outputs);
    }

    #[test]
    fn a_state_without_produced_outputs_records_none() {
        assert!(parse_produced_outputs("kind=image-assembly\n").is_empty());
        // A record missing a field is skipped rather than half-read.
        let broken = "produced_output_count=1\nproduced_output.1.kind=disk\n";
        assert!(parse_produced_outputs(broken).is_empty());
    }
}
