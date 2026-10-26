//! The control file of a package, the maintainer scripts generated for its
//! systemd services, and the parse of a control file back into fields (the
//! check the written package passes).

use gaia_spec::AssemblyPackageControlSpec;

/// The control file text: the fields in the order opkg writes them, with
/// `Installed-Size` (KiB) after the maintainer and the description last.
pub(super) fn control_text(control: &AssemblyPackageControlSpec, installed_kib: u64) -> String {
    let mut out = String::new();
    push_field(&mut out, "Package", &control.package);
    push_field(&mut out, "Version", &control.version);
    push_field(&mut out, "Architecture", &control.architecture);
    push_field(&mut out, "Maintainer", &control.maintainer);
    push_field(&mut out, "Installed-Size", &installed_kib.to_string());
    if !control.depends.is_empty() {
        push_field(&mut out, "Depends", &control.depends.join(", "));
    }
    let optional = [
        ("Section", &control.section),
        ("Priority", &control.priority),
        ("Source", &control.source),
        ("Homepage", &control.homepage),
    ];
    for (name, value) in optional {
        if let Some(value) = value {
            push_field(&mut out, name, value);
        }
    }
    for (name, value) in &control.extra {
        push_field(&mut out, name, value);
    }
    push_description(&mut out, &control.description);
    out
}

fn push_field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

/// The synopsis on the `Description` line, then each further line indented
/// by one space; a blank line becomes ` .`, the Debian paragraph break.
fn push_description(out: &mut String, description: &str) {
    let description = description.trim_end();
    let mut lines = description.split('\n');
    let synopsis = lines.next().unwrap_or("").trim_end();
    push_field(out, "Description", synopsis);
    for line in lines {
        let line = line.trim_end();
        if line.trim().is_empty() {
            out.push_str(" .\n");
        } else {
            out.push(' ');
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// The maintainer scripts the services generate, POSIX sh, each ending in
/// `exit 0`. `None` when there are no services, so no script is written.
pub(super) fn generated_script(name: &str, services: &[String]) -> Option<String> {
    if services.is_empty() {
        return None;
    }
    let mut out = String::from("#!/bin/sh\n");
    match name {
        "postinst" => {
            out.push_str("systemctl daemon-reload\n");
            for unit in services {
                let unit = shell_quote(unit);
                out.push_str(&format!("systemctl enable {unit}\n"));
                out.push_str(&format!(
                    "systemctl start {unit} || echo \"Failed to start {unit}\" >&2\n"
                ));
            }
        }
        "prerm" => {
            for unit in services {
                let unit = shell_quote(unit);
                out.push_str(&format!("systemctl stop {unit} >/dev/null 2>&1 || true\n"));
                out.push_str(&format!(
                    "systemctl disable {unit} >/dev/null 2>&1 || true\n"
                ));
            }
        }
        "postrm" => {
            out.push_str("systemctl daemon-reload >/dev/null 2>&1 || true\n");
            for unit in services {
                let unit = shell_quote(unit);
                out.push_str(&format!(
                    "systemctl reset-failed {unit} >/dev/null 2>&1 || true\n"
                ));
            }
        }
        _ => return None,
    }
    out.push_str("exit 0\n");
    Some(out)
}

/// `text` as one POSIX shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The fields of a control file, in order, with continuation lines joined
/// by newlines (a leading space removed). Errors on a line that is not a
/// field, a blank line, or a field named twice.
pub(super) fn parse_control(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let number = number + 1;
        if line.is_empty() {
            return Err(format!("control line {number} is blank"));
        }
        if let Some(continued) = line.strip_prefix(' ') {
            let Some((_, value)) = fields.last_mut() else {
                return Err(format!("control line {number} continues no field"));
            };
            value.push('\n');
            value.push_str(continued);
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!(
                "control line {number} is not a 'Field: value' line"
            ));
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!(
                "control line {number} has the bad field name '{name}'"
            ));
        }
        if fields.iter().any(|(existing, _)| existing == name) {
            return Err(format!("control field '{name}' is set more than once"));
        }
        fields.push((name.to_string(), value.trim_start_matches(' ').to_string()));
    }
    Ok(fields)
}

/// The control fields the package must carry, and a legal `Installed-Size`.
pub(super) fn check_control_fields(fields: &[(String, String)]) -> Result<(), String> {
    for required in [
        "Package",
        "Version",
        "Architecture",
        "Maintainer",
        "Installed-Size",
        "Description",
    ] {
        if !fields.iter().any(|(name, _)| name == required) {
            return Err(format!("control has no {required} field"));
        }
    }
    let size = fields
        .iter()
        .find(|(name, _)| name == "Installed-Size")
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    if size.parse::<u64>().is_err() {
        return Err(format!("control Installed-Size '{size}' is not a number"));
    }
    Ok(())
}

/// The value of `name` among parsed control `fields`.
pub(super) fn control_value<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
}
