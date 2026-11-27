//! A library built with `[lib] test = false` has no unit-test binary: the
//! gate does not build one for it, so a unit test written in its `src`
//! would never run. Each such crate has none, and this keeps it so: any
//! test attribute (`#[test]`, `#[tokio::test]`, `#[rstest]`,
//! `#[test_case]`, …), any `cfg` that names `test`, and the `proptest!` and
//! `quickcheck!` macros count.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// The package name, when the manifest's `[lib]` table says `test = false`.
fn lib_tests_off(manifest: &str) -> Option<String> {
    let (mut table, mut name, mut off) = (String::new(), None, false);
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            table = line.to_string();
            continue;
        }
        let compact = line.replace(' ', "");
        if table == "[package]" {
            if let Some(value) = compact.strip_prefix("name=") {
                name = Some(value.trim_matches('"').to_string());
            }
        } else if table == "[lib]" && compact == "test=false" {
            off = true;
        }
    }
    off.then(|| name.unwrap_or_default())
}

/// What in `text` makes a unit test, if anything.
fn unit_test_in(text: &str) -> Option<String> {
    for mac in ["proptest!", "quickcheck!"] {
        if text.contains(mac) {
            return Some(mac.to_string());
        }
    }
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(found) = text[at..].find('#') {
        let start = at + found;
        at = start + 1;
        let skip = |mut i: usize| {
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            i
        };
        let mut i = skip(start + 1);
        if bytes.get(i) == Some(&b'!') {
            i = skip(i + 1);
        }
        if bytes.get(i) != Some(&b'[') {
            continue;
        }
        // The attribute, to its closing bracket.
        let (mut depth, mut end) = (0, None);
        for (j, &b) in bytes.iter().enumerate().skip(i) {
            match b {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(j);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let attr: String = text[i + 1..end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let path = attr.split(['(', '=']).next().unwrap_or_default();
        let last = path.rsplit("::").next().unwrap_or_default();
        let words = || attr.split(|c: char| !(c.is_alphanumeric() || c == '_'));
        let is_test = last == "test"
            || last.starts_with("test_")
            || matches!(last, "rstest" | "proptest" | "quickcheck")
            || ((path == "cfg" || path == "cfg_attr") && words().any(|w| w == "test"));
        if is_test {
            return Some(format!("#[{attr}]"));
        }
    }
    None
}

/// Each `test = false` library under `crates` (`crates/<group>/<crate>`)
/// whose `src` holds a unit test: the crate, the file and what is there.
/// And how many such libraries there are.
fn unit_tests_with_the_build_off(crates: &Path) -> (usize, Vec<String>) {
    let (mut off, mut written) = (0, Vec::new());
    for group in std::fs::read_dir(crates).expect("crates").flatten() {
        for krate in std::fs::read_dir(group.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(manifest) = std::fs::read_to_string(krate.path().join("Cargo.toml")) else {
                continue;
            };
            let Some(name) = lib_tests_off(&manifest) else {
                continue;
            };
            off += 1;
            let mut files = Vec::new();
            rust_files(&krate.path().join("src"), &mut files);
            for file in files {
                let text = std::fs::read_to_string(&file).expect("read");
                if let Some(what) = unit_test_in(&text) {
                    written.push(format!("{name}: {what} in {}", file.display()));
                }
            }
        }
    }
    (off, written)
}

#[test]
fn a_library_without_a_unit_test_build_has_no_unit_tests() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (off, written) = unit_tests_with_the_build_off(&crates);
    assert!(off > 0, "no crate found with its unit-test build off");
    assert!(
        written.is_empty(),
        "unit tests in a library whose unit-test build is off (`[lib] test = false`), so they \
         never run; move them to tests/ or drop `test = false`:\n{}",
        written.join("\n")
    );
}

#[test]
fn a_planted_unit_test_is_found_and_its_crate_named() {
    let dir = tempfile::tempdir().expect("tempdir");
    let krate = dir.path().join("nodes/planted");
    std::fs::create_dir_all(krate.join("src/inner")).expect("mkdir");
    std::fs::write(
        krate.join("Cargo.toml"),
        "[package]\nname = \"gaia-planted\"\n\n[lib]\ntest = false\n",
    )
    .expect("write");
    std::fs::write(krate.join("src/lib.rs"), "pub fn f() {}\n").expect("write");
    let (off, written) = unit_tests_with_the_build_off(dir.path());
    assert_eq!((off, written.len()), (1, 0), "{written:?}");

    std::fs::write(
        krate.join("src/inner/mod.rs"),
        "#[cfg(test)] mod t { #[test] fn x(){} }\n",
    )
    .expect("write");
    let (_, written) = unit_tests_with_the_build_off(dir.path());
    assert_eq!(written.len(), 1, "{written:?}");
    assert!(written[0].starts_with("gaia-planted: "), "{written:?}");
}

#[test]
fn every_way_to_write_a_unit_test_is_seen() {
    for text in [
        "#[test]\nfn x() {}",
        "#[cfg(test)]\nmod tests {}",
        "#![cfg(test)]",
        "#[cfg(all(test, unix))]\nmod tests {}",
        "#[cfg_attr(test, derive(Debug))]",
        "#[tokio::test]\nasync fn x() {}",
        "#[tokio::test(flavor = \"multi_thread\")]",
        "#[async_std::test]",
        "#[rstest]\nfn x() {}",
        "#[test_case(1)]\nfn x(_: u8) {}",
        "#[test_log::test]",
        "# [ test ]\nfn x() {}",
        "# ! [ cfg ( test ) ]",
        "proptest! { fn x(a in 0..1) {} }",
        "quickcheck! { fn x(a: u8) -> bool { true } }",
    ] {
        assert!(unit_test_in(text).is_some(), "not seen: {text}");
    }
    for text in [
        "#[derive(Debug)]\nstruct X;",
        "#[cfg(unix)]\nfn x() {}",
        "#[cfg(feature = \"testing\")]\nfn x() {}",
        "#[must_use]\nfn latest() {}",
        "let t = a[0]; // # not an attribute",
    ] {
        assert!(unit_test_in(text).is_none(), "seen in: {text}");
    }
}

#[test]
fn the_manifest_reading_finds_the_switch_in_the_lib_table_only() {
    let off = |m: &str| lib_tests_off(m);
    assert_eq!(
        off("[package]\nname = \"x\"\n\n[lib]\ntest = false\n").as_deref(),
        Some("x")
    );
    assert!(off("[lib]\n# why\ntest=false\n[dependencies]\n").is_some());
    assert!(off("[lib]\npath = \"src/lib.rs\"\n").is_none());
    assert!(off("[[bin]]\nname = \"x\"\ntest = false\n").is_none());
    assert!(off("[package]\nname = \"x\"\n").is_none());
}
