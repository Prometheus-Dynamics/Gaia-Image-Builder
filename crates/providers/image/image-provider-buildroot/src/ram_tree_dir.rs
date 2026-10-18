//! The per-user directory a RAM tree lives in (see [`tree_dir`]).

use super::*;
use sha2::{Digest, Sha256};

/// `<base>/gaia-<user>/<hash of output_dir>/buildroot-output`, where `<user>`
/// is the account name of the real user id (the uid when it has none), as
/// earlier Gaia versions named it, so existing trees are found again. `base`
/// is world-writable, so the per-user directory must be ours: created with
/// mode 0700 here, or an existing directory owned by us and not a symlink
/// (its mode is tightened to 0700). Anything else is refused, since a tree
/// there could be planted or read by another user.
pub(crate) fn tree_dir(base: &Path, output_dir: &Path) -> Result<PathBuf, ImageProviderError> {
    let uid = real_uid();
    let user_dir = base.join(format!("gaia-{}", user_dir_name(uid)));
    // Only an existing directory is checked here: a preview computes the path
    // and must not create it (see `create_private_tree_parent`).
    if fs::symlink_metadata(&user_dir).is_ok() {
        check_private_user_dir(&user_dir, uid)?;
    }
    let digest = Sha256::digest(output_dir.to_string_lossy().as_bytes());
    Ok(user_dir.join(hex(&digest[..6])).join("buildroot-output"))
}

/// The real user id.
fn real_uid() -> u32 {
    // SAFETY: getuid cannot fail and takes no arguments.
    unsafe { libc::getuid() }
}

/// The account name of `uid` from the password database, or the uid itself.
/// Unlike `$USER`, it cannot be set to another user's name.
fn user_dir_name(uid: u32) -> String {
    let mut buffer = vec![0 as libc::c_char; 4096];
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: the buffers outlive the call and their sizes are passed.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut passwd,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status == 0 && !result.is_null() && !passwd.pw_name.is_null() {
        // SAFETY: getpwuid_r succeeded, so pw_name is a NUL-terminated string
        // inside `buffer`.
        let name = unsafe { std::ffi::CStr::from_ptr(passwd.pw_name) }.to_string_lossy();
        if !name.is_empty() && !name.contains('/') {
            return name.into_owned();
        }
    }
    uid.to_string()
}

/// Creates the per-user directory of `tree` (a path from [`tree_dir`]) as a
/// 0700 directory when it is missing, and checks it (see [`tree_dir`]).
pub(crate) fn create_private_tree_parent(tree: &Path) -> Result<(), ImageProviderError> {
    let Some(user_dir) = tree.parent().and_then(Path::parent) else {
        return Ok(());
    };
    let uid = real_uid();
    if fs::symlink_metadata(user_dir).is_err() {
        use std::os::unix::fs::DirBuilderExt;
        if let Some(parent) = user_dir.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to create '{}': {error}",
                    parent.display()
                ))
            })?;
        }
        // Another process may create it between the check and this call: the
        // check below then decides.
        let _ = fs::DirBuilder::new().mode(0o700).create(user_dir);
    }
    check_private_user_dir(user_dir, uid)
}

/// Checks that an existing `dir` is a directory of `uid`, not a symlink, and
/// tightens its mode to 0700 when it is looser.
fn check_private_user_dir(dir: &Path, uid: u32) -> Result<(), ImageProviderError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let refuse = |reason: &str| {
        ImageProviderError::backend_command(format!(
            "refusing RAM tree directory '{}': {reason}; remove it or use a disk work_dir",
            dir.display()
        ))
    };
    let metadata = fs::symlink_metadata(dir)
        .map_err(|error| refuse(&format!("cannot inspect it ({error})")))?;
    if metadata.file_type().is_symlink() {
        return Err(refuse("it is a symbolic link"));
    }
    if !metadata.is_dir() {
        return Err(refuse("it is not a directory"));
    }
    if metadata.uid() != uid {
        return Err(refuse("it is not owned by you"));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| refuse(&format!("cannot set its mode to 0700 ({error})")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn tree_dir_is_a_private_directory_named_by_the_account() {
        let scratch = TempDir::new("gaia-ram-tree-dir");
        let base = scratch.join("base");
        fs::create_dir_all(&base).expect("base");
        let output = Path::new("/work/build/image/buildroot-output");
        let tree = tree_dir(&base, output).expect("tree dir");
        // SAFETY: getuid cannot fail.
        let name = user_dir_name(unsafe { libc::getuid() });
        let user_dir = base.join(format!("gaia-{name}"));
        assert!(tree.starts_with(&user_dir), "{tree:?}");
        // Computing the path creates nothing (a preview computes it too).
        assert!(!user_dir.exists());
        create_private_tree_parent(&tree).expect("create");
        let mode = fs::metadata(&user_dir)
            .expect("user dir")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        // An existing private directory is accepted as it is.
        assert_eq!(tree_dir(&base, output).expect("again"), tree);
        create_private_tree_parent(&tree).expect("existing is accepted");
    }

    #[test]
    fn tree_dir_refuses_a_symlink_and_tightens_a_loose_mode() {
        let scratch = TempDir::new("gaia-ram-tree-refuse");
        let base = scratch.join("base");
        fs::create_dir_all(&base).expect("base");
        // SAFETY: getuid cannot fail.
        let user_dir = base.join(format!("gaia-{}", user_dir_name(unsafe { libc::getuid() })));
        let elsewhere = scratch.join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("elsewhere");
        std::os::unix::fs::symlink(&elsewhere, &user_dir).expect("link");
        let output = Path::new("/work/build/image/buildroot-output");
        let error = tree_dir(&base, output).expect_err("symlink refused");
        assert!(error.message.contains("symbolic link"), "{}", error.message);

        fs::remove_file(&user_dir).expect("unlink");
        fs::create_dir_all(&user_dir).expect("shared dir");
        fs::set_permissions(&user_dir, fs::Permissions::from_mode(0o755)).expect("chmod");
        tree_dir(&base, output).expect("our own directory with a loose mode is accepted");
        let mode = fs::metadata(&user_dir)
            .expect("user dir")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "the mode is tightened");
    }
}
