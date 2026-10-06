use super::tar::TarWriter;
use super::*;
use gaia_spec::AssemblyArchiveMemberSourceSpec;

pub(super) struct AssemblyTarArchiveSummary {
    pub(super) output: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) members: Vec<AssemblyTarMemberSummary>,
}

pub(super) struct AssemblyTarMemberSummary {
    pub(super) name: String,
    /// Source file; `None` for a generated member.
    pub(super) src: Option<PathBuf>,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

/// Writes an `[[image.assembly.archives]]` entry as a deterministic ustar
/// file and publishes it atomically.
pub(super) fn execute_assembly_archive(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    archive: &gaia_spec::AssemblyArchiveSpec,
) -> Result<AssemblyTarArchiveSummary, String> {
    let output = roots.resolve_path(spec, &archive.output)?;
    if let Some(parent) = output.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly archive output dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    let temp = temporary_assembly_output_path(&output);
    let members = write_archive(spec, roots, archive, &temp).inspect_err(|_| {
        let _ = std_fs::remove_file(&temp);
    })?;
    publish_assembly_output(&temp, &output)?;
    Ok(AssemblyTarArchiveSummary {
        bytes: file_len(&output)?,
        sha256: file_sha256(&output)?,
        output,
        members,
    })
}

fn write_archive(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    archive: &gaia_spec::AssemblyArchiveSpec,
    temp: &Path,
) -> Result<Vec<AssemblyTarMemberSummary>, String> {
    let file = std_fs::File::create(temp).map_err(|error| {
        format!(
            "failed to create assembly archive '{}': {error}",
            temp.display()
        )
    })?;
    let mut writer = TarWriter::new(std::io::BufWriter::new(file));
    let mut members = Vec::new();
    for member in &archive.members {
        let context = |error: String| {
            format!(
                "assembly archive '{}' member '{}': {error}",
                archive.id, member.name
            )
        };
        let mut hasher = Sha256::new();
        let (src, bytes) = match member.source() {
            Some(AssemblyArchiveMemberSourceSpec::File(src)) => {
                let src = roots.resolve_path(spec, src).map_err(context)?;
                let mut reader = std_fs::File::open(&src).map_err(|error| {
                    context(format!("failed to open '{}': {error}", src.display()))
                })?;
                let bytes = file_len(&src).map_err(context)?;
                writer
                    .append(&member.name, bytes, &mut reader, |chunk| {
                        hasher.update(chunk)
                    })
                    .map_err(context)?;
                (Some(src), bytes)
            }
            Some(AssemblyArchiveMemberSourceSpec::Generated(entries)) => {
                let contents = render_env_file(spec, roots, entries).map_err(context)?;
                let bytes = contents.len() as u64;
                writer
                    .append(&member.name, bytes, &mut contents.as_bytes(), |chunk| {
                        hasher.update(chunk)
                    })
                    .map_err(context)?;
                (None, bytes)
            }
            None => {
                return Err(context("must set exactly one of src or entries".into()));
            }
        };
        members.push(AssemblyTarMemberSummary {
            name: member.name.clone(),
            src,
            bytes,
            sha256: hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        });
    }
    writer
        .finish()?
        .into_inner()
        .map_err(|error| format!("failed to flush assembly archive: {error}"))?
        .sync_all()
        .map_err(|error| {
            format!(
                "failed to sync assembly archive '{}': {error}",
                temp.display()
            )
        })?;
    Ok(members)
}

/// Renders `KEY=value` lines. `${assembly.sha256:<path>}` becomes the hex
/// sha256 of that file; values are single-quoted unless they consist only of
/// shell-safe characters, so the file can be sourced by `sh`.
pub(super) fn render_env_file(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    entries: &[(String, String)],
) -> Result<String, String> {
    let mut contents = String::new();
    for (key, value) in entries {
        let value = gaia_spec::assembly_digest_tokens(value, |token| {
            let path = roots.resolve_path(spec, &token.path)?;
            if !path.is_file() {
                return Err(format!(
                    "'${{assembly.sha256:{}}}' file '{}' does not exist",
                    token.path,
                    path.display()
                ));
            }
            file_sha256(&path)
        })
        .map_err(|error| format!("entry '{key}': {error}"))?;
        contents.push_str(key);
        contents.push('=');
        contents.push_str(&shell_quote(&value));
        contents.push('\n');
    }
    Ok(contents)
}

pub(super) fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
                )
        });
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(super) fn record_archive_state(
    state: &mut KeyValueState,
    archive_index: usize,
    archive: &gaia_spec::AssemblyArchiveSpec,
    summary: &AssemblyTarArchiveSummary,
) {
    let archive_state = AssemblyStateKey::new("archives", archive_index);
    state.insert(archive_state.field("id"), archive.id.as_str());
    state.insert(archive_state.field("format"), "tar");
    state.insert(
        archive_state.field("output"),
        summary.output.display().to_string(),
    );
    state.insert(archive_state.field("bytes"), summary.bytes);
    state.insert(archive_state.field("sha256"), &summary.sha256);
    state.insert(archive_state.field("member_count"), summary.members.len());
    for (member_index, member) in summary.members.iter().enumerate() {
        let field = |name: &str| archive_state.child_field("member", member_index + 1, name);
        state.insert(field("name"), &member.name);
        match &member.src {
            Some(src) => state.insert(field("src"), src.display().to_string()),
            None => state.insert(field("generated"), true),
        }
        state.insert(field("bytes"), member.bytes);
        state.insert(field("sha256"), &member.sha256);
    }
}
