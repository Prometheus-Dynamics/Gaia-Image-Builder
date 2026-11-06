//! "Not both": a program that links a surface's state crates statically
//! (its handle tables) and also loads the surface library holds two copies
//! of that state, and a handle from one is unknown to the other. Lemnos's
//! rule for `lemnos-capi`: a process links the surface in (the crate
//! without its `shared` feature) or loads `liblemnos_capi.so.1`, not both.
//!
//! What a program links comes from its artifact's crate record (written by
//! the Rust provider when the build declares a surface); what it loads is
//! the `DT_NEEDED` closure of the installed file. Libraries opened at run
//! time (`dlopen`, plugins) are not seen. Crate names compare with `-` and
//! `_` as one, as cargo's do.

use super::kernel_modules::Outcome;
use super::surfaces::{installed_rust, needed_closure};
use gaia_plan::parse_elf;
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec, RustSurfaceSpec};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

struct Surface<'a> {
    artifact: &'a ArtifactSpec,
    spec: &'a RustSurfaceSpec,
    package: &'a str,
    guest: String,
    /// The names a program needs it by: its file name and its `DT_SONAME`.
    names: BTreeSet<String>,
}

/// Checks every installed Rust program against the surfaces it loads.
pub(super) fn check(spec: &ResolvedBuildSpec, rootfs: &Path, outcome: &mut Outcome) {
    let installed = installed_rust(spec, rootfs);
    let surfaces = installed
        .iter()
        .filter_map(|(artifact, guest)| {
            let ArtifactDefinition::Rust(rust) = &artifact.definition else {
                return None;
            };
            let surface = rust.surface.as_ref()?;
            let mut names = BTreeSet::new();
            if let Some(name) = Path::new(guest).file_name() {
                names.insert(name.to_string_lossy().into_owned());
            }
            let soname = std::fs::read(rootfs.join(guest.trim_start_matches('/')))
                .ok()
                .and_then(|bytes| parse_elf(&bytes).ok())
                .and_then(|object| object.soname);
            names.extend(soname);
            Some(Surface {
                artifact,
                spec: surface,
                package: rust.package.as_str(),
                guest: guest.clone(),
                names,
            })
        })
        .collect::<Vec<_>>();
    if surfaces.is_empty() {
        return;
    }
    for (artifact, guest) in &installed {
        if surfaces
            .iter()
            .any(|surface| surface.artifact.id == artifact.id)
        {
            continue;
        }
        let Some(object) = std::fs::read(rootfs.join(guest.trim_start_matches('/')))
            .ok()
            .and_then(|bytes| parse_elf(&bytes).ok())
        else {
            continue;
        };
        let (loads, _) = needed_closure(rootfs, &object.needed);
        let loaded = surfaces
            .iter()
            .filter(|surface| surface.names.iter().any(|name| loads.contains(name)))
            .collect::<Vec<_>>();
        if loaded.is_empty() {
            continue;
        }
        let record_path = record_path(spec, artifact);
        let Some(crates) = read_record(&record_path) else {
            outcome.warning(
                "surface_crates_unrecorded",
                Some(guest),
                format!(
                    "{guest} loads surface {} but artifact '{}' has no crate record ({}), so whether it also links the surface's state is not known; rebuild it (--rebuild artifact:{})",
                    loaded[0].guest,
                    artifact.id.as_str(),
                    record_path.display(),
                    artifact.id.as_str()
                ),
            );
            continue;
        };
        for surface in loaded {
            let state = state_crates(surface);
            let linked = crates
                .iter()
                .filter(|(name, features)| {
                    state.iter().any(|crate_name| same_crate(crate_name, name))
                        && !(same_crate(name, surface.package)
                            && surface
                                .spec
                                .shared_feature
                                .as_ref()
                                .is_some_and(|feature| features.contains(feature)))
                })
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>();
            if linked.is_empty() {
                continue;
            }
            let remedy = match &surface.spec.shared_feature {
                Some(feature) => format!(
                    "turn on {}'s '{feature}' feature, or do not load the library",
                    surface.package
                ),
                None => "link the surface or load it, not both".to_string(),
            };
            outcome.error(
                "surface_state_twice",
                Some(guest),
                format!(
                    "{guest} links {} statically and loads surface {} ('{}'): the surface's state is in the process twice, and a handle from one copy is unknown to the other; {remedy}",
                    linked.join(", "),
                    surface.guest,
                    surface.artifact.id.as_str()
                ),
            );
        }
    }
}

fn same_crate(a: &str, b: &str) -> bool {
    a.replace('_', "-") == b.replace('_', "-")
}

fn state_crates<'a>(surface: &'a Surface<'a>) -> Vec<&'a str> {
    if surface.spec.state_crates.is_empty() {
        vec![surface.package]
    } else {
        surface
            .spec
            .state_crates
            .iter()
            .map(String::as_str)
            .collect()
    }
}

fn record_path(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec) -> PathBuf {
    let output = gaia_spec::resolve_workspace_path(&spec.workspace, &artifact.output.path)
        .unwrap_or_else(|_| PathBuf::from(&artifact.output.path));
    gaia_spec::rust_crates_record_path(&output)
}

/// The crates and their features, from the record; `None` when it is
/// missing or not one Gaia reads.
fn read_record(path: &Path) -> Option<Vec<(String, Vec<String>)>> {
    let text = std::fs::read_to_string(path).ok()?;
    let record: serde_json::Value = serde_json::from_str(&text).ok()?;
    let crates = record.get("crates")?.as_array()?;
    Some(
        crates
            .iter()
            .filter_map(|entry| {
                let name = entry.get("name")?.as_str()?.to_string();
                let features = entry
                    .get("features")?
                    .as_array()?
                    .iter()
                    .filter_map(|feature| feature.as_str().map(str::to_string))
                    .collect();
                Some((name, features))
            })
            .collect(),
    )
}
