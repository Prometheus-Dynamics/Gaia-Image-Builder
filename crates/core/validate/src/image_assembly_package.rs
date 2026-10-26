//! `[[image.assembly.packages]]` entries: ids, the format, the tree and
//! destination, the allowed prefixes, the control fields and their names,
//! and the scripts. What the built package holds (the units, the conffiles,
//! the prefixes of the files) is checked when it is written.

use std::collections::HashSet;

use gaia_spec::{AssemblyPackageSpec, AssemblyTreeId, ImageAssemblySpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
use crate::image_assembly::validate_assembly_path_template;

/// The control fields the writer sets itself. An `extra` field may not name
/// one of them (compared without case).
const RESERVED_CONTROL_FIELDS: [&str; 11] = [
    "Package",
    "Version",
    "Architecture",
    "Maintainer",
    "Installed-Size",
    "Depends",
    "Section",
    "Priority",
    "Source",
    "Homepage",
    "Description",
];

pub(crate) fn validate_packages(
    spec: &ResolvedBuildSpec,
    assembly: &ImageAssemblySpec,
    tree_ids: &HashSet<AssemblyTreeId>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.packages".to_string());
    let mut ids = HashSet::new();
    for package in &assembly.packages {
        let label = format!("assembly package '{}'", package.id);
        if package.id.trim().is_empty() {
            diagnostics.push(error(
                "assembly_package_id_empty",
                "assembly package id cannot be empty".into(),
                location(),
            ));
        } else if !ids.insert(package.id.as_str()) {
            diagnostics.push(error(
                "assembly_package_duplicate",
                format!("{label} is declared more than once"),
                location(),
            ));
        }

        match package.format.as_str() {
            "ipk" => {}
            "deb" => diagnostics.push(error(
                "assembly_package_format_unsupported",
                format!("{label} format 'deb' is not supported yet; use ipk"),
                location(),
            )),
            other => diagnostics.push(error(
                "assembly_package_format_invalid",
                format!("{label} format '{other}' is not known; use ipk"),
                location(),
            )),
        }

        if !tree_ids.contains(&package.tree) {
            diagnostics.push(error(
                "assembly_package_tree_unknown",
                format!(
                    "{label} names tree '{}', which is not defined",
                    package.tree
                ),
                Some("image.assembly.packages.tree".into()),
            ));
        }

        if package.dest.as_str().trim().is_empty() {
            diagnostics.push(error(
                "assembly_package_dest_empty",
                format!("{label} dest cannot be empty"),
                location(),
            ));
        } else {
            validate_assembly_path_template(
                spec,
                tree_ids,
                &package.dest,
                "image.assembly.packages.dest",
                diagnostics,
            );
        }

        for prefix in &package.allowed_prefixes {
            if !prefix.starts_with('/') || prefix.split('/').any(|part| part == "..") {
                diagnostics.push(error(
                    "assembly_package_prefix_invalid",
                    format!(
                        "{label} allowed_prefixes entry '{prefix}' must be an absolute path without '..'"
                    ),
                    Some("image.assembly.packages.allowed_prefixes".into()),
                ));
            }
        }

        validate_control(package, &label, diagnostics);
        validate_scripts(package, &label, diagnostics);
    }
}

fn validate_control(
    package: &AssemblyPackageSpec,
    label: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.packages.control".to_string());
    let control = &package.control;

    if control.package.is_empty() {
        diagnostics.push(error(
            "assembly_package_control_missing",
            format!("{label} control.Package is required"),
            location(),
        ));
    } else if !valid_package_name(&control.package) {
        diagnostics.push(error(
            "assembly_package_name_invalid",
            format!(
                "{label} control.Package '{}' must be two or more of a-z, 0-9, + . - and start with a letter or digit",
                control.package
            ),
            location(),
        ));
    }

    if control.version.is_empty() {
        diagnostics.push(error(
            "assembly_package_control_missing",
            format!("{label} control.Version is required"),
            location(),
        ));
    } else if !control
        .version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "+-.:~_".contains(c))
    {
        diagnostics.push(error(
            "assembly_package_control_invalid",
            format!(
                "{label} control.Version '{}' may hold only letters, digits and + - . : ~ _",
                control.version
            ),
            location(),
        ));
    }

    if control.architecture.is_empty() {
        diagnostics.push(error(
            "assembly_package_architecture_missing",
            format!(
                "{label} control.Architecture is required (there is no default; name the target, such as aarch64)"
            ),
            location(),
        ));
    } else if !control
        .architecture
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
    {
        diagnostics.push(error(
            "assembly_package_control_invalid",
            format!(
                "{label} control.Architecture '{}' may hold only letters, digits and _ - .",
                control.architecture
            ),
            location(),
        ));
    }

    if control.maintainer.is_empty() {
        diagnostics.push(error(
            "assembly_package_control_missing",
            format!("{label} control.Maintainer is required"),
            location(),
        ));
    } else if control.maintainer.contains('\n') {
        diagnostics.push(error(
            "assembly_package_control_invalid",
            format!("{label} control.Maintainer must be a single line"),
            location(),
        ));
    }

    if control.description.trim().is_empty() {
        diagnostics.push(error(
            "assembly_package_control_missing",
            format!("{label} control.Description is required"),
            location(),
        ));
    } else if control
        .description
        .split('\n')
        .next()
        .is_some_and(|synopsis| synopsis.trim().is_empty())
    {
        diagnostics.push(error(
            "assembly_package_control_invalid",
            format!("{label} control.Description must start with a one-line synopsis"),
            location(),
        ));
    }

    for dependency in &control.depends {
        if dependency.trim().is_empty() || dependency.contains('\n') {
            diagnostics.push(error(
                "assembly_package_control_invalid",
                format!("{label} control.Depends entries must be single, non-empty lines"),
                location(),
            ));
        }
    }

    let optional = [
        ("Section", &control.section),
        ("Priority", &control.priority),
        ("Source", &control.source),
        ("Homepage", &control.homepage),
    ];
    for (name, value) in optional {
        if let Some(value) = value
            && (value.trim().is_empty() || value.contains('\n'))
        {
            diagnostics.push(error(
                "assembly_package_control_invalid",
                format!("{label} control.{name} must be a single, non-empty line"),
                location(),
            ));
        }
    }

    let mut names = HashSet::new();
    for (name, value) in &control.extra {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            diagnostics.push(error(
                "assembly_package_field_name_invalid",
                format!("{label} control.extra field '{name}' must be letters, digits and -"),
                Some("image.assembly.packages.control.extra".into()),
            ));
        } else if RESERVED_CONTROL_FIELDS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            diagnostics.push(error(
                "assembly_package_field_reserved",
                format!("{label} control.extra field '{name}' is a field the package sets itself"),
                Some("image.assembly.packages.control.extra".into()),
            ));
        } else if !names.insert(name.to_ascii_lowercase()) {
            diagnostics.push(error(
                "assembly_package_field_duplicate",
                format!(
                    "{label} control.extra names '{name}' more than once (field names ignore case)"
                ),
                Some("image.assembly.packages.control.extra".into()),
            ));
        }
        if value.contains('\n') || value.contains('\r') {
            diagnostics.push(error(
                "assembly_package_control_invalid",
                format!("{label} control.extra field '{name}' must be a single line"),
                Some("image.assembly.packages.control.extra".into()),
            ));
        }
    }
}

fn validate_scripts(
    package: &AssemblyPackageSpec,
    label: &str,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = || Some("image.assembly.packages.scripts".to_string());
    let scripts = &package.scripts;
    let mut services = HashSet::new();
    for service in &scripts.services {
        if service.trim().is_empty() || service.contains(['/', ' ', '\t']) {
            diagnostics.push(error(
                "assembly_package_service_invalid",
                format!("{label} scripts.services entry '{service}' must be a unit file name"),
                location(),
            ));
        } else if !services.insert(service.as_str()) {
            diagnostics.push(error(
                "assembly_package_service_duplicate",
                format!("{label} lists service '{service}' more than once"),
                location(),
            ));
        }
    }
    let overrides = [
        ("postinst", &scripts.postinst),
        ("preinst", &scripts.preinst),
        ("prerm", &scripts.prerm),
        ("postrm", &scripts.postrm),
    ];
    for (name, path) in overrides {
        if let Some(path) = path
            && path.as_str().trim().is_empty()
        {
            diagnostics.push(error(
                "assembly_package_script_invalid",
                format!("{label} scripts.{name} names an empty path"),
                location(),
            ));
        }
    }
    for conffile in &scripts.conffiles {
        if !conffile.starts_with('/') || conffile.ends_with('/') {
            diagnostics.push(error(
                "assembly_package_conffile_invalid",
                format!(
                    "{label} scripts.conffiles entry '{conffile}' must be an absolute file path"
                ),
                location(),
            ));
        }
    }
}

/// `[a-z0-9][a-z0-9+.-]+`: the Debian package name rule.
fn valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit());
    let mut rest = chars.peekable();
    first_ok
        && rest.peek().is_some()
        && rest.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
}
