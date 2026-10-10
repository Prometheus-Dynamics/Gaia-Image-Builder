use super::*;
use crate::preview::preview_buildroot;
use gaia_image_providers::{ImagePreview, PreviewCleanKind, PreviewDeletionKind};
use gaia_spec::SourceId;

/// Every file and link under `dir` with its contents, and every directory:
/// two snapshots compare equal only when nothing in the tree changed.
fn tree_snapshot(dir: &Path) -> BTreeMap<String, String> {
    let mut snapshot = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let key = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .display()
                .to_string();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path).expect("link");
                snapshot.insert(key, format!("link -> {}", target.display()));
            } else if metadata.is_dir() {
                snapshot.insert(key, "dir".to_string());
                stack.push(path);
            } else {
                snapshot.insert(
                    key,
                    format!(
                        "file {}",
                        fs::read(&path).map(|bytes| bytes.len()).unwrap_or(0)
                    ),
                );
                snapshot.insert(
                    format!(
                        "{} contents",
                        path.strip_prefix(dir).unwrap_or(&path).display()
                    ),
                    fs::read_to_string(&path).unwrap_or_default(),
                );
            }
        }
    }
    snapshot
}

/// A Buildroot source whose defconfig writes `BR2_PACKAGE_FOO=n`, whose
/// `show-info` reports foo, bar (needs foo) and baz, and whose build
/// installs each package once (as `buildroot_option_change_rebuilds_only...`).
struct Fixture {
    spec: ResolvedBuildSpec,
    buildroot_dir: PathBuf,
    output_dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = temp_path(name);
        let buildroot_dir = root.join("build/sources/buildroot");
        let output_dir = root.join("build/image/buildroot-output");
        fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
        let package = |name: &str, dependencies: &str, reverse: &str| {
            format!(
                "\"{name}\": {{\"type\": \"target\", \"name\": \"{name}\", \"virtual\": false, \
                 \"version\": \"1\", \"stamp_dir\": \"build/{name}-1\", \
                 \"dependencies\": [{dependencies}], \"reverse_dependencies\": [{reverse}]}}"
            )
        };
        fs::write(
            buildroot_dir.join("graph.json"),
            format!(
                "{{{}, {}, {}}}\n",
                package("foo", "", "\"bar\""),
                package("bar", "\"foo\"", ""),
                package("baz", "", "")
            ),
        )
        .expect("graph");
        fs::write(
            buildroot_dir.join("Makefile"),
            ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=n\\n' > $(O)/.config\n\
             olddefconfig:\n\t@true\nclean:\n\t@printf clean >> $(O)/cleaned\nshow-info:\n\t@cat graph.json\n\
             target-finalize: all\nall:\n\t@for p in foo bar baz; do test -d $(O)/build/$$p-1 || { mkdir -p $(O)/build/$$p-1 $(O)/target/usr/bin; \
             echo $$p,./usr/bin/$$p > $(O)/build/$$p-1/.files-list.txt; echo $$p > $(O)/target/usr/bin/$$p; echo $$p >> $(O)/built; \
             touch $(O)/build/$$p-1/.stamp_installed; }; done\n",
        )
        .expect("makefile");
        let mut spec = ResolvedBuildSpec::new("buildroot-preview");
        spec.workspace.root_dir = root.display().to_string();
        spec.workspace.build_dir = "build".to_string();
        Self {
            spec,
            buildroot_dir,
            output_dir,
        }
    }

    fn image(&self, option: Option<&str>) -> ImageSpec {
        ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                source: Some(SourceId::new("buildroot")),
                defconfig: Some("test_defconfig".into()),
                config_overrides: option
                    .map(|value| vec![("BR2_PACKAGE_FOO_OPTION".into(), value.into())])
                    .unwrap_or_default(),
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec::default(),
            assembly: None,
        }
    }

    fn run(&self, image: &ImageSpec) {
        let execution = test_execution();
        let policy = ImageExecutionPolicy::default();
        run_buildroot(BuildrootRunRequest {
            spec: &self.spec,
            image,
            buildroot_dir: &self.buildroot_dir,
            output_dir: &self.output_dir,
            command: test_command_context(&execution, &policy),
        })
        .expect("buildroot run");
    }

    fn preview(&self, image: &ImageSpec, policy: &ImageExecutionPolicy) -> ImagePreview {
        preview_buildroot(&self.spec, image, policy).expect("preview")
    }
}

#[test]
fn preview_of_a_config_change_names_the_rebuild_and_changes_nothing() {
    let fixture = Fixture::new("gaia-preview-rebuild");
    fixture.run(&fixture.image(Some("n")));
    let before = tree_snapshot(&fixture.output_dir);

    let preview = fixture.preview(&fixture.image(Some("y")), &ImageExecutionPolicy::default());

    assert_eq!(
        tree_snapshot(&fixture.output_dir),
        before,
        "the real tree changed"
    );
    assert_eq!(preview.clean, PreviewCleanKind::Packages, "{preview:?}");
    assert_eq!(preview.rebuilt_packages, ["bar", "foo"], "{preview:?}");
    assert!(preview.blocked.is_none());
    assert!(
        preview
            .clean_reasons
            .iter()
            .any(|reason| reason == "foo changed: BR2_PACKAGE_FOO_OPTION unset -> y"),
        "{:?}",
        preview.clean_reasons
    );
    assert!(
        preview
            .deletions
            .iter()
            .any(|deletion| deletion.kind == PreviewDeletionKind::Package
                && deletion.path.ends_with("target/usr/bin/foo")),
        "{:?}",
        preview.deletions
    );
    assert!(
        preview
            .verdict
            .starts_with("no clean, 2 packages rebuilt (bar, foo)"),
        "{}",
        preview.verdict
    );
    // Not a full clean, but it deletes installed files: --fail-on-clean trips.
    assert_ne!(preview.clean, PreviewCleanKind::Full);
    assert!(preview.trips_fail_on_clean());
}

#[test]
fn preview_of_an_unchanged_tree_cleans_nothing() {
    let fixture = Fixture::new("gaia-preview-unchanged");
    fixture.run(&fixture.image(Some("y")));
    let before = tree_snapshot(&fixture.output_dir);

    let preview = fixture.preview(&fixture.image(Some("y")), &ImageExecutionPolicy::default());

    assert_eq!(tree_snapshot(&fixture.output_dir), before);
    assert_eq!(preview.clean, PreviewCleanKind::Nothing);
    assert_eq!(
        preview.verdict,
        "no clean, 0 packages rebuilt (none), 0 deleted paths"
    );
}

#[test]
fn preview_of_an_unattributed_config_change_is_a_full_clean() {
    let fixture = Fixture::new("gaia-preview-full");
    fixture.run(&fixture.image(Some("n")));
    // Without the snapshot, the change cannot be named: the run cleans all.
    fs::remove_file(fixture.output_dir.join(".gaia-buildroot-config.last")).expect("snapshot");
    let before = tree_snapshot(&fixture.output_dir);

    let preview = fixture.preview(&fixture.image(Some("y")), &ImageExecutionPolicy::default());

    assert_eq!(tree_snapshot(&fixture.output_dir), before);
    assert_eq!(preview.clean, PreviewCleanKind::Full, "{preview:?}");
    assert!(preview.trips_fail_on_clean());
    assert!(
        preview.verdict.starts_with("FULL CLEAN of ")
            && preview.verdict.contains("effective config changed"),
        "{}",
        preview.verdict
    );
    assert!(
        preview
            .deletions
            .iter()
            .any(|deletion| deletion.kind == PreviewDeletionKind::Tree
                && deletion.path.ends_with("/target")),
        "{:?}",
        preview.deletions
    );
}

#[test]
fn preview_of_a_tree_moved_to_another_work_dir_discards_the_disk_tree_and_says_so() {
    let fixture = Fixture::new("gaia-preview-move");
    fixture.run(&fixture.image(Some("y")));
    let before = tree_snapshot(&fixture.output_dir);
    let fast = fixture.output_dir.with_file_name("fast-work");
    let policy = ImageExecutionPolicy {
        work_dir: gaia_spec::BuildrootWorkDirPolicySpec {
            work_dir: fast.display().to_string(),
            ram_budget: None,
            keep_ram_tree: true,
        },
        ..ImageExecutionPolicy::default()
    };

    let preview = fixture.preview(&fixture.image(Some("y")), &policy);

    assert_eq!(
        tree_snapshot(&fixture.output_dir),
        before,
        "the disk tree was touched"
    );
    assert!(!fast.exists(), "the work dir was created by a preview");
    assert!(
        preview
            .deletions
            .iter()
            .any(|deletion| deletion.kind == PreviewDeletionKind::Tree
                && deletion.path == fixture.output_dir.display().to_string()
                && deletion.reason.contains("disk tree moved")),
        "{:?}",
        preview.deletions
    );
    assert!(
        preview
            .sections
            .iter()
            .find(|section| section.title == "work dir")
            .is_some_and(|section| section
                .lines
                .iter()
                .any(|line| line.contains("moved the Buildroot tree"))),
        "{:?}",
        preview.sections
    );
    // The fresh tree starts empty: nothing to clean, packages come back
    // from the cache or are built.
    assert_eq!(preview.clean, PreviewCleanKind::Nothing);
}
