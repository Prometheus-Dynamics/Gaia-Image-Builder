//! Recording the links of trimmed libraries. A build group with a trimmed
//! library links through a wrapper Gaia writes into the cargo target dir:
//! it runs the build's own linker, then, for the libraries named, keeps the
//! link's arguments and what rustc deletes once the link is done (its
//! temporary directory with the version script, and the crate's object
//! files). `link_trim` replays that link exporting less.
//!
//! Cargo rebuilds every crate when the linker changes, so a recording group
//! keeps its own target dir (see `cargo_target`) and the wrapper's path
//! never changes. Rustc infers how to call the linker (cc, clang or ld
//! style, and whether to add its own lld) from the linker's file name, so
//! the wrapper takes the name of the linker it runs: `gaia-link/bin/clang`.

use gaia_artifact_providers::{
    ArtifactExecutionBackend, ArtifactExecutionContract, ArtifactProviderError,
    ArtifactProviderErrorKind,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The directory in the cargo target dir holding the wrapper and one
/// record per library, named by its file.
pub(crate) const LINK_DIR: &str = "gaia-link";

const WRAPPER: &str = r#"#!/bin/sh
# Written by Gaia: links with the build's own linker, then keeps a record of
# the links of the libraries Gaia relinks to export less (link_record.rs).
"$GAIA_LINK_LINKER" "$@" || exit
response=
if [ $# -eq 1 ] && [ "${1#@}" != "$1" ]; then response=${1#@}; fi
args() {
    if [ -n "$response" ]; then cat "$response"; echo; else printf '%s\n' "$@"; fi
}
out=$(args "$@" | sed -n '/^-o$/{n;p;q;}')
name=${out##*/}
case " $GAIA_LINK_RECORD " in *" $name "*) ;; *) exit 0 ;; esac
record=$GAIA_LINK_DIR/$name
rm -rf "$record.new" && mkdir -p "$record.new/objects" || exit 1
printf '%s\n' "$GAIA_LINK_LINKER" > "$record.new/linker" || exit 1
if [ -n "$response" ]; then
    cp "$response" "$record.new/response" || exit 1
else
    printf '%s\0' "$@" > "$record.new/args" || exit 1
fi
args "$@" | while IFS= read -r arg; do
    case $arg in
        -Wl,--version-script=*)
            dir=${arg#-Wl,--version-script=}
            dir=${dir%/*}
            printf '%s\n' "$dir" > "$record.new/rustc-dir" || exit 1
            cp -R "$dir" "$record.new/rustc" || exit 1
            ;;
        *.rcgu.o) cp "$arg" "$record.new/objects/" || exit 1 ;;
    esac
done || exit 1
rm -rf "$record" && mv "$record.new" "$record"
"#;

/// The variables that send a cargo build's links through the wrapper:
/// `CARGO_TARGET_<TRIPLE>_LINKER` and the wrapper's own. `env` is what the
/// build already sets (a toolchain's linker comes first).
pub(crate) fn recording_env(
    target_dir: &Path,
    contract: &ArtifactExecutionContract,
    source_dir: &str,
    env: &[(String, String)],
    recorded: &[String],
) -> Result<Vec<(String, String)>, ArtifactProviderError> {
    let failed = |message: String| {
        ArtifactProviderError::new(ArtifactProviderErrorKind::RuntimeState, message)
    };
    let triple = match contract.artifact_target.as_deref() {
        Some(triple) => triple.to_string(),
        None => host_triple(source_dir).map_err(failed)?,
    };
    let variable = format!(
        "CARGO_TARGET_{}_LINKER",
        triple.replace('-', "_").to_ascii_uppercase()
    );
    // What cargo would see without Gaia's variables: the host's
    // environment, or a container's, which starts from its image's `ENV`.
    let inherited = match &contract.execution_backend {
        ArtifactExecutionBackend::Docker(docker) => image_env(&docker.image, &variable),
        ArtifactExecutionBackend::Host => std::env::var(&variable).ok(),
    };
    let linker = original_linker(&variable, &triple, Path::new(source_dir), env, inherited);
    let dir = target_dir.join(LINK_DIR);
    let name = Path::new(&linker)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cc".into());
    let wrapper = dir.join("bin").join(name);
    write_wrapper(&wrapper).map_err(failed)?;
    Ok(vec![
        (variable, wrapper.display().to_string()),
        ("GAIA_LINK_LINKER".into(), linker),
        ("GAIA_LINK_RECORD".into(), recorded.join(" ")),
        ("GAIA_LINK_DIR".into(), dir.display().to_string()),
    ])
}

/// The linker the build used before the wrapper: the build's variable (the
/// first of `env` named so), the `inherited` one (the host's, or the
/// execution image's), the cargo config's `target.<triple>.linker` nearest
/// the source, then `cc`, cargo's own default.
fn original_linker(
    variable: &str,
    triple: &str,
    source_dir: &Path,
    env: &[(String, String)],
    inherited: Option<String>,
) -> String {
    // A variable's relative path is relative to where cargo runs.
    let relative_to_source = |linker: String| {
        if linker.contains('/') && !Path::new(&linker).is_absolute() {
            source_dir.join(linker).display().to_string()
        } else {
            linker
        }
    };
    env.iter()
        .find(|(key, _)| key == variable)
        .map(|(_, value)| value.clone())
        .or(inherited)
        .map(relative_to_source)
        .or_else(|| config_linker(triple, source_dir))
        .unwrap_or_else(|| "cc".into())
}

/// `target.<triple>.linker` from the cargo config files cargo reads for
/// `source_dir`: its `.cargo/config.toml` and its parents', then
/// `$CARGO_HOME`'s; the nearest wins. A value with a `/` is relative to the
/// directory holding `.cargo`, as cargo reads it.
fn config_linker(triple: &str, source_dir: &Path) -> Option<String> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cargo")));
    let dirs = source_dir
        .ancestors()
        .map(|dir| dir.join(".cargo"))
        .chain(cargo_home);
    for dir in dirs {
        for name in ["config.toml", "config"] {
            let Ok(text) = fs::read_to_string(dir.join(name)) else {
                continue;
            };
            let Ok(config) = text.parse::<toml::Table>() else {
                continue;
            };
            let Some(linker) = config
                .get("target")
                .and_then(|targets| targets.get(triple))
                .and_then(|target| target.get("linker"))
                .and_then(|linker| linker.as_str())
            else {
                continue;
            };
            if linker.contains('/') && !Path::new(linker).is_absolute() {
                let base = dir.parent().unwrap_or(&dir);
                return Some(base.join(linker).display().to_string());
            }
            return Some(linker.to_string());
        }
    }
    None
}

/// `variable` as an image sets it (`ENV`), read with `docker image inspect`.
/// HeliOS's cross image sets the target's linker this way, over the cargo
/// config's.
fn image_env(image: &str, variable: &str) -> Option<String> {
    let output = Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            "{{json .Config.Env}}",
            image,
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let env: Vec<String> = serde_json::from_slice(&output.stdout).ok()?;
    let prefix = format!("{variable}=");
    env.iter()
        .find_map(|entry| entry.strip_prefix(&prefix))
        .map(str::to_string)
}

fn host_triple(source_dir: &str) -> Result<String, String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let output = Command::new(&rustc)
        .arg("-vV")
        .current_dir(source_dir)
        .output()
        .map_err(|error| format!("cannot run '{rustc} -vV': {error}"))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|host| host.trim().to_string())
        .ok_or_else(|| format!("'{rustc} -vV' names no host"))
}

/// Writes the wrapper unless it is already there as written.
fn write_wrapper(path: &Path) -> Result<(), String> {
    if fs::read_to_string(path).is_ok_and(|text| text == WRAPPER) {
        return Ok(());
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create '{}': {error}", dir.display()))?;
    let temporary = dir.join(format!(".wrapper.{}", std::process::id()));
    fs::write(&temporary, WRAPPER)
        .map_err(|error| format!("cannot write '{}': {error}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).map_err(|error| {
            format!("cannot make '{}' executable: {error}", temporary.display())
        })?;
    }
    fs::rename(&temporary, path)
        .map_err(|error| format!("cannot write '{}': {error}", path.display()))
}

/// One recorded link: the linker it ran and its arguments, rustc's
/// temporary directory as the link saw it, and where the record keeps copies.
#[derive(Debug)]
pub(crate) struct LinkRecord {
    pub(crate) dir: PathBuf,
    pub(crate) linker: String,
    pub(crate) args: Vec<String>,
    pub(crate) rustc_dir: String,
}

/// The record of `name`'s last link in `target_dir`.
pub(crate) fn read_record(target_dir: &Path, name: &str) -> Result<LinkRecord, String> {
    let dir = target_dir.join(LINK_DIR).join(name);
    let args = match fs::read_to_string(dir.join("response")) {
        Ok(text) => response_args(&text),
        Err(_) => {
            let bytes = fs::read(dir.join("args"))
                .map_err(|error| format!("no recorded link in '{}': {error}", dir.display()))?;
            bytes
                .split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect()
        }
    };
    let rustc_dir = fs::read_to_string(dir.join("rustc-dir"))
        .map_err(|error| {
            format!(
                "the recorded link in '{}' passed no version script: {error}",
                dir.display()
            )
        })?
        .trim_end_matches('\n')
        .to_string();
    let linker = fs::read_to_string(dir.join("linker"))
        .map_err(|error| {
            format!(
                "the recorded link in '{}' names no linker: {error}",
                dir.display()
            )
        })?
        .trim_end_matches('\n')
        .to_string();
    Ok(LinkRecord {
        dir,
        linker,
        args,
        rustc_dir,
    })
}

/// Rustc's response file for a GNU-style linker: one argument per line,
/// with `\` escaping the next character.
fn response_args(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut arg = String::new();
            let mut chars = line.chars();
            while let Some(char) = chars.next() {
                if char == '\\' {
                    arg.extend(chars.next());
                } else {
                    arg.push(char);
                }
            }
            arg
        })
        .collect()
}

#[cfg(test)]
#[path = "link_record_tests.rs"]
mod tests;
