//! `gaia lock` on a product whose sources are all on this machine: a git
//! repository with a branch and an annotated tag, a `file://` download and a
//! directory.

use gaia_lock::{Entry, Lock};
use gaia_lock_cmd::{Options, Tools, describe, lock, lock_path};
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

fn commit(repo: &Path, text: &str) -> String {
    write(&repo.join("file"), text);
    git(repo, &["add", "file"]);
    git(repo, &["commit", "-qm", text]);
    git(repo, &["rev-parse", "HEAD"])
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    options: Options,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    git(&repo, &["init", "-q", "-b", "main"]);
    commit(&repo, "one");
    git(&repo, &["tag", "-a", "v1", "-m", "v1"]);
    write(&root.join("blob.bin"), "bytes to hash\n");
    let product = root.join("p/product.toml");
    write(
        &root.join("p/components/c/component.toml"),
        &format!(
            "version = 1\nname = \"c\"\n[sources.main]\ngit = \"file://{repo}\"\nref = \"main\"\n[sources.tagged]\ngit = \"file://{repo}\"\nref = \"v1\"\n[sources.blob]\ndownload = \"file://{blob}\"\n[sources.files]\npath = \"files\"\n",
            repo = repo.display(),
            blob = root.join("blob.bin").display()
        ),
    );
    write(&root.join("p/components/c/files/a"), "a\n");
    write(
        &product,
        "version = 1\nname = \"p\"\n[components]\nc = \"components/c/component.toml\"\n[[images]]\nname = \"i\"\nboard = \"boards/b/board.toml\"\ncomponents = [\"c\"]\nrecipe = \"r\"\n",
    );
    write(
        &root.join("p/boards/b/board.toml"),
        "version = 1\nname = \"b\"\n[target]\narch = \"x86_64\"\nos = \"linux\"\n",
    );
    let options = Options {
        product,
        lock: None,
        update: Vec::new(),
        tools: Tools {
            git: "git".into(),
            curl: "curl".into(),
        },
        scratch: root.join("scratch"),
    };
    std::fs::create_dir_all(&options.scratch).expect("mkdir");
    Fixture {
        _dir: dir,
        root,
        repo,
        options,
    }
}

fn run(options: &Options) -> Vec<String> {
    let (new, changes) = lock(options).expect("locks");
    new.write(&lock_path(options)).expect("writes");
    let lines = describe(&changes);
    println!("{}", lines.join("\n"));
    lines
}

#[test]
fn locks_moves_only_what_is_asked() {
    let f = fixture();
    let first = git(&f.repo, &["rev-parse", "HEAD"]);
    let lines = run(&f.options);
    assert_eq!(lines.len(), 4, "{lines:?}");
    let lock = Lock::read(&lock_path(&f.options)).expect("reads");
    println!("{}", lock.render());
    assert_eq!(
        lock.get("c.main").map(Entry::resolved),
        Some(first.as_str())
    );
    // An annotated tag locks the commit it tags.
    assert_eq!(
        lock.get("c.tagged").map(Entry::resolved),
        Some(first.as_str())
    );
    let blob = gaia_store::file_digest(&f.root.join("blob.bin")).expect("digest");
    assert_eq!(
        lock.get("c.blob").map(Entry::resolved),
        Some(blob.to_string().as_str())
    );
    let Some(Entry::Tree { path, .. }) = lock.get("c.files") else {
        panic!("a tree entry");
    };
    assert_eq!(path, "components/c/files");

    // The branch moves on; running again moves nothing.
    let second = commit(&f.repo, "two");
    let before = std::fs::read_to_string(lock_path(&f.options)).expect("read");
    assert!(run(&f.options).is_empty());

    // `--update` moves that one entry: one line of the lock.
    let mut update = f.options.clone();
    update.update = vec!["c.main".into()];
    assert_eq!(run(&update), [format!("~ c.main  {first} -> {second}")]);
    let after = std::fs::read_to_string(lock_path(&f.options)).expect("read");
    let differ: Vec<(&str, &str)> = before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(
        differ,
        [(
            format!("commit = \"{first}\"").as_str(),
            format!("commit = \"{second}\"").as_str()
        )]
    );

    // A directory's entry follows its contents.
    write(&f.root.join("p/components/c/files/a"), "changed\n");
    let lines = run(&f.options);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("~ c.files"), "{lines:?}");
}

#[test]
fn a_ref_that_is_not_there_names_the_source() {
    let f = fixture();
    let component = f.root.join("p/components/c/component.toml");
    let text = std::fs::read_to_string(&component).expect("read");
    write(&component, &text.replace("ref = \"v1\"", "ref = \"v9\""));
    let e = lock(&f.options).expect_err("fails");
    println!("{e}");
    assert!(e.contains("c.tagged: ") && e.contains("no ref `v9`"), "{e}");
}

/// A shared tree: `boards/`, `components/` and `products/` side by side,
/// the component's directory source outside the product's directory.
fn shared_tree(root: &Path) -> Options {
    write(
        &root.join("devices/components/c/component.toml"),
        "version = 1\nname = \"c\"\n[sources.files]\npath = \"files\"\n",
    );
    write(&root.join("devices/components/c/files/a"), "a\n");
    write(
        &root.join("devices/boards/b/board.toml"),
        "version = 1\nname = \"b\"\n[target]\narch = \"x86_64\"\nos = \"linux\"\n",
    );
    let product = root.join("devices/products/p/product.toml");
    write(
        &product,
        "version = 1\nname = \"p\"\n[components]\nc = \"../../components/c/component.toml\"\n[[images]]\nname = \"i\"\nboard = \"../../boards/b/board.toml\"\ncomponents = [\"c\"]\nrecipe = \"r\"\n",
    );
    std::fs::create_dir_all(root.join("scratch")).expect("mkdir");
    Options {
        product,
        lock: None,
        update: Vec::new(),
        tools: Tools {
            git: "git".into(),
            curl: "curl".into(),
        },
        scratch: root.join("scratch"),
    }
}

#[test]
fn a_tree_beside_the_product_is_locked_by_a_relative_path() {
    let one = tempfile::tempdir().expect("tempdir");
    let options = shared_tree(one.path());
    run(&options);
    let path = lock_path(&options);
    let text = std::fs::read_to_string(&path).expect("the lock");
    println!("{text}");
    let Some(Entry::Tree { path: locked, .. }) =
        Lock::read(&path).expect("reads").get("c.files").cloned()
    else {
        panic!("c.files is a tree");
    };
    assert_eq!(locked, "../../components/c/files");
    let checkout = one.path().display().to_string();
    assert!(
        !text.contains(&checkout),
        "the lock names no path of the checkout: {text}"
    );
    // Locked again, nothing changed: the relative path is the one wanted.
    assert_eq!(run(&options), Vec::<String>::new());

    // The same tree in another checkout gives the same lock, byte for byte.
    let two = tempfile::tempdir().expect("tempdir");
    let other = shared_tree(two.path());
    run(&other);
    let other_text = std::fs::read_to_string(lock_path(&other)).expect("the lock");
    assert_eq!(text, other_text);
}

#[test]
fn a_changed_file_in_a_tree_beside_the_product_changes_its_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = shared_tree(dir.path());
    run(&options);
    write(&dir.path().join("devices/components/c/files/a"), "b\n");
    let lines = run(&options);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("c.files"), "{lines:?}");
}

/// A git repository with one commit at `main`, in `root`; its head.
fn repo_at(root: &Path) -> (PathBuf, String) {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    git(&repo, &["init", "-q", "-b", "main"]);
    let head = commit(&repo, "one");
    (repo, head)
}

/// A product whose one component has `sources` (TOML lines), under `dir`.
fn product(dir: &Path, sources: &str) -> Options {
    write(
        &dir.join("p/components/c/component.toml"),
        &format!("version = 1\nname = \"c\"\n{sources}"),
    );
    write(
        &dir.join("p/product.toml"),
        "version = 1\nname = \"p\"\n[components]\nc = \"components/c/component.toml\"\n[[images]]\nname = \"i\"\nboard = \"boards/b/board.toml\"\ncomponents = [\"c\"]\nrecipe = \"r\"\n",
    );
    write(
        &dir.join("p/boards/b/board.toml"),
        "version = 1\nname = \"b\"\n[target]\narch = \"x86_64\"\nos = \"linux\"\n",
    );
    std::fs::create_dir_all(dir.join("scratch")).expect("mkdir");
    Options {
        product: dir.join("p/product.toml"),
        lock: None,
        update: Vec::new(),
        tools: Tools {
            git: "git".into(),
            curl: "curl".into(),
        },
        scratch: dir.join("scratch"),
    }
}

/// A program that does not exist: any request made through it fails.
const NO_TOOL: &str = "/nonexistent/gaia-test-tool";

/// A sha256 digest of the right form, for an image that names it.
const DIGEST: &str = "sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2";
const OTHER_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000001";
const IMAGE: &str = "docker.io/library/debian:trixie-slim";

#[test]
fn a_git_ref_that_is_a_full_commit_locks_without_git() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (repo, head) = repo_at(dir.path());
    let url = format!("file://{}", repo.display());
    let upper = head.to_ascii_uppercase();
    let mut options = product(
        dir.path(),
        &format!(
            "[sources.full]\ngit = \"{url}\"\nref = \"{head}\"\n[sources.upper]\ngit = \"{url}\"\nref = \"{upper}\"\n"
        ),
    );
    options.tools.git = NO_TOOL.into();
    let (new, changes) = lock(&options).expect("locks with no git");
    assert_eq!(new.get("c.full").map(Entry::resolved), Some(head.as_str()));
    assert_eq!(new.get("c.upper").map(Entry::resolved), Some(head.as_str()));
    assert_eq!(changes.len(), 2, "{changes:?}");
}

#[test]
fn a_locked_git_source_needs_no_repository() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (repo, head) = repo_at(dir.path());
    let options = product(
        dir.path(),
        &format!(
            "[sources.main]\ngit = \"file://{}\"\nref = \"main\"\n",
            repo.display()
        ),
    );
    assert_eq!(run(&options).len(), 1);
    // The repository is gone: a lock of the unchanged product asks for nothing.
    std::fs::rename(&repo, dir.path().join("moved")).expect("rename");
    let (new, changes) = lock(&options).expect("locks without the repository");
    assert!(changes.is_empty(), "{changes:?}");
    assert_eq!(new.get("c.main").map(Entry::resolved), Some(head.as_str()));
    // And with no git program at all.
    let mut no_git = options.clone();
    no_git.tools.git = NO_TOOL.into();
    assert!(lock(&no_git).expect("locks").1.is_empty());
}

#[test]
fn a_locked_download_needs_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blob = dir.path().join("blob.bin");
    write(&blob, "bytes\n");
    let options = product(
        dir.path(),
        &format!("[sources.blob]\ndownload = \"file://{}\"\n", blob.display()),
    );
    assert_eq!(run(&options).len(), 1);
    std::fs::remove_file(&blob).expect("remove");
    let mut no_curl = options.clone();
    no_curl.tools.curl = NO_TOOL.into();
    let (_, changes) = lock(&no_curl).expect("locks without the file or curl");
    assert!(changes.is_empty(), "{changes:?}");
}

#[test]
fn an_image_written_with_its_digest_locks_without_a_registry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = product(
        dir.path(),
        &format!("[sources.root]\nimage = \"{IMAGE}@{DIGEST}\"\n"),
    );
    let mut no_curl = options.clone();
    no_curl.tools.curl = NO_TOOL.into();
    let (new, changes) = lock(&no_curl).expect("locks with no registry");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        new.get("c.root"),
        Some(&Entry::Image {
            reference: format!("{IMAGE}@{DIGEST}"),
            digest: DIGEST.into(),
        })
    );
    run(&no_curl);
    assert!(lock(&no_curl).expect("again").1.is_empty());
}

#[test]
fn an_image_without_a_digest_not_yet_locked_names_the_digest_form() {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = product(
        dir.path(),
        &format!("[sources.root]\nimage = \"{IMAGE}\"\n"),
    );
    let mut no_curl = options.clone();
    no_curl.tools.curl = NO_TOOL.into();
    let e = lock(&no_curl).expect_err("fails");
    println!("{e}");
    assert!(e.starts_with("c.root: "), "{e}");
    assert!(
        e.contains(&format!("image = \"{IMAGE}@sha256:<64 hex digits>\""))
            && e.contains("avoids this request"),
        "{e}"
    );
}

#[test]
fn an_image_without_a_digest_that_is_locked_is_not_asked_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = product(
        dir.path(),
        &format!("[sources.root]\nimage = \"{IMAGE}\"\n"),
    );
    write(
        &lock_path(&options),
        &format!(
            "# Written by `gaia lock`. Do not edit.\nversion = 1\n\n[source.\"c.root\"]\nkind = \"image\"\nreference = \"{IMAGE}\"\ndigest = \"{DIGEST}\"\n"
        ),
    );
    let mut no_curl = options.clone();
    no_curl.tools.curl = NO_TOOL.into();
    let (_, changes) = lock(&no_curl).expect("locks with no registry");
    assert!(changes.is_empty(), "{changes:?}");
    // `--update` asks again, and that needs the registry.
    no_curl.update = vec!["c.root".into()];
    let e = lock(&no_curl).expect_err("asks");
    assert!(e.contains("c.root: "), "{e}");
}

#[test]
fn a_changed_written_digest_is_locked_to_the_one_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let options = product(
        dir.path(),
        &format!("[sources.root]\nimage = \"{IMAGE}@{DIGEST}\"\n"),
    );
    write(
        &lock_path(&options),
        &format!(
            "version = 1\n\n[source.\"c.root\"]\nkind = \"image\"\nreference = \"{IMAGE}@{OTHER_DIGEST}\"\ndigest = \"{OTHER_DIGEST}\"\n"
        ),
    );
    let mut no_curl = options.clone();
    no_curl.tools.curl = NO_TOOL.into();
    let lines = run(&no_curl);
    assert_eq!(
        lines,
        [format!("~ c.root  {OTHER_DIGEST} -> {DIGEST}")],
        "{lines:?}"
    );
}

#[test]
fn check_asks_for_nothing_and_says_what_is_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (repo, _) = repo_at(dir.path());
    let blob = dir.path().join("blob.bin");
    write(&blob, "bytes\n");
    let options = product(
        dir.path(),
        &format!(
            "[sources.main]\ngit = \"file://{}\"\nref = \"main\"\n[sources.blob]\ndownload = \"file://{}\"\n[sources.root]\nimage = \"{IMAGE}@{DIGEST}\"\n",
            repo.display(),
            blob.display()
        ),
    );
    run(&options);
    // The repository and the file are gone, and no tool works.
    std::fs::remove_dir_all(&repo).expect("remove");
    std::fs::remove_file(&blob).expect("remove");
    let lock_file = lock_path(&options).display().to_string();
    let check = |extra: &[&str]| {
        let mut words: Vec<String> = vec![
            options.product.display().to_string(),
            "--lock".into(),
            lock_file.clone(),
            "--check".into(),
            "--tool".into(),
            format!("git={NO_TOOL}"),
            "--tool".into(),
            format!("curl={NO_TOOL}"),
        ];
        words.extend(extra.iter().map(|w| w.to_string()));
        gaia_lock_cmd::command::run(&words)
    };
    check(&[]).expect("a complete lock checks with no tool");

    // A source the lock lacks fails, by name, and says to run `gaia lock`.
    write(
        &options
            .product
            .parent()
            .expect("dir")
            .join("components/c/component.toml"),
        &format!(
            "version = 1\nname = \"c\"\n[sources.main]\ngit = \"file://{}\"\nref = \"main\"\n[sources.blob]\ndownload = \"file://{}\"\n[sources.root]\nimage = \"{IMAGE}@{DIGEST}\"\n[sources.extra]\nimage = \"{IMAGE}@{OTHER_DIGEST}\"\n",
            repo.display(),
            blob.display()
        ),
    );
    let (code, message) = check(&[]).expect_err("missing");
    assert_eq!(code, 1);
    assert!(message.contains("not in the lock: c.extra"), "{message}");
}
