//! `gaia.build.rust.part`: one package's output out of a group build.
//!
//! A group build (`gaia.build.rust` with several packages) leaves each
//! package's files under its own name: `<package>/bin` and `<package>/lib`.
//! A part takes one of those directories and makes it a tree of its own, the
//! same tree a build of that package alone gives, so the steps that take a
//! build's output take a part the same way.

use crate::group;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.rust.part";

/// One package's directory out of a group build's tree.
///
/// `package` is one name: the package the group built, as a directory of the
/// group's tree. The part's tree holds that directory's contents.
#[node(
    id = "gaia.build.rust.part",
    inputs(store, group, package),
    outputs(tree, artifacts)
)]
pub fn build_rust_part(
    store: &str,
    group: Tree,
    package: &str,
    cx: &Ctx,
) -> Result<(Tree, Vec<Artifact>), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |why: String| StepError::body(NAME, why);
    let names = group::names(package).map_err(fail)?;
    let [package] = names[..] else {
        return Err(fail(format!("`{package}`: a part is one package")));
    };
    let object = group
        .object
        .object_ref()
        .ok_or_else(|| fail("the group build is not in the store".into()))?;
    let key = KeyBuilder::new(NAME, 1)
        .config("package", package)
        .input("group", object.digest)
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let from = store.path(&object).join(package);
            if !from.is_dir() {
                return Err(fail(format!("the group build has no {package}")));
            }
            copy_tree(&from, &work.dir().join("out")).map_err(|e| fail(format!("{package}: {e}")))
        })?;
    let out = made.outputs["out"];
    let artifacts = crate::artifacts_in(&store, &store.path(&out))?;
    if artifacts.is_empty() {
        return Err(fail(format!(
            "{package} has no binary or library in the group build"
        )));
    }
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        artifacts,
    ))
}

/// Every file and directory under `from`, copied to `to` with its mode.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target: PathBuf = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            gaia_store::copy_file(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_part_is_its_package_directory_with_the_modes_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let group = dir.path().join("group");
        let bin = group.join("lemnosd/bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::write(bin.join("lemnosd"), "binary").expect("write");
        std::fs::set_permissions(bin.join("lemnosd"), std::fs::Permissions::from_mode(0o755))
            .expect("mode");
        std::fs::create_dir_all(group.join("lemnos-ctl/bin")).expect("mkdir");
        std::fs::write(group.join("lemnos-ctl/bin/lemnos-ctl"), "other").expect("write");

        let part = dir.path().join("part");
        copy_tree(&group.join("lemnosd"), &part).expect("copied");
        let copied = std::fs::read(part.join("bin/lemnosd")).expect("read");
        assert_eq!(copied, b"binary");
        let mode = std::fs::metadata(part.join("bin/lemnosd"))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(
            !part.join("bin/lemnos-ctl").exists(),
            "another package's file"
        );
    }
}
