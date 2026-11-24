//! Fragments of a shared file, and the product's order of them: each
//! fragment is named in an order, each name has a fragment.

use gaia_describe::{Component, Error, InstallFrom, Product};
use std::path::Path;

/// Writes `files` (relative path, text) under a new directory.
fn files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    dir
}

fn assert_names(error: &Error, file: &Path, key: &str, says: &str) {
    assert_eq!(error.file, file, "{error}");
    assert_eq!(error.key, key, "{error}");
    assert!(error.message.contains(says), "{error}");
}

const BOARD: &str = "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n";
const LEMNOS: &str = "version = 1\nname = \"lemnos\"\n[[fragments]]\nfrom = { file = \"mdev.conf.fragment\" }\nto = \"/etc/mdev.conf\"\n";
const PLAIN: &str = "version = 1\nname = \"plain\"\n";

/// A product of one image with `lemnos` and `plain` in `lists`, the
/// image's own lines `image`.
fn product(lists: &str, image: &str) -> String {
    format!(
        "version = 1\nname = \"p\"\n[sources.buildroot]\ngit = \"https://example.org/br.git\"\nref = \"1\"\n[components]\nlemnos = \"lemnos/component.toml\"\nplain = \"plain/component.toml\"\n[[images]]\nname = \"i\"\nboard = \"b.toml\"\n{lists}\nos = {{ provider = \"buildroot\", source = \"buildroot\" }}\nrecipe = \"r\"\n{image}"
    )
}

fn load(
    lists: &str,
    image: &str,
    lemnos: &str,
) -> (Result<gaia_describe::Loaded, Error>, tempfile::TempDir) {
    let dir = files(&[
        ("b.toml", BOARD),
        ("lemnos/component.toml", lemnos),
        ("lemnos/mdev.conf.fragment", "^lemnos$ root:lemnos 0660\n"),
        ("plain/component.toml", PLAIN),
        ("p.toml", &product(lists, image)),
    ]);
    let loaded = Product::load(dir.path().join("p.toml"));
    (loaded, dir)
}

const BOTH: &str = "components = [\"lemnos\", \"plain\"]";
const ORDER: &str =
    "[[images.assembled]]\npath = \"/etc/mdev.conf\"\norder = [\"lemnos\", \"os\"]\n";

#[test]
fn a_fragment_named_in_the_order_loads() {
    let (loaded, dir) = load(BOTH, ORDER, LEMNOS);
    let loaded = loaded.expect("loads");
    let image = &loaded.product.images[0];
    assert_eq!(image.assembled[0].path, "/etc/mdev.conf");
    assert_eq!(image.assembled[0].order, ["lemnos", "os"]);
    assert_eq!(image.assembled[0].rule_key, 1, "the first field by default");
    let fragment = &loaded.components["lemnos"].fragments[0];
    assert_eq!(fragment.to, "/etc/mdev.conf");
    match &fragment.from {
        InstallFrom::File(gaia_describe::FileRef::Local { file }) => {
            assert_eq!(file.path(), dir.path().join("lemnos/mdev.conf.fragment"))
        }
        other => panic!("{other:?}"),
    }
    let (loaded, _dir) = load(
        BOTH,
        "[[images.assembled]]\npath = \"/etc/mdev.conf\"\norder = [\"os\", \"lemnos\"]\nrule_key = 0\n",
        LEMNOS,
    );
    assert_eq!(
        loaded.expect("loads").product.images[0].assembled[0].rule_key,
        0
    );
}

#[test]
fn a_fragment_and_its_order_must_name_each_other() {
    let at = |dir: &tempfile::TempDir, file: &str| dir.path().join(file);
    // A fragment of a file the image does not assemble.
    let (loaded, dir) = load(BOTH, "", LEMNOS);
    assert_names(
        &loaded.expect_err("refused"),
        &at(&dir, "lemnos/component.toml"),
        "fragments[0].to",
        "the image i assembles no /etc/mdev.conf",
    );
    // A fragment whose component the order leaves out.
    let (loaded, dir) = load(
        BOTH,
        "[[images.assembled]]\npath = \"/etc/mdev.conf\"\norder = [\"os\"]\n",
        LEMNOS,
    );
    assert_names(
        &loaded.expect_err("refused"),
        &at(&dir, "lemnos/component.toml"),
        "fragments[0].to",
        "lemnos is not in the order of /etc/mdev.conf in the image i",
    );
    // A name in the order with no fragment.
    let (loaded, dir) = load(
        BOTH,
        "[[images.assembled]]\npath = \"/etc/mdev.conf\"\norder = [\"lemnos\", \"plain\", \"os\"]\n",
        LEMNOS,
    );
    assert_names(
        &loaded.expect_err("refused"),
        &at(&dir, "p.toml"),
        "images[0].assembled[0].order[1]",
        "plain gives no fragment of /etc/mdev.conf",
    );
    // A component with a fragment in the initramfs.
    let (loaded, dir) = load(
        "components = [\"plain\"]\ninitramfs = [\"lemnos\"]",
        "",
        LEMNOS,
    );
    assert_names(
        &loaded.expect_err("refused"),
        &at(&dir, "lemnos/component.toml"),
        "fragments[0].to",
        "lemnos is not in the root filesystem of the image i",
    );
}

#[test]
fn an_order_is_checked_when_the_product_is_read() {
    let fails = |lists: &str, image: &str, key: &str, says: &str| {
        let (loaded, dir) = load(lists, image, LEMNOS);
        assert_names(
            &loaded.expect_err(says),
            &dir.path().join("p.toml"),
            key,
            says,
        );
    };
    let one = |path: &str, order: &str| {
        format!("[[images.assembled]]\npath = \"{path}\"\norder = {order}\n")
    };
    fails(
        BOTH,
        &one("etc/mdev.conf", "[\"lemnos\"]"),
        "images[0].assembled[0].path",
        "`etc/mdev.conf` is not an absolute path of a file in the image",
    );
    for bad in ["/etc/", "/etc//mdev.conf", "/etc/../mdev.conf", "/./x"] {
        fails(
            BOTH,
            &one(bad, "[\"lemnos\"]"),
            "images[0].assembled[0].path",
            "is not an absolute path of a file",
        );
    }
    fails(
        BOTH,
        &format!("{ORDER}{ORDER}"),
        "images[0].assembled[1].path",
        "/etc/mdev.conf is assembled twice",
    );
    fails(
        BOTH,
        &one("/etc/mdev.conf", "[]"),
        "images[0].assembled[0].order",
        "no parts",
    );
    fails(
        BOTH,
        &one("/etc/mdev.conf", "[\"lemnos\", \"os\", \"lemnos\"]"),
        "images[0].assembled[0].order[2]",
        "`lemnos` is named twice",
    );
    fails(
        "components = [\"plain\"]",
        &one("/etc/mdev.conf", "[\"lemnos\"]"),
        "images[0].assembled[0].order[0]",
        "`lemnos` is not one of the image's components",
    );
    fails(
        "components = [\"plain\"]\ninitramfs = [\"lemnos\"]",
        &one("/etc/mdev.conf", "[\"lemnos\"]"),
        "images[0].assembled[0].order[0]",
        "`lemnos` is not in the root filesystem",
    );
    // `os` with no OS.
    let dir = files(&[
        ("b.toml", BOARD),
        (
            "p.toml",
            "version = 1\nname = \"p\"\n[[images]]\nname = \"i\"\nboard = \"b.toml\"\nrecipe = \"r\"\n[[images.assembled]]\npath = \"/etc/x\"\norder = [\"os\"]\n",
        ),
    ]);
    assert_names(
        &Product::read(dir.path().join("p.toml")).expect_err("no OS"),
        &dir.path().join("p.toml"),
        "images[0].assembled[0].order[0]",
        "`os` is the OS's file, and the image has no OS",
    );
}

#[test]
fn a_fragment_is_checked_when_the_component_is_read() {
    let fails = |text: &str, key: &str, says: &str| {
        let dir = files(&[("c/component.toml", text), ("c/f", "x\n")]);
        let path = dir.path().join("c/component.toml");
        assert_names(&Component::read(&path).expect_err(says), &path, key, says);
    };
    let head = "version = 1\nname = \"c\"\n";
    fails(
        &format!("{head}[[fragments]]\nfrom = {{ link = \"/x\" }}\nto = \"/etc/x\"\n"),
        "fragments[0].from",
        "a fragment is a file's text, not a link",
    );
    fails(
        &format!("{head}[[fragments]]\nfrom = \"bin/x\"\nto = \"/etc/x\"\n"),
        "fragments[0].from",
        "names a build output, but there is no [build]",
    );
    fails(
        &format!("{head}[[fragments]]\nfrom = {{ file = \"gone\" }}\nto = \"/etc/x\"\n"),
        "fragments[0].from.file",
        "",
    );
    fails(
        &format!(
            "{head}[[fragments]]\nfrom = {{ source = \"s\", path = \"x\" }}\nto = \"/etc/x\"\n"
        ),
        "fragments[0].from.source",
        "",
    );
    fails(
        &format!("{head}[[fragments]]\nfrom = {{ generated = \"x\" }}\nto = \"/etc/x\"\n"),
        "fragments[0].from.generated",
        "there is no [generate]",
    );
    for bad in ["etc/x", "/etc/", "/etc/../x"] {
        fails(
            &format!("{head}[[fragments]]\nfrom = {{ file = \"f\" }}\nto = \"{bad}\"\n"),
            "fragments[0].to",
            "is not an absolute path of a file in the image",
        );
    }
    fails(
        &format!(
            "{head}[[fragments]]\nfrom = {{ file = \"f\" }}\nto = \"/etc/x\"\n[[fragments]]\nfrom = {{ file = \"f\" }}\nto = \"/etc/x\"\n"
        ),
        "fragments[1].to",
        "/etc/x has another fragment of this component",
    );
    fails(
        &format!(
            "{head}[[fragments]]\nfrom = {{ file = \"f\" }}\nto = \"/etc/x\"\nmode = \"0644\"\n"
        ),
        "fragments[0].mode",
        "",
    );
}
