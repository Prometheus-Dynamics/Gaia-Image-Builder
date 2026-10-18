//! Config errors and unknown keys name the file, line and column they are at.

pub mod support;

use gaia_config::{ConfigError, ResolveOptions, try_resolve_config_with_options};
use support::{TempConfig, write_temp_config};

fn resolve(file: &TempConfig) -> Result<gaia_spec::ResolvedBuildSpec, ConfigError> {
    try_resolve_config_with_options(
        file.to_str().expect("utf-8 temp path"),
        &ResolveOptions::default(),
    )
}

fn parse_error(contents: &str) -> String {
    let file = write_temp_config(contents);
    match resolve(&file) {
        Ok(_) => panic!("the config should not resolve"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_type_error_names_the_line_and_column_of_the_value() {
    let file = write_temp_config(
        "build_name = \"demo\"\n\n[workspace]\nroot_dir = 5\nbuild_dir = \"build\"\n",
    );
    let message = match resolve(&file) {
        Ok(_) => panic!("a number is not a root_dir"),
        Err(error) => error.to_string(),
    };
    let expected = format!("{}:4:12", file.display());
    assert!(message.contains(&expected), "{message}");
}

#[test]
fn a_syntax_error_names_its_line_and_column() {
    let message = parse_error("build_name = \"demo\"\n[workspace\nroot_dir = \".\"\n");
    assert!(message.contains("config.toml:2:"), "{message}");
}

#[test]
fn an_unknown_key_names_its_line_and_column() {
    let file = write_temp_config(
        "build_name = \"demo\"\n\n[[sources]]\nid = \"app\"\nkind = \"path\"\n\
         path = \".\"\nrepo = \"https://example.invalid/app.git\"\n",
    );
    let spec = resolve(&file).expect("an unknown key is a warning, not an error");
    let expected = format!("unknown key 'sources[0].repo' in '{}:7:1'", file.display());
    assert!(
        spec.metadata
            .config_warnings
            .iter()
            .any(|warning| warning.starts_with(&expected)),
        "{:?}",
        spec.metadata.config_warnings
    );
}

#[test]
fn an_unknown_key_in_a_nested_table_names_its_line() {
    let file = write_temp_config(
        "build_name = \"demo\"\n\n[providers.java]\ngradle_home = \"user-cache\"\n\
         gradle_hme = \"user-cache\"\n",
    );
    let spec = resolve(&file).expect("resolves");
    let expected = format!(
        "unknown key 'providers.java.gradle_hme' in '{}:5:1' (did you mean 'gradle_home'?)",
        file.display()
    );
    assert!(
        spec.metadata
            .config_warnings
            .iter()
            .any(|warning| warning.starts_with(&expected)),
        "{:?}",
        spec.metadata.config_warnings
    );
}
