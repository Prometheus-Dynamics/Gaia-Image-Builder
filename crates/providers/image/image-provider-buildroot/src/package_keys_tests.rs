use super::*;

struct Fixture {
    root: PathBuf,
    buildroot: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "gaia-package-keys-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let buildroot = root.join("buildroot");
        for (path, contents) in [
            ("Makefile", "ifeq ($(BR2_OPTIMIZE_2),y)\nendif\n"),
            (
                "package/Makefile.in",
                "TARGET_CFLAGS += $(BR2_TARGET_OPTIMIZATION)\n",
            ),
            ("package/pkg-generic.mk", "# generic\n"),
            ("package/zlib/zlib.mk", "ZLIB_VERSION = 1.3\n"),
            ("package/zlib/zlib.hash", "sha256 abc zlib-1.3.tar.xz\n"),
            (
                "package/libpng/libpng.mk",
                "LIBPNG_VERSION = 1.6\nifeq ($(BR2_PACKAGE_LIBPNG_TOOLS),y)\nendif\n\
                 LIBPNG_CONF = $(BR2_PACKAGE_LIBPNG_CONFIG_FILE)\n",
            ),
            (
                "package/local-app/local-app.mk",
                "LOCAL_APP_SITE_METHOD = local\n",
            ),
            ("package/app/app.mk", "APP_VERSION = 1\n"),
            ("files/libpng.conf", "a\n"),
        ] {
            let path = buildroot.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, contents).expect("file");
        }
        Self { root, buildroot }
    }

    fn output(&self, name: &str, config: &str) -> PathBuf {
        let output = self.root.join(name);
        fs::create_dir_all(&output).expect("output");
        fs::write(output.join(".config"), config).expect("config");
        output
    }

    fn keys(&self, output: &Path, graph: &PackageGraph) -> BTreeMap<String, Option<String>> {
        package_keys(&KeyInputs {
            buildroot_dir: &self.buildroot,
            output_dir: output,
            graph,
            execution_identity: "host:test",
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn graph() -> PackageGraph {
    let mut graph = PackageGraph::default();
    for (name, dependencies) in [
        ("zlib", &[][..]),
        ("libpng", &["zlib"][..]),
        ("local-app", &["zlib"][..]),
        ("app", &["local-app"][..]),
    ] {
        graph.packages.insert(
            name.to_string(),
            PackageInfo {
                kind: "target".to_string(),
                version: Some("1".to_string()),
                stamp_dir: Some(format!("build/{name}-1")),
                package_dir: Some(format!("package/{name}")),
                sources: vec![format!("{name}-1.tar.xz")],
                hash_files: if name == "zlib" {
                    vec!["package/zlib/zlib.hash".to_string()]
                } else {
                    Vec::new()
                },
                dependencies: dependencies.iter().map(|d| d.to_string()).collect(),
                ..PackageInfo::default()
            },
        );
    }
    graph
}

const CONFIG: &str = "BR2_aarch64=y\nBR2_OPTIMIZE_2=y\nBR2_PACKAGE_ZLIB=y\nBR2_PACKAGE_LIBPNG=y\n\
                      BR2_PACKAGE_LIBPNG_CONFIG_FILE=\"files/libpng.conf\"\n\
                      BR2_DEFCONFIG=\"/work/a/defconfig\"\nBR2_PACKAGE_HTOP=y\n";

#[test]
fn keys_do_not_depend_on_where_the_tree_is() {
    let fixture = Fixture::new("location");
    let first = fixture.output("first", CONFIG);
    let second = fixture.output(
        "elsewhere",
        &CONFIG.replace("/work/a/defconfig", "/work/b/other.defconfig"),
    );
    let keys = fixture.keys(&first, &graph());
    assert!(keys["zlib"].is_some() && keys["libpng"].is_some());
    // An unrelated package selection (htop) does not matter either.
    assert_eq!(keys, fixture.keys(&second, &graph()));
}

#[test]
fn local_packages_and_their_dependents_have_no_key() {
    let fixture = Fixture::new("local");
    let keys = fixture.keys(&fixture.output("out", CONFIG), &graph());
    assert_eq!(keys["local-app"], None);
    assert_eq!(keys["app"], None);
}

#[test]
fn keys_follow_package_files_settings_and_dependencies() {
    let fixture = Fixture::new("inputs");
    let output = fixture.output("out", CONFIG);
    let before = fixture.keys(&output, &graph());

    // A referenced file's content (not its path).
    fs::write(fixture.buildroot.join("files/libpng.conf"), "b\n").expect("conf");
    let after = fixture.keys(&output, &graph());
    assert_eq!(before["zlib"], after["zlib"]);
    assert_ne!(before["libpng"], after["libpng"]);

    // A dependency's definition changes its dependents' keys.
    fs::write(
        fixture.buildroot.join("package/zlib/zlib.hash"),
        "sha256 def\n",
    )
    .expect("hash");
    let changed = fixture.keys(&output, &graph());
    assert_ne!(after["zlib"], changed["zlib"]);
    assert_ne!(after["libpng"], changed["libpng"]);

    // A setting the package references, and one the infrastructure does.
    let tools = fixture.output("tools", &format!("{CONFIG}BR2_PACKAGE_LIBPNG_TOOLS=y\n"));
    let with_tools = fixture.keys(&tools, &graph());
    assert_eq!(changed["zlib"], with_tools["zlib"]);
    assert_ne!(changed["libpng"], with_tools["libpng"]);
    let optimized = fixture.output("o3", &CONFIG.replace("BR2_OPTIMIZE_2", "BR2_OPTIMIZE_3"));
    assert_ne!(changed["zlib"], fixture.keys(&optimized, &graph())["zlib"]);
}
