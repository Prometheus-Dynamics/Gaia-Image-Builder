//! `gaia preview`: what `gaia run` would do, before running it. The plan is
//! the one `gaia plan` shows; each image operation that would execute is
//! previewed by its provider, without changing anything.

use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_image_providers::{ImagePreview, ImageProviderOperation};
use gaia_plan::{OperationKind, OperationReuse, PlanTarget, plan_build_with_reuse_state};
use gaia_validate::validate_spec_with_providers;

use crate::AppContext;

use super::{CommandOutcome, load_reuse_state};

/// How many deletions the human report lists before counting the rest.
const LISTED_DELETIONS: usize = 25;

/// One operation of the plan, and whether a run executes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewOperation {
    pub id: String,
    pub executes: bool,
    /// Why it executes (`code: message`) or what it reuses.
    pub reason: String,
}

/// The preview of an image operation (or why there is none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewImage {
    /// The plan operations this preview covers (prepare and build share a tree).
    pub operations: Vec<String>,
    pub provider_id: String,
    pub preview: Option<ImagePreview>,
    /// Why there is no preview, when there is none.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewReport {
    pub build_name: String,
    pub operations: Vec<PreviewOperation>,
    pub images: Vec<PreviewImage>,
    /// `--fail-on-clean`: exit 3 when a run would clean or delete.
    pub fail_on_clean: bool,
    /// `--json`: print the report as JSON.
    pub json: bool,
}

impl PreviewReport {
    /// Whether `--fail-on-clean` should fail: a full clean, or a deletion
    /// other than leftovers of an earlier clean.
    pub fn tripped(&self) -> bool {
        self.images.iter().any(|image| {
            image
                .preview
                .as_ref()
                .is_some_and(ImagePreview::trips_fail_on_clean)
        })
    }

    /// The one-line verdict, without the `preview: ` prefix.
    pub fn verdict(&self) -> String {
        let verdicts = self
            .images
            .iter()
            .filter_map(|image| {
                image
                    .preview
                    .as_ref()
                    .map(|preview| preview.verdict.clone())
            })
            .collect::<Vec<_>>();
        if verdicts.is_empty() {
            let executing = self.operations.iter().filter(|op| op.executes).count();
            return if executing == 0 {
                "nothing runs: every operation is reused".to_string()
            } else {
                "no image operation would run, 0 deleted paths".to_string()
            };
        }
        verdicts.join(" | ")
    }
}

pub fn preview_build_command(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
    targets: &[PlanTarget],
    fail_on_clean: bool,
    json: bool,
) -> CommandOutcome {
    match preview_report(context, build, options, targets, fail_on_clean, json) {
        Ok(report) => CommandOutcome::Previewed { report },
        Err(message) => CommandOutcome::Failed { message },
    }
}

fn preview_report(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
    targets: &[PlanTarget],
    fail_on_clean: bool,
    json: bool,
) -> Result<PreviewReport, String> {
    let spec =
        try_resolve_config_with_options(build, options).map_err(|error| error.to_string())?;
    preview_resolved(context, &spec, targets, fail_on_clean, json)
}

/// The preview of a resolved build (what `gaia preview` shows for it).
pub(crate) fn preview_resolved(
    context: &AppContext,
    spec: &gaia_spec::ResolvedBuildSpec,
    targets: &[PlanTarget],
    fail_on_clean: bool,
    json: bool,
) -> Result<PreviewReport, String> {
    let validation = validate_spec_with_providers(
        spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    if !validation.errors.is_empty() {
        return Err(format!(
            "refusing to preview build '{}': {} validation error(s)",
            spec.identity.display_name,
            validation.errors.len()
        ));
    }
    let reuse_state = load_reuse_state(spec);
    let plan = plan_build_with_reuse_state(
        spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
        reuse_state.as_ref(),
    );
    let plan = if targets.is_empty() {
        plan
    } else {
        plan.restrict_to(targets)?
    };

    let operations = plan
        .operations
        .iter()
        .map(|operation| match &operation.reuse {
            OperationReuse::Execute(reason) => PreviewOperation {
                id: operation.id.as_str().to_string(),
                executes: true,
                reason: format!("{}: {}", reason.code, reason.message),
            },
            OperationReuse::Reuse { source } => PreviewOperation {
                id: operation.id.as_str().to_string(),
                executes: false,
                reason: format!("reused from {source}"),
            },
        })
        .collect::<Vec<_>>();

    let image_ids = |kinds: &[OperationKind]| -> Vec<String> {
        plan.operations
            .iter()
            .filter(|operation| kinds.contains(&operation.kind))
            .map(|operation| operation.id.as_str().to_string())
            .collect()
    };
    let image_kinds = [OperationKind::PrepareImage, OperationKind::BuildImage];
    let executing_ids = plan
        .operations
        .iter()
        .filter(|operation| {
            operation.reuse.should_execute() && image_kinds.contains(&operation.kind)
        })
        .map(|operation| operation.id.as_str().to_string())
        .collect::<Vec<_>>();
    let mut images = Vec::new();
    if !executing_ids.is_empty() {
        let provider_kind = spec.image.provider_kind();
        let (provider_id, preview, note) = match context.image_catalog.find_for_kind(provider_kind)
        {
            None => (
                format!("{provider_kind:?}"),
                None,
                Some(format!("no image provider for {provider_kind:?}")),
            ),
            Some(provider) => {
                // Prepare and build are one tree: the provider previews it once.
                let operation = if plan.operations.iter().any(|operation| {
                    operation.kind == OperationKind::PrepareImage
                        && operation.reuse.should_execute()
                }) {
                    ImageProviderOperation::Prepare
                } else {
                    ImageProviderOperation::Build
                };
                let policy = gaia_exec::image_execution_policy(spec);
                let (preview, note) =
                    match provider.preview_image(spec, &spec.image, &policy, operation) {
                        Ok(Some(preview)) => (Some(preview), None),
                        Ok(None) => (None, Some("this provider has no preview".to_string())),
                        Err(error) => {
                            return Err(format!("preview of the image failed: {}", error.message));
                        }
                    };
                (provider.id().to_string(), preview, note)
            }
        };
        images.push(PreviewImage {
            operations: executing_ids,
            provider_id,
            preview,
            note,
        });
    } else {
        let reused = image_ids(&image_kinds);
        if !reused.is_empty() {
            images.push(PreviewImage {
                operations: reused,
                provider_id: String::new(),
                preview: None,
                note: Some("reused: a run does not touch the image tree".to_string()),
            });
        }
    }

    Ok(PreviewReport {
        build_name: spec.build_name().to_string(),
        operations,
        images,
        fail_on_clean,
        json,
    })
}

/// Prints the report: JSON with `--json`, otherwise the readable report.
pub(crate) fn print_preview(report: &PreviewReport) {
    if report.json {
        println!("{}", preview_json(report));
        return;
    }
    println!(
        "preview of '{}': {} operation(s), {} would execute",
        report.build_name,
        report.operations.len(),
        report.operations.iter().filter(|op| op.executes).count()
    );
    for operation in &report.operations {
        let state = if operation.executes { "run  " } else { "reuse" };
        println!("{state} {}: {}", operation.id, operation.reason);
    }
    for image in &report.images {
        println!();
        println!(
            "image {} ({}):",
            if image.provider_id.is_empty() {
                "-"
            } else {
                image.provider_id.as_str()
            },
            image.operations.join(", ")
        );
        if let Some(note) = &image.note {
            println!("  {note}");
        }
        let Some(preview) = &image.preview else {
            continue;
        };
        if let Some(reason) = &preview.blocked {
            println!("  blocked: {reason}");
        }
        for section in &preview.sections {
            println!("  {}:", section.title);
            for line in &section.lines {
                println!("    {line}");
            }
        }
        let outside = preview.deletions_outside_trash();
        println!(
            "  deletions: {outside} outside trash, {} in trash",
            preview.deletions.len() - outside
        );
        for deletion in preview.deletions.iter().take(LISTED_DELETIONS) {
            println!(
                "    {:<8} {}  ({})",
                deletion.kind.as_str(),
                deletion.path,
                deletion.reason
            );
        }
        if preview.deletions.len() > LISTED_DELETIONS {
            println!(
                "    ... and {} more (--json lists every path)",
                preview.deletions.len() - LISTED_DELETIONS
            );
        }
    }
    println!();
    println!("preview: {}", report.verdict());
}

/// The report as JSON, for scripts.
pub(crate) fn preview_json(report: &PreviewReport) -> serde_json::Value {
    serde_json::json!({
        "build": report.build_name,
        "verdict": report.verdict(),
        "fail_on_clean": report.fail_on_clean,
        "trips_fail_on_clean": report.tripped(),
        "operations": report.operations.iter().map(|operation| serde_json::json!({
            "id": operation.id,
            "executes": operation.executes,
            "reason": operation.reason,
        })).collect::<Vec<_>>(),
        "images": report.images.iter().map(|image| serde_json::json!({
            "provider": image.provider_id,
            "operations": image.operations,
            "note": image.note,
            "blocked": image.preview.as_ref().and_then(|preview| preview.blocked.clone()),
            "verdict": image.preview.as_ref().map(|preview| preview.verdict.clone()),
            "clean": image.preview.as_ref().map(|preview| preview.clean.as_str()),
            "clean_reasons": image.preview.as_ref().map(|preview| preview.clean_reasons.clone()),
            "rebuilt_packages": image.preview.as_ref().map(|preview| preview.rebuilt_packages.clone()),
            "uninstalled_packages": image.preview.as_ref().map(|preview| preview.uninstalled_packages.clone()),
            "sections": image.preview.as_ref().map(|preview| preview.sections.iter().map(|section| serde_json::json!({
                "title": section.title,
                "lines": section.lines,
            })).collect::<Vec<_>>()),
            "deletions_outside_trash": image.preview.as_ref().map(ImagePreview::deletions_outside_trash),
            "deletions": image.preview.as_ref().map(|preview| preview.deletions.iter().map(|deletion| serde_json::json!({
                "kind": deletion.kind.as_str(),
                "path": deletion.path,
                "reason": deletion.reason,
            })).collect::<Vec<_>>()),
            "trips_fail_on_clean": image.preview.as_ref().map(ImagePreview::trips_fail_on_clean),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_image_providers::PreviewCleanKind;
    use gaia_spec::{BuildrootImageSpec, ImageDefinition, ImageOutputSpec, ImageSpec, SourceId};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Every path under `dir` with its kind and, for files, its SHA-256.
    fn tree_digest(dir: &Path) -> BTreeMap<String, String> {
        let mut digests = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in fs::read_dir(&current).expect("read dir").flatten() {
                let path = entry.path();
                let key = path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let metadata = fs::symlink_metadata(&path).expect("metadata");
                let digest = if metadata.is_dir() {
                    stack.push(path.clone());
                    "dir".to_string()
                } else if metadata.file_type().is_symlink() {
                    format!("link {}", fs::read_link(&path).expect("link").display())
                } else {
                    let hash = Sha256::digest(fs::read(&path).expect("file contents"));
                    format!(
                        "file {}",
                        hash.iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                    )
                };
                digests.insert(key, digest);
            }
        }
        digests
    }

    fn scratch_root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "gaia-app-preview-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn preview_leaves_the_real_output_tree_byte_for_byte_unchanged() {
        let root = scratch_root("unchanged");
        let source = root.join("build/sources/buildroot");
        let output = root.join("build/image/buildroot-output");
        fs::create_dir_all(&source).expect("source");
        fs::write(
            source.join("graph.json"),
            "{\"foo\": {\"type\": \"target\", \"name\": \"foo\", \"virtual\": false, \
             \"version\": \"1\", \"stamp_dir\": \"build/foo-1\", \"dependencies\": [], \
             \"reverse_dependencies\": []}}\n",
        )
        .expect("graph");
        fs::write(
            source.join("Makefile"),
            ".DEFAULT_GOAL := all\n%_defconfig:\n\t@mkdir -p $(O)\n\t@printf 'BR2_PACKAGE_FOO=n\\n' > $(O)/.config\n\
             olddefconfig:\n\t@true\nclean:\n\t@true\nshow-info:\n\t@cat graph.json\nall:\n\t@true\n",
        )
        .expect("makefile");
        // A built tree: its config snapshot says foo is off, the image turns
        // it on, so a run would rebuild foo.
        fs::create_dir_all(output.join("target/usr/bin")).expect("target");
        fs::create_dir_all(output.join("build/foo-1")).expect("build");
        fs::write(output.join("target/usr/bin/foo"), "foo").expect("installed");
        fs::write(output.join("build/foo-1/.stamp_installed"), "").expect("stamp");
        fs::write(
            output.join("build/foo-1/.files-list.txt"),
            "foo,./usr/bin/foo\n",
        )
        .expect("file list");
        fs::write(output.join(".config"), "BR2_PACKAGE_FOO=n\n").expect("config");
        fs::write(
            output.join(".gaia-buildroot-config.last"),
            "BR2_PACKAGE_FOO=n\n",
        )
        .expect("snapshot");
        fs::write(
            output.join(".gaia-buildroot-packages.json"),
            fs::read_to_string(source.join("graph.json")).expect("graph"),
        )
        .expect("graph state");

        let mut spec = gaia_spec::ResolvedBuildSpec::new("app-preview");
        spec.workspace.root_dir = root.display().to_string();
        spec.workspace.build_dir = "build".to_string();
        spec.sources.push(gaia_spec::SourceSpec::new(
            "buildroot",
            gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
                path: source.display().to_string(),
                identity_ignore: Vec::new(),
                refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
                pin_policy: gaia_spec::SourcePinPolicySpec::Floating,
            }),
        ));
        spec.image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                source: Some(SourceId::new("buildroot")),
                defconfig: Some("test_defconfig".into()),
                config_overrides: vec![("BR2_PACKAGE_FOO".into(), "y".into())],
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec::default(),
            assembly: None,
        };

        let context = AppContext::with_defaults();
        let before = tree_digest(&root.join("build"));
        let report = preview_resolved(&context, &spec, &[], true, false)
            .expect("the preview of a valid build succeeds");
        assert_eq!(
            tree_digest(&root.join("build")),
            before,
            "the preview changed the build directory"
        );

        assert!(
            report.operations.iter().any(|op| op.executes),
            "the image operations execute without a reuse state: {report:?}"
        );
        let preview = report
            .images
            .iter()
            .find_map(|image| image.preview.as_ref())
            .expect("the Buildroot image is previewed");
        assert_eq!(preview.rebuilt_packages, ["foo"], "{preview:?}");
        assert!(report.tripped(), "a rebuild deletes installed files");
        assert_eq!(
            report.verdict(),
            format!(
                "no clean, 1 packages rebuilt (foo), {} deleted paths",
                preview.deletions_outside_trash()
            )
        );
        assert!(!report.operations.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_report_without_images_says_so_and_never_trips() {
        let report = PreviewReport {
            build_name: "none".into(),
            operations: vec![PreviewOperation {
                id: "resolve".into(),
                executes: true,
                reason: "fresh".into(),
            }],
            images: Vec::new(),
            fail_on_clean: true,
            json: false,
        };
        assert!(!report.tripped());
        assert_eq!(
            report.verdict(),
            "no image operation would run, 0 deleted paths"
        );
    }

    #[test]
    fn fail_on_clean_exits_3_only_when_a_run_would_clean_or_delete() {
        let report = |fail_on_clean: bool, clean: PreviewCleanKind| PreviewReport {
            build_name: "exit".into(),
            operations: Vec::new(),
            images: vec![PreviewImage {
                operations: vec!["image.build".into()],
                provider_id: "image.buildroot".into(),
                preview: Some(ImagePreview {
                    provider_id: "image.buildroot".into(),
                    sections: Vec::new(),
                    clean,
                    clean_reasons: Vec::new(),
                    rebuilt_packages: Vec::new(),
                    uninstalled_packages: Vec::new(),
                    deletions: Vec::new(),
                    blocked: None,
                    verdict: "no clean, 0 packages rebuilt (none), 0 deleted paths".into(),
                }),
                note: None,
            }],
            fail_on_clean,
            json: false,
        };
        let exit = |report: PreviewReport| CommandOutcome::Previewed { report }.exit_code();
        assert_eq!(exit(report(true, PreviewCleanKind::Full)), 3);
        assert_eq!(exit(report(false, PreviewCleanKind::Full)), 0);
        assert_eq!(exit(report(true, PreviewCleanKind::Nothing)), 0);
    }
}
