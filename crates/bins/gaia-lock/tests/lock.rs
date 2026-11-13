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
