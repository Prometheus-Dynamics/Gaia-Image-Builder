pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyArchiveMemberSpec, AssemblyArchiveSpec, AssemblyDirSpec, AssemblyFileSpec,
    AssemblySymlinkSpec, AssemblyTreeSpec, ImageAssemblySpec, ResolvedBuildSpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use support::{assembly_file, provider_catalogs, test_spec};

fn run_assembly(spec: &ResolvedBuildSpec) {
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
}

struct Entry {
    name: String,
    kind: u8,
    mode: String,
    link: String,
    contents: Vec<u8>,
}

/// Parses ustar entries, checking each header's checksum.
fn parse_tar(bytes: &[u8]) -> Vec<Entry> {
    assert_eq!(bytes.len() % 512, 0);
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            return entries;
        }
        let field = |range: std::ops::Range<usize>| {
            String::from_utf8(header[range].to_vec())
                .expect("utf-8")
                .trim_end_matches(['\0', ' '])
                .to_string()
        };
        assert_eq!(&header[257..263], b"ustar\0");
        let mut summed = header.to_vec();
        summed[148..156].copy_from_slice(b"        ");
        let expected: u32 = summed.iter().map(|byte| *byte as u32).sum();
        assert_eq!(
            u32::from_str_radix(&field(148..156), 8).expect("checksum"),
            expected
        );
        let size = u64::from_str_radix(&field(124..136), 8).expect("size") as usize;
        let start = offset + 512;
        entries.push(Entry {
            name: field(0..100),
            kind: header[156],
            mode: field(100..108),
            link: field(157..257),
            contents: bytes[start..start + size].to_vec(),
        });
        offset = start + size.div_ceil(512) * 512;
    }
    panic!("tar archive is missing its end blocks");
}

fn names(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.name.as_str()).collect()
}

fn tree(id: &str, path: &Path) -> AssemblyTreeSpec {
    AssemblyTreeSpec {
        strip: false,
        id: id.into(),
        path: path.display().to_string().into(),
    }
}

/// Fixture files for the overlay tree, written outside the build dir.
fn overlay_fixtures(root: &Path) -> PathBuf {
    let dir = root.join("fixtures");
    fs::create_dir_all(&dir).expect("fixtures");
    fs::write(dir.join("a.dtbo"), b"alpha").expect("a");
    fs::write(dir.join("zz.dtbo"), b"zeta").expect("zz");
    fs::write(dir.join("b.dtbo"), b"beta").expect("b");
    fs::write(dir.join("run.sh"), b"#!/bin/sh\n").expect("run");
    dir
}

/// Assembly entries that fill the `overlays` tree: a subdirectory, files
/// (one executable), and a link to `a.dtbo`.
fn overlay_entries(fixtures: &Path, spec_assembly: &mut ImageAssemblySpec) {
    let mut run = assembly_file("overlays", fixtures.join("run.sh").display(), "run.sh");
    run.mode = Some("0755".into());
    spec_assembly.dirs.push(AssemblyDirSpec {
        tree: "overlays".into(),
        path: "sub".into(),
        mode: None,
    });
    spec_assembly.files.push(assembly_file(
        "overlays",
        fixtures.join("a.dtbo").display(),
        "a.dtbo",
    ));
    spec_assembly.files.push(assembly_file(
        "overlays",
        fixtures.join("zz.dtbo").display(),
        "zz.dtbo",
    ));
    spec_assembly.files.push(assembly_file(
        "overlays",
        fixtures.join("b.dtbo").display(),
        "sub/b.dtbo",
    ));
    spec_assembly.files.push(run);
    spec_assembly.symlinks.push(AssemblySymlinkSpec {
        tree: "overlays".into(),
        path: "link".into(),
        target: "a.dtbo".into(),
    });
}

fn archive(output: &Path, members: Vec<AssemblyArchiveMemberSpec>) -> AssemblyArchiveSpec {
    AssemblyArchiveSpec {
        id: "members".into(),
        output: output.display().to_string().into(),
        members,
    }
}

#[cfg(unix)]
#[test]
fn tree_member_archives_every_entry_in_order_with_modes_and_links() {
    let scratch = TempDir::new("gaia-exec-archive-tree");
    let mut spec = test_spec(&scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let out = build_dir.join("tree-members.tar");
    let fixtures = overlay_fixtures(scratch.path());
    let mut assembly = ImageAssemblySpec {
        trees: vec![tree("overlays", &build_dir.join("overlays-tree"))],
        archives: vec![archive(
            &out,
            vec![AssemblyArchiveMemberSpec {
                tree: Some("overlays".into()),
                prefix: "boot/overlays/".into(),
                ..AssemblyArchiveMemberSpec::default()
            }],
        )],
        ..ImageAssemblySpec::default()
    };
    overlay_entries(&fixtures, &mut assembly);
    spec.image.assembly = Some(assembly);

    run_assembly(&spec);

    let entries = parse_tar(&fs::read(&out).expect("archive"));
    assert_eq!(
        names(&entries),
        vec![
            "boot/overlays/a.dtbo",
            "boot/overlays/link",
            "boot/overlays/run.sh",
            "boot/overlays/sub/",
            "boot/overlays/sub/b.dtbo",
            "boot/overlays/zz.dtbo",
        ]
    );
    assert_eq!(entries[0].kind, b'0');
    assert_eq!(entries[0].mode, "0000644");
    assert_eq!(entries[0].contents, b"alpha");
    assert_eq!(entries[1].kind, b'2');
    assert_eq!(entries[1].link, "a.dtbo");
    assert_eq!(entries[2].mode, "0000755");
    assert_eq!(entries[3].kind, b'5');
    assert_eq!(entries[4].contents, b"beta");
}

#[cfg(unix)]
#[test]
fn tree_glob_and_dir_members_respect_exclude_and_prefixes() {
    let scratch = TempDir::new("gaia-exec-archive-globs");
    let mut spec = test_spec(&scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let out = build_dir.join("glob-members.tar");
    let fixtures = overlay_fixtures(scratch.path());
    let sources = scratch.join("glob-sources");
    fs::create_dir_all(&sources).expect("sources");
    fs::write(sources.join("x.txt"), b"x").expect("x");
    fs::write(sources.join("y.txt"), b"y").expect("y");
    let mut assembly = ImageAssemblySpec {
        trees: vec![tree("overlays", &build_dir.join("overlays-tree"))],
        archives: vec![archive(
            &out,
            vec![
                AssemblyArchiveMemberSpec {
                    tree: Some("overlays".into()),
                    prefix: "t/".into(),
                    exclude: vec!["zz*".into(), "link".into()],
                    ..AssemblyArchiveMemberSpec::default()
                },
                AssemblyArchiveMemberSpec {
                    name: "glob/".into(),
                    src_glob: Some(sources.join("*.txt").display().to_string().into()),
                    exclude: vec!["y.txt".into()],
                    ..AssemblyArchiveMemberSpec::default()
                },
                AssemblyArchiveMemberSpec {
                    src_dir: Some(sources.display().to_string().into()),
                    prefix: "dir/".into(),
                    ..AssemblyArchiveMemberSpec::default()
                },
            ],
        )],
        ..ImageAssemblySpec::default()
    };
    overlay_entries(&fixtures, &mut assembly);
    spec.image.assembly = Some(assembly);

    run_assembly(&spec);

    let entries = parse_tar(&fs::read(&out).expect("archive"));
    assert_eq!(
        names(&entries),
        vec![
            "t/a.dtbo",
            "t/run.sh",
            "t/sub/",
            "t/sub/b.dtbo",
            "glob/x.txt",
            "dir/x.txt",
            "dir/y.txt",
        ]
    );
    assert_eq!(entries[6].contents, b"y");
}

#[cfg(unix)]
#[test]
fn tree_member_waits_for_the_file_that_fills_its_tree() {
    let scratch = TempDir::new("gaia-exec-archive-order");
    let mut spec = test_spec(&scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let out = build_dir.join("ordered-members.tar");
    let source = scratch.join("filled.src");
    fs::write(&source, b"filled").expect("source");
    let filled_tree = build_dir.join("filled-tree");
    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![tree("filled", &filled_tree)],
        files: vec![assembly_file("filled", source.display(), "filled.dtbo")],
        archives: vec![archive(
            &out,
            vec![AssemblyArchiveMemberSpec {
                tree: Some("filled".into()),
                prefix: "boot/".into(),
                ..AssemblyArchiveMemberSpec::default()
            }],
        )],
        ..ImageAssemblySpec::default()
    });

    run_assembly(&spec);

    let entries = parse_tar(&fs::read(&out).expect("archive"));
    assert_eq!(names(&entries), vec!["boot/filled.dtbo"]);
    assert_eq!(entries[0].contents, b"filled");
}

#[cfg(unix)]
#[test]
fn file_src_glob_exclude_skips_named_matches() {
    let scratch = TempDir::new("gaia-exec-file-exclude");
    let mut spec = test_spec(&scratch);
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let sources = scratch.join("exclude-sources");
    fs::create_dir_all(&sources).expect("sources");
    fs::write(sources.join("a.dtbo"), b"a").expect("a");
    fs::write(sources.join("b.dtbo"), b"b").expect("b");
    let rootfs = build_dir.join("exclude-tree");
    fs::create_dir_all(&rootfs).expect("rootfs");
    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![tree("rootfs", &rootfs)],
        files: vec![AssemblyFileSpec {
            include: Vec::new(),
            strip: false,
            tree: "rootfs".into(),
            src: None,
            src_glob: Some(sources.join("*.dtbo").display().to_string().into()),
            exclude: vec!["b.dtbo".into()],
            content: None,
            json: None,
            dest: "overlays/".into(),
            mode: None,
            optional: false,
            src_dir: None,
            preserve_symlink: false,
            allow_missing_libs: false,
        }],
        ..ImageAssemblySpec::default()
    });

    run_assembly(&spec);

    assert!(rootfs.join("overlays/a.dtbo").is_file());
    assert!(!rootfs.join("overlays/b.dtbo").exists());
}
