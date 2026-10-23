use super::*;

fn unknown(text: &str) -> Vec<String> {
    unknown_config_keys(&toml::from_str(text).expect("toml"))
        .into_iter()
        .map(|key| key.path)
        .collect()
}

fn unknown_with_hints(text: &str) -> Vec<(String, String)> {
    unknown_config_keys(&toml::from_str(text).expect("toml"))
        .into_iter()
        .map(|key| (key.path, key.suggestion))
        .collect()
}

#[test]
fn known_keys_are_accepted() {
    assert!(
        unknown(
            "gaia_version = \">=2.1.0\"\nbuild_name = \"demo\"\n\
                 [workspace]\nroot_dir = \".\"\n\
                 [providers.buildroot]\noverride_check = \"warn\"\n\
                 [providers.buildroot.ccache]\nenabled = true\n\
                 [[stage.files]]\nid = \"a\"\nsrc = \"a\"\ndest = \"/a\"\n\
                 [[sources]]\nid = \"s\"\nkind = \"path\"\npath = \".\"\n\
                 identity_ignore = [\"target\"]\nrefresh = \"never\"\npin = \"locked\"\n"
        )
        .is_empty()
    );
}

#[test]
fn java_gradle_home_is_a_known_key() {
    assert!(unknown("[providers.java]\ngradle_home = \"user-cache\"\n").is_empty());
    assert_eq!(
        unknown("[providers.java]\ngradle_hme = \"user-cache\"\n"),
        vec!["providers.java.gradle_hme"]
    );
}

#[test]
fn unknown_host_tools_are_reported() {
    assert_eq!(
        unknown(
            "[providers.buildroot.host_tools]\ndefault = \"build\"\n\
                 ccache = \"system,build\"\ncache = \"system\"\n"
        ),
        vec!["providers.buildroot.host_tools.cache"]
    );
}

#[test]
fn unknown_top_level_and_section_keys_are_reported() {
    assert_eq!(
        unknown(
            "build_command = \"make\"\n\
                 [providers.buildroot]\nshared_outptu = true\n\
                 [providers.buildroot.ccache]\nsize = 1\n\
                 [[stage.files]]\nid = \"a\"\nsrc = \"a\"\ndest = \"/a\"\nowner = \"root\"\n\
                 [future_section]\nkey = 1\n"
        ),
        vec![
            "build_command",
            "future_section",
            "providers.buildroot.ccache.size",
            "providers.buildroot.shared_outptu",
            "stage.files[0].owner",
        ]
    );
}

#[test]
fn export_dir_keys_are_known() {
    assert_eq!(
        unknown(
            "[workspace]\nexport_dir = \"exports\"\n\
                 [image.output]\nexport_dir = \"~/images\"\n"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn nested_image_tables_are_checked_and_kind_keys_too() {
    assert_eq!(
        unknown(
            "[image]\nkind = \"buildroot\"\ndefconfig = \"x\"\n\
                 [image.output]\narchive_name = \"a.img\"\narchive_format = \"img.xz\"\n\
                 [image.assembly]\nwork_dir = \"w\"\n\
                 [[image.assembly.disks]]\nid = \"sd\"\noutput = \"sd.img\"\nsector_size = 512\n\
                 [[image.assembly.disks.partitions]]\nname = \"boot\"\nimage = \"b\"\nlabel = \"x\"\n"
        ),
        vec![
            "image.assembly.disks[0].partitions[0].label",
            "image.assembly.disks[0].sector_size",
            "image.output.archive_format",
        ]
    );
}

#[test]
fn assembly_archive_tables_are_checked() {
    assert_eq!(
        unknown(
            "[[image.assembly.archives]]\nid = \"b\"\noutput = \"b.tar\"\nformat = \"tar\"\n\
                 [[image.assembly.archives.members]]\nname = \"a\"\nsrc = \"a\"\nmode = \"0644\"\n\
                 [[image.assembly.archives.members]]\nname = \"m\"\nentries = [[\"K\", \"v\"]]\n\
                 [[image.assembly.archives.generated]]\nname = \"g\"\nentries = []\nquote = true\n\
                 [[image.assembly.disks]]\nid = \"sd\"\noutput = \"sd.img\"\n\
                 [[image.assembly.disks.partitions]]\nname = \"p\"\nsize = \"2G\"\nwipe = true\n\
                 [[image.assembly.transforms]]\nkind = \"zstd\"\ndest = \"x.zst\"\nlevel = 19\n"
        ),
        vec![
            "image.assembly.archives[0].format",
            "image.assembly.archives[0].generated[0].quote",
            "image.assembly.archives[0].members[0].mode",
        ]
    );
}

#[test]
fn assembly_oci_tables_are_checked() {
    assert_eq!(
        unknown(
            "[[image.assembly.oci]]\nid = \"sim\"\ndest = \"sim.oci\"\nformat = \"oci-layout\"\n\
                 compression = \"gzip\"\nref = \"sim:dev\"\n\
                 layers = [ { tar = \"a.tar\" }, { tree = \"app\", tre = \"x\" } ]\n\
                 [image.assembly.oci.config]\narchitecture = \"amd64\"\nos = \"linux\"\n\
                 entrypoint = []\nlabels = { \"k\" = \"v\" }\nworking_directory = \"/\"\n\
                 [[image.assembly.oci]]\nid = \"b\"\ndest = \"b.oci\"\nreference = \"x\"\n"
        ),
        vec![
            "image.assembly.oci[0].config.working_directory",
            "image.assembly.oci[0].layers[1].tre",
            "image.assembly.oci[1].reference",
        ]
    );
}

#[test]
fn a_source_key_is_checked_against_its_kind() {
    assert_eq!(
        unknown_with_hints(
            "[[sources]]\nid = \"s\"\nkind = \"git\"\nrepo = \"r\"\nbranh = \"main\"\n\
                 [[sources]]\nid = \"t\"\nkind = \"path\"\npath = \".\"\nrepo = \"r\"\n"
        ),
        vec![
            (
                "sources[0].branh".to_string(),
                " (did you mean 'branch'?)".to_string()
            ),
            ("sources[1].repo".to_string(), String::new()),
        ]
    );
}

#[test]
fn a_source_of_an_unknown_kind_is_not_checked() {
    assert!(unknown("[[sources]]\nid = \"s\"\nkind = \"tarball\"\nwhatever = 1\n").is_empty());
}

#[test]
fn an_artifact_key_is_checked_against_its_kind() {
    assert_eq!(
        unknown_with_hints(
            "[[artifacts]]\nid = \"a\"\nkind = \"rust\"\npackage = \"p\"\n\
                 output_path = \"o\"\nfeatures = []\nexecution.docker.imag = \"x\"\n\
                 [[artifacts]]\nid = \"b\"\nkind = \"go\"\npackage = \"p\"\nfeatures = []\n\
                 output_path = \"o\"\nbuild_args = []\n"
        ),
        vec![
            (
                "artifacts[0].execution.docker.imag".to_string(),
                " (did you mean 'image'?)".to_string()
            ),
            ("artifacts[1].build_args".to_string(), String::new()),
            ("artifacts[1].features".to_string(), String::new()),
        ]
    );
}

#[test]
fn every_kind_of_image_takes_its_own_keys_only() {
    assert!(
        unknown(
            "[image]\nkind = \"buildroot\"\nsource = \"b\"\ndefconfig = \"d\"\n\
                 [image.rootfs]\nexclude_packages = [\"x\"]\n\
                 [[image.expected_images]]\nname = \"n\"\nformat = \"tar\"\nrequired = true\n"
        )
        .is_empty()
    );
    assert!(
        unknown(
            "[image]\nkind = \"starting-point\"\nrootfs_path = \"r\"\nsource = \"s\"\n\
                 [image.packages]\nenabled = true\ninstall = [\"x\"]\n"
        )
        .is_empty()
    );
    assert_eq!(
        unknown("[image]\nkind = \"starting-point\"\ndefconfig = \"d\"\n"),
        vec!["image.defconfig"]
    );
    assert_eq!(
        unknown("[image]\nkind = \"buildroot\"\nrootfs_path = \"r\"\n"),
        vec!["image.rootfs_path"]
    );
    // A layer that leaves the kind to the layer it extends takes either.
    assert!(unknown("[image]\nrootfs_path = \"r\"\n").is_empty());
    assert!(unknown("[image]\ndefconfig = \"d\"\n").is_empty());
}

#[test]
fn a_misspelled_image_key_suggests_its_sibling() {
    assert_eq!(
        unknown_with_hints("[image]\nkind = \"starting-point\"\nrootfs_pth = \"r\"\n"),
        vec![(
            "image.rootfs_pth".to_string(),
            " (did you mean 'rootfs_path'?)".to_string()
        )]
    );
}

#[test]
fn flattened_and_custom_types_are_not_checked() {
    assert!(struct_fields::<raw::RawSourceConfig>().is_none());
    assert!(struct_fields::<raw::RawImageConfig>().is_none());
    assert!(struct_fields::<RawBuildConfig>().is_some_and(|fields| {
        fields.contains(&"gaia_version") && !fields.contains(&"unknown_keys")
    }));
}

/// The ```` ```toml ```` code blocks of a Markdown text, each with the line
/// its first line is on.
fn toml_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (index, line) in markdown.lines().enumerate() {
        let fence = line.trim_start();
        if let Some((start, text)) = open.as_mut() {
            if fence.starts_with("```") {
                blocks.push((*start, std::mem::take(text)));
                open = None;
            } else {
                text.push_str(line);
                text.push('\n');
            }
        } else if fence == "```toml" {
            open = Some((index + 2, String::new()));
        }
    }
    blocks
}

/// Every TOML file under `dir`, recursively, skipping build output a local
/// run left in an example (gitignored `build`, `out`, `.gaia`, `target`,
/// `seed`) and anything unreadable.
fn toml_files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if matches!(
            entry.file_name().to_str(),
            Some("build" | "out" | ".gaia" | "target" | "seed")
        ) {
            continue;
        }
        if path.is_dir() {
            toml_files_under(&path, out);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            out.push(path);
        }
    }
}

/// The examples' build files and the TOML blocks of the docs name no key
/// Gaia does not know. (A block that is not a whole TOML document, such
/// as a fragment with a placeholder, is skipped.)
#[test]
fn examples_and_docs_name_no_unknown_key() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut problems = Vec::new();
    let mut files = Vec::new();
    toml_files_under(&root.join("examples"), &mut files);
    for file in files {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if matches!(name, "Cargo.toml" | "pyproject.toml" | "config.toml") {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("example");
        let value: toml::Value = toml::from_str(&text).expect("example parses");
        for key in unknown_config_keys(&value) {
            problems.push(format!("{}: {}", file.display(), key.path));
        }
    }
    let mut docs = Vec::new();
    for entry in std::fs::read_dir(root.join("docs")).expect("docs") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|extension| extension == "md") {
            docs.push(path);
        }
    }
    // Blocks that show another format on purpose: the legacy config a
    // migration starts from, and the lockfile `gaia lock` writes.
    const NOT_BUILD_CONFIG: &[(&str, &str)] = &[
        ("migration-from-legacy.md", "[program.install]"),
        ("configuration.md", "[[git]]"),
        // The taplo rule that points at the schema, not a build config.
        ("configuration.md", "[[rule]]"),
    ];
    let mut checked = 0;
    for doc in docs {
        let text = std::fs::read_to_string(&doc).expect("doc");
        let name = doc.file_name().and_then(|name| name.to_str()).unwrap_or("");
        for (line, block) in toml_blocks(&text) {
            if NOT_BUILD_CONFIG
                .iter()
                .any(|(file, marker)| *file == name && block.contains(marker))
            {
                continue;
            }
            let Ok(value) = toml::from_str::<toml::Value>(&block) else {
                continue;
            };
            checked += 1;
            for key in unknown_config_keys(&value) {
                problems.push(format!("{}:{line}: {}", doc.display(), key.path));
            }
        }
    }
    assert!(checked > 0, "the docs have TOML blocks to check");
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn new_buildroot_image_and_provider_keys_are_known() {
    assert!(
        unknown(
            "build_name = \"demo\"\n\
             [execution]\noffline = true\n\
             [providers.buildroot]\nerofs_workers = \"auto\"\nmake_timeout_seconds = 0\nmake_retry_attempts = 1\n\
             [providers.rust]\nsccache = true\nstrip = true\n\
             [providers.docker]\nreuse_container = true\n\
             [image]\nkind = \"buildroot\"\nlegal_info = true\n\
             [image.signing]\nkind = \"gpg\"\nkey = \"env:KEY\"\noutputs = [\"*.img.xz\"]\n"
        )
        .is_empty(),
        "{:?}",
        unknown(
            "build_name = \"demo\"\n\
             [execution]\noffline = true\n\
             [providers.buildroot]\nerofs_workers = \"auto\"\nmake_timeout_seconds = 0\nmake_retry_attempts = 1\n\
             [providers.rust]\nsccache = true\nstrip = true\n\
             [providers.docker]\nreuse_container = true\n\
             [image]\nkind = \"buildroot\"\nlegal_info = true\n\
             [image.signing]\nkind = \"gpg\"\nkey = \"env:KEY\"\noutputs = [\"*.img.xz\"]\n"
        )
    );
}

/// Scans every `.toml` under the directories in `GAIA_UNKNOWN_KEYS_SCAN`
/// (colon-separated) for unknown keys. Run by hand against downstream
/// recipes: `GAIA_UNKNOWN_KEYS_SCAN=dir1:dir2 cargo test -p gaia-config
/// --lib scan_unknown_keys -- --ignored --nocapture`.
#[test]
#[ignore]
fn scan_unknown_keys_in_directories() {
    let Ok(dirs) = std::env::var("GAIA_UNKNOWN_KEYS_SCAN") else {
        return;
    };
    let mut problems = Vec::new();
    let mut files = 0;
    let mut stack: Vec<std::path::PathBuf> = dirs.split(':').map(Into::into).collect();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == "build" || name == "output" || name == ".gaia" || name == "target" {
                    continue;
                }
                stack.push(entry.path());
            }
        } else if path.extension().is_some_and(|ext| ext == "toml") && !path.ends_with("Cargo.toml")
        {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = toml::from_str::<toml::Value>(&text) else {
                continue;
            };
            files += 1;
            for key in unknown_config_keys(&value) {
                problems.push(format!(
                    "{}: {} ({})",
                    path.display(),
                    key.path,
                    key.suggestion
                ));
            }
        }
    }
    println!("scanned {files} toml files");
    for problem in &problems {
        println!("UNKNOWN {problem}");
    }
}

#[test]
fn image_layers_keys_and_layer_tables_are_checked() {
    // The layer names are the user's; each layer's own keys are checked.
    assert!(
        unknown(
            "[image]\nkind = \"buildroot\"\n[image.layers]\norder = [\"base\", \"app\"]\n\
             default = \"base\"\noverrides = [\"/etc/x\"]\nerofs = true\nverity = false\n\
             [image.layers.app]\npackages = [\"linux-ext-*\"]\npaths = [\"/opt\"]\n\
             installs = [\"raze-*\"]\nstage = [\"raze-*\"]\nservices = [\"raze-*\"]\n"
        )
        .is_empty()
    );
    assert_eq!(
        unknown(
            "[image]\nkind = \"buildroot\"\n[image.layers]\norder = [\"base\"]\n[image.layers.app]\npackges = [\"x\"]\n"
        ),
        vec!["image.layers.app.packges"]
    );
    assert_eq!(
        unknown("[image]\nkind = \"buildroot\"\n[image.layers]\norderr = [\"base\"]\n"),
        vec!["image.layers.orderr"]
    );
}
