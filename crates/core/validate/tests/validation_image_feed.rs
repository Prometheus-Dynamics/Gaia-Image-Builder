pub mod support;

use gaia_config::resolve_config;
use gaia_validate::{DiagnosticSeverity, validate_spec};
use std::fs;
use support::write_temp_config;

const BASE: &str = r#"
build_name = "feed-lists"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[stage.files]]
id = "listed"
src = "assets/listed"
dest = "/etc/listed"
origin = "static-asset"

[[stage.files]]
id = "late-layer"
src = "assets/late"
dest = "/etc/late"
origin = "static-asset"

[[stage.files]]
id = "late-layer-too"
src = "assets/late-too"
dest = "/etc/late-too"
origin = "static-asset"
"#;

fn unfed_messages(config: &str) -> Vec<(DiagnosticSeverity, String)> {
    let path = write_temp_config(config);
    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.code == "image_feed_unfed_entry")
        .map(|diagnostic| (diagnostic.severity, diagnostic.message))
        .collect()
}

#[test]
fn an_explicit_feed_list_warns_about_what_it_leaves_out() {
    let messages = unfed_messages(&format!(
        "{BASE}\n[image.feed]\nstage_files = [\"listed\"]\n"
    ));
    assert_eq!(messages.len(), 1, "{messages:?}");
    let (severity, message) = &messages[0];
    assert_eq!(*severity, DiagnosticSeverity::Warning);
    assert!(
        message.starts_with(
            "stage files 'late-layer', 'late-layer-too' are declared but not in image.feed.stage_files"
        ),
        "{message}"
    );
}

#[test]
fn the_default_feed_takes_every_entry() {
    assert!(unfed_messages(BASE).is_empty());
}

#[test]
fn a_list_naming_every_entry_is_quiet() {
    assert!(
        unfed_messages(&format!(
            "{BASE}\n[image.feed]\nstage_files = [\"listed\", \"late-layer\", \"late-layer-too\"]\n"
        ))
        .is_empty()
    );
}
