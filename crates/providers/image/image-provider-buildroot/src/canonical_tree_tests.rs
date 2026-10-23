use super::*;
use crate::ram_tree::{WorkDir, place_work_dir};
use crate::ram_tree_dir::tree_dir;
use gaia_testkit::TempDir;

fn request(build_id: &str, identity: &str) -> CanonicalRequest {
    CanonicalRequest {
        build_id: build_id.to_string(),
        identity: identity.to_string(),
    }
}

fn ram_policy() -> ImageExecutionPolicy {
    ImageExecutionPolicy {
        work_dir: gaia_spec::BuildrootWorkDirPolicySpec {
            work_dir: "ram".to_string(),
            ram_budget: Some("60G".to_string()),
            keep_ram_tree: true,
        },
        ..ImageExecutionPolicy::default()
    }
}

/// A checkout's `<build>/image/buildroot-output`, with a small recorded RAM
/// tree size so a RAM build fits.
fn checkout(root: &Path, name: &str) -> PathBuf {
    let output = root.join(name).join("build/image/buildroot-output");
    fs::create_dir_all(output.parent().expect("image dir")).expect("image dir");
    fs::write(
        output
            .parent()
            .expect("image dir")
            .join(".gaia-ram-tree-size"),
        "1048576",
    )
    .expect("size");
    output
}

#[test]
fn the_identity_is_the_same_whatever_the_checkout_path() {
    let one = "external_tree=/work/one/ext\nfragment=abc\n";
    let two = "external_tree=/work/two/ext\nfragment=abc\n";
    assert_eq!(
        workspace_neutral(one.to_string(), "/work/one"),
        workspace_neutral(two.to_string(), "/work/two")
    );
    // A different fragment is a different build.
    assert_ne!(
        workspace_neutral("fragment=abc\n".to_string(), "/work/one"),
        workspace_neutral("fragment=def\n".to_string(), "/work/one")
    );
}

#[test]
fn the_path_is_named_by_the_build_and_its_identity() {
    let base = Path::new("/dev/shm");
    let tree = canonical_tree_dir(base, &request("img", "defconfig=a\n"));
    let hash = tree
        .parent()
        .and_then(Path::file_name)
        .expect("hash dir")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        tree.parent().and_then(Path::parent),
        Some(Path::new("/dev/shm/gaia"))
    );
    assert_eq!(tree.file_name().expect("name"), "buildroot-output");
    assert_eq!(hash.len(), 12, "{hash:?}");
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        canonical_tree_dir(base, &request("img", "defconfig=a\n")),
        tree
    );
    assert_ne!(
        canonical_tree_dir(base, &request("other", "defconfig=a\n")),
        tree
    );
    assert_ne!(
        canonical_tree_dir(base, &request("img", "defconfig=b\n")),
        tree
    );
}

#[test]
fn checkouts_of_one_build_share_the_canonical_tree_and_otherwise_differ() {
    let scratch = TempDir::new("canonical-same");
    let base = scratch.join("ram");
    fs::create_dir_all(&base).expect("base");
    let identity = request("img", "defconfig=a\n");
    let one = checkout(&scratch.join("checkouts"), "one");
    let two = checkout(&scratch.join("checkouts"), "two");

    // Canonical: the same tree, one checkout after the other.
    let first = place_work_dir(&one, &ram_policy(), &base, Some(&identity)).expect("first");
    assert!(first.ram, "{:?}", first.messages);
    assert_eq!(first.dir, canonical_tree_dir(&base, &identity));
    let shared = first.dir.clone();
    drop(first);
    let second = place_work_dir(&two, &ram_policy(), &base, Some(&identity)).expect("second");
    assert_eq!(second.dir, shared, "{:?}", second.messages);
    drop(second);

    // Off (no request): each checkout has its own tree.
    let off_one = place_work_dir(&one, &ram_policy(), &base, None).expect("off one");
    assert_eq!(off_one.dir, tree_dir(&base, &one).expect("tree"));
    let off_two = place_work_dir(&two, &ram_policy(), &base, None).expect("off two");
    assert_eq!(off_two.dir, tree_dir(&base, &two).expect("tree"));
    assert_ne!(off_one.dir, off_two.dir);
}

#[test]
fn a_second_live_build_of_the_same_id_falls_back_with_a_message() {
    let scratch = TempDir::new("canonical-busy");
    let base = scratch.join("ram");
    fs::create_dir_all(&base).expect("base");
    let identity = request("img", "defconfig=a\n");
    let one = checkout(&scratch.join("checkouts"), "one");
    let two = checkout(&scratch.join("checkouts"), "two");

    let held = place_work_dir(&one, &ram_policy(), &base, Some(&identity)).expect("held");
    assert_eq!(held.dir, canonical_tree_dir(&base, &identity));
    // The same process holds it, but it is another checkout: busy.
    let busy = place_work_dir(&two, &ram_policy(), &base, Some(&identity)).expect("busy");
    assert_eq!(busy.dir, tree_dir(&base, &two).expect("per-checkout"));
    assert!(
        busy.messages
            .iter()
            .any(|message| message.contains("in use by")
                && message.contains("per-checkout RAM tree")),
        "{:?}",
        busy.messages
    );
    assert_eq!(
        busy_holder(&base, &identity, &two).as_deref(),
        Some(
            format!(
                "build 'img' of '{}' (pid {})",
                one.display(),
                std::process::id()
            )
            .as_str()
        )
    );
    // Released when the holder finishes: the next build gets it.
    drop(held);
    let next = place_work_dir(&two, &ram_policy(), &base, Some(&identity)).expect("next");
    assert_eq!(next.dir, canonical_tree_dir(&base, &identity));
}

#[test]
fn a_stale_record_is_taken_over_and_a_reused_pid_is_not_live() {
    let scratch = TempDir::new("canonical-stale");
    let base = scratch.join("ram");
    fs::create_dir_all(&base).expect("base");
    let identity = request("img", "defconfig=a\n");
    let output = checkout(&scratch.join("checkouts"), "one");
    let hash_dir = canonical_hash_dir(&base, &identity);
    fs::create_dir_all(&hash_dir).expect("hash dir");

    // A dead pid.
    let dead = serde_json::json!({
        "pid": 4294967294u32, "start": 1, "output_dir": "/gone", "build_id": "img", "claimed": 1
    });
    fs::write(hash_dir.join(OWNER_FILE), dead.to_string()).expect("dead record");
    let claimed = claim(&base, &identity, &output).expect("claim");
    assert!(matches!(claimed, Claim::Owned(_)));
    drop(claimed);

    // Our own pid with a start time it does not have: a reused pid.
    let me = Owner::this_run(&output, "img").expect("this run");
    let reused = Owner {
        start: me.start + 1,
        output_dir: "/elsewhere/output".to_string(),
        ..me
    };
    // The claim dropped and took its empty dir with it.
    fs::create_dir_all(&hash_dir).expect("hash dir again");
    fs::write(hash_dir.join(OWNER_FILE), reused.to_json()).expect("reused record");
    assert!(busy_holder(&base, &identity, &output).is_none());
    assert!(matches!(
        claim(&base, &identity, &output).expect("claim"),
        Claim::Owned(_)
    ));
}

#[test]
fn the_record_is_removed_when_the_claim_drops_and_a_kept_tree_keeps_its_dir() {
    let scratch = TempDir::new("canonical-release");
    let base = scratch.join("ram");
    fs::create_dir_all(&base).expect("base");
    let identity = request("img", "defconfig=a\n");
    let output = checkout(&scratch.join("checkouts"), "one");
    let owned = claim(&base, &identity, &output).expect("claim");
    let Claim::Owned(owned) = owned else {
        panic!("expected a claim");
    };
    let hash_dir = canonical_hash_dir(&base, &identity);
    assert!(hash_dir.join(OWNER_FILE).is_file());
    // The tree built inside the dir keeps it.
    fs::create_dir_all(hash_dir.join("buildroot-output")).expect("tree");
    drop(owned);
    assert!(!hash_dir.join(OWNER_FILE).exists());
    assert!(hash_dir.join("buildroot-output").is_dir());
    assert!(busy_holder(&base, &identity, &output).is_none());
}

#[test]
fn a_symlinked_canonical_parent_is_refused() {
    let scratch = TempDir::new("canonical-link");
    let base = scratch.join("ram");
    fs::create_dir_all(&base).expect("base");
    let elsewhere = scratch.join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, base.join(CANONICAL_DIR)).expect("link");
    let output = checkout(&scratch.join("checkouts"), "one");
    let error = claim(&base, &request("img", "x\n"), &output)
        .err()
        .expect("refused");
    assert!(error.message.contains("symbolic link"), "{}", error.message);
    // Nothing was written through the link.
    assert!(
        fs::read_dir(&elsewhere)
            .expect("elsewhere")
            .next()
            .is_none()
    );
}

#[test]
fn a_ram_build_that_falls_back_releases_its_claim() {
    let scratch = TempDir::new("canonical-fallback");
    let base = scratch.join("ram");
    fs::create_dir_all(&base).expect("base");
    let identity = request("img", "defconfig=a\n");
    let output = checkout(&scratch.join("checkouts"), "one");
    // Too big for the budget: the build runs on disk.
    fs::write(
        output.parent().expect("image").join(".gaia-ram-tree-size"),
        (500u64 * 1024 * 1024 * 1024).to_string(),
    )
    .expect("size");
    let work: WorkDir =
        place_work_dir(&output, &ram_policy(), &base, Some(&identity)).expect("fallback");
    assert!(!work.ram);
    assert!(work._claim.is_none());
    assert!(
        !canonical_hash_dir(&base, &identity)
            .join(OWNER_FILE)
            .exists()
    );
}
