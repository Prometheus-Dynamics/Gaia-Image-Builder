use super::content::{render_json, token_value};
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
    verity: &VerityOutputs,
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
    let members = write_archive(spec, roots, archive, verity, &temp).inspect_err(|_| {
        let _ = std_fs::remove_file(&temp);
    })?;
    let sha256 = publish_assembly_output(&temp, &output)?;
    Ok(AssemblyTarArchiveSummary {
        bytes: file_len(&output)?,
        sha256,
        output,
        members,
    })
}

/// One entry a tree, directory or glob member expands to.
enum ArchiveEntry {
    File {
        name: String,
        src: PathBuf,
        mode: u32,
    },
    Dir {
        name: String,
    },
    Symlink {
        name: String,
        target: String,
    },
}

fn write_archive(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    archive: &gaia_spec::AssemblyArchiveSpec,
    verity: &VerityOutputs,
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
                archive.id,
                member_label(member)
            )
        };
        let expanded = match member.source() {
            Some(AssemblyArchiveMemberSourceSpec::Tree(tree)) => {
                let root = roots
                    .tree_path(tree.as_str())
                    .map_err(context)?
                    .to_path_buf();
                let mut entries = Vec::new();
                collect_tree_entries(&root, &member.prefix, &member.exclude, &mut entries)
                    .map_err(context)?;
                Some(entries)
            }
            Some(AssemblyArchiveMemberSourceSpec::Dir(template)) => {
                let root = roots.resolve_path(spec, template).map_err(context)?;
                let mut entries = Vec::new();
                collect_tree_entries(&root, &member.prefix, &member.exclude, &mut entries)
                    .map_err(context)?;
                Some(entries)
            }
            Some(AssemblyArchiveMemberSourceSpec::Glob(pattern)) => {
                // A directory that matches is skipped: the glob archives files and links.
                let matches = gaia_spec::exclude_names(
                    gaia_spec::expand_simple_glob(spec, roots, pattern.as_str())
                        .map_err(context)?,
                    &member.exclude,
                )
                .into_iter()
                .filter(|path| {
                    !std_fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
                })
                .collect::<Vec<_>>();
                let mut entries = Vec::new();
                for path in matches {
                    let metadata = std_fs::metadata(&path).map_err(|error| {
                        context(format!("failed to read '{}': {error}", path.display()))
                    })?;
                    if !metadata.is_file() {
                        return Err(context(format!(
                            "'{}' matched by src_glob is not a file",
                            path.display()
                        )));
                    }
                    let file_name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            context(format!("'{}' has no UTF-8 file name", path.display()))
                        })?
                        .to_string();
                    entries.push(ArchiveEntry::File {
                        name: format!("{}{file_name}", member.name),
                        mode: file_mode(&metadata),
                        src: path,
                    });
                }
                Some(entries)
            }
            _ => None,
        };
        if let Some(entries) = expanded {
            append_entries(&mut writer, &entries, &mut members).map_err(context)?;
            continue;
        }
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
                let contents = render_env_file(spec, roots, entries, verity).map_err(context)?;
                let bytes = contents.len() as u64;
                writer
                    .append(&member.name, bytes, &mut contents.as_bytes(), |chunk| {
                        hasher.update(chunk)
                    })
                    .map_err(context)?;
                (None, bytes)
            }
            Some(AssemblyArchiveMemberSourceSpec::Json(json)) => {
                let contents = render_json(spec, roots, verity, json).map_err(context)?;
                let bytes = contents.len() as u64;
                writer
                    .append(&member.name, bytes, &mut contents.as_bytes(), |chunk| {
                        hasher.update(chunk)
                    })
                    .map_err(context)?;
                (None, bytes)
            }
            Some(_) => return Err(context("internal: unexpanded member source".into())),
            None => {
                return Err(context(
                    "must set exactly one of src, entries, json, tree, src_dir or src_glob".into(),
                ));
            }
        };
        members.push(AssemblyTarMemberSummary {
            name: member.name.clone(),
            src,
            bytes,
            sha256: hex_digest(hasher),
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

fn hex_digest(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// How a member is named in messages: its name, else its source.
fn member_label(member: &gaia_spec::AssemblyArchiveMemberSpec) -> String {
    if !member.name.is_empty() {
        return member.name.clone();
    }
    match member.source() {
        Some(AssemblyArchiveMemberSourceSpec::Tree(tree)) => format!("tree {}", tree.as_str()),
        Some(AssemblyArchiveMemberSourceSpec::Dir(dir)) => format!("src_dir {}", dir.as_str()),
        _ => String::new(),
    }
}

/// 0755 for an executable file, else 0644.
fn file_mode(metadata: &std_fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o111 != 0 {
        0o755
    } else {
        0o644
    }
}

/// Every entry under `root`, in sorted order, directories before their
/// contents, stored under `prefix`. Symlinks are kept as links and never
/// followed; `exclude` drops files and links by name.
fn collect_tree_entries(
    root: &Path,
    prefix: &str,
    exclude: &[String],
    entries: &mut Vec<ArchiveEntry>,
) -> Result<(), String> {
    let read = std_fs::read_dir(root)
        .map_err(|error| format!("failed to read '{}': {error}", root.display()))?;
    let mut children = Vec::new();
    for entry in read {
        children.push(
            entry
                .map_err(|error| format!("failed to read '{}': {error}", root.display()))?
                .path(),
        );
    }
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("'{}' has no UTF-8 file name", child.display()))?
            .to_string();
        let archive_name = format!("{prefix}{name}");
        let metadata = std_fs::symlink_metadata(&child)
            .map_err(|error| format!("failed to read '{}': {error}", child.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            entries.push(ArchiveEntry::Dir {
                name: format!("{archive_name}/"),
            });
            collect_tree_entries(&child, &format!("{archive_name}/"), exclude, entries)?;
            continue;
        }
        if exclude
            .iter()
            .any(|pattern| gaia_spec::wildcard_match(pattern, &name))
        {
            continue;
        }
        if file_type.is_symlink() {
            let target = std_fs::read_link(&child)
                .map_err(|error| format!("failed to read '{}': {error}", child.display()))?;
            let target = target
                .to_str()
                .ok_or_else(|| format!("symlink '{}' target is not UTF-8", child.display()))?
                .to_string();
            entries.push(ArchiveEntry::Symlink {
                name: archive_name,
                target,
            });
        } else if file_type.is_file() {
            entries.push(ArchiveEntry::File {
                name: archive_name,
                mode: file_mode(&metadata),
                src: child,
            });
        } else {
            return Err(format!(
                "'{}' is not a regular file, directory or symlink",
                child.display()
            ));
        }
    }
    Ok(())
}

/// Appends expanded entries; regular files are recorded as members with
/// their source and digest. Directories and links are not recorded.
fn append_entries(
    writer: &mut TarWriter<std::io::BufWriter<std_fs::File>>,
    entries: &[ArchiveEntry],
    members: &mut Vec<AssemblyTarMemberSummary>,
) -> Result<(), String> {
    for entry in entries {
        match entry {
            ArchiveEntry::Dir { name } => writer.append_dir(name)?,
            ArchiveEntry::Symlink { name, target } => writer.append_symlink(name, target)?,
            ArchiveEntry::File { name, src, mode } => {
                let mut reader = std_fs::File::open(src)
                    .map_err(|error| format!("failed to open '{}': {error}", src.display()))?;
                let bytes = file_len(src)?;
                let mut hasher = Sha256::new();
                writer.append_file(name, *mode, bytes, &mut reader, |chunk| {
                    hasher.update(chunk)
                })?;
                members.push(AssemblyTarMemberSummary {
                    name: name.clone(),
                    src: Some(src.clone()),
                    bytes,
                    sha256: hex_digest(hasher),
                });
            }
        }
    }
    Ok(())
}

/// Renders `KEY=value` lines. `${assembly.sha256:<path>}` becomes the hex
/// sha256 of that file, `${assembly.verity:<path>:<field>}` a field of
/// that verity transform's output, and `${assembly.size:<path>}` the size. Values are single-quoted unless they
/// consist only of shell-safe characters, so the file can be sourced by `sh`.
pub(super) fn render_env_file(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    entries: &[(String, String)],
    verity: &VerityOutputs,
) -> Result<String, String> {
    let mut contents = String::new();
    for (key, value) in entries {
        let value = gaia_spec::assembly_value_tokens(value, |token| {
            token_value(spec, roots, verity, token)
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
