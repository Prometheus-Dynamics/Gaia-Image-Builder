//! `[image] kind = "none"`: a build with no rootfs, kernel or image. Its
//! sources and assembly make one ipk package, which is the primary output.

pub mod support;

use gaia_app::{AppArgs, AppCommand, CommandOutcome, run_with_args};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use support::write_temp_build;

/// A build of one ipk package from a `files/` source, with `kind = "none"`.
fn kind_none_build(root: &Path) -> String {
    format!(
        r#"
build_name = "kind-none-package"
version = "1.0.0"

[workspace]
root_dir = "{root}"
build_dir = "{root}/build"
out_dir = "{root}/out"

[[sources]]
id = "device-files"
kind = "path"
path = "files"
refresh = "never"
pin = "locked"

[image]
kind = "none"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"
out_dir = "${{workspace.out_dir}}"

[[image.assembly.trees]]
id = "package-root"
path = "$assembly.work/package-root"

[[image.assembly.files]]
tree = "package-root"
src = "${{source.device-files.path}}/demo.service"
dest = "etc/systemd/system/demo.service"
mode = "0644"

[[image.assembly.files]]
tree = "package-root"
src = "${{source.device-files.path}}/demo"
dest = "usr/bin/demo"
mode = "0755"

[[image.assembly.packages]]
id = "demo"
tree = "package-root"
dest = "$assembly.out/"
allowed_prefixes = ["/usr/bin", "/etc/systemd/system"]

[image.assembly.packages.control]
Package = "kind-none-demo"
Version = "${{build.version}}"
Architecture = "all"
Maintainer = "Test <test@example.invalid>"
Description = "A kind none package."

[image.assembly.packages.scripts]
services = ["demo.service"]
"#,
        root = root.display()
    )
}

fn write_sources(root: &Path) {
    let files = root.join("files");
    fs::create_dir_all(&files).expect("files dir");
    fs::write(
        files.join("demo.service"),
        "[Unit]\nDescription=Demo\n[Service]\nExecStart=/usr/bin/demo\n",
    )
    .expect("service file");
    fs::write(files.join("demo"), "#!/bin/sh\necho demo\n").expect("script file");
}

#[test]
fn kind_none_plans_no_image_operation_and_assembles_the_package() {
    let scratch = TempDir::new("gaia-kind-none-plan");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    write_sources(&root);
    let build = write_temp_build(&scratch, &kind_none_build(&root));

    let plan = run_with_args(AppArgs {
        command: AppCommand::Plan,
        build,
        ..AppArgs::default()
    });
    match plan {
        CommandOutcome::Planned {
            spec,
            plan,
            diagnostics,
            ..
        } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            assert_eq!(
                spec.image.provider_kind(),
                gaia_spec::ImageProviderKind::None
            );
            let ids: Vec<&str> = plan
                .operations
                .iter()
                .map(|operation| operation.id.as_str())
                .collect();
            for absent in ["image:prepare", "image:build", "image:check"] {
                assert!(!ids.contains(&absent), "{absent} in {ids:?}");
            }
            assert!(ids.contains(&"image:assembly"), "{ids:?}");
            assert!(ids.contains(&"report:emit"), "{ids:?}");
        }
        other => panic!("expected planned outcome, got {other:?}"),
    }
}

#[test]
fn kind_none_build_runs_end_to_end_and_its_ipk_is_the_primary_output() {
    let scratch = TempDir::new("gaia-kind-none-run");
    let root = scratch.join("root");
    fs::create_dir_all(&root).expect("root");
    write_sources(&root);
    let build = write_temp_build(&scratch, &kind_none_build(&root));

    let run = run_with_args(AppArgs {
        command: AppCommand::Run,
        build,
        ..AppArgs::default()
    });
    match run {
        CommandOutcome::Ran {
            report,
            execution_errors,
            validation,
            ..
        } => {
            assert!(validation.errors.is_empty(), "{:?}", validation.errors);
            assert!(execution_errors.is_empty(), "{execution_errors:?}");
            let primary = report
                .summary
                .primary_image_output
                .clone()
                .expect("the package is the primary output");
            let primary = PathBuf::from(primary);
            assert!(
                primary.extension().is_some_and(|ext| ext == "ipk"),
                "{}",
                primary.display()
            );
            assert!(primary.is_file(), "{}", primary.display());
            assert!(
                primary.starts_with(root.join("out")),
                "{}",
                primary.display()
            );
            assert!(
                report
                    .summary
                    .published_outputs
                    .first()
                    .is_some_and(|first| Path::new(first) == primary),
                "{:?}",
                report.summary.published_outputs
            );
        }
        other => panic!("expected run outcome, got {other:?}"),
    }
}
