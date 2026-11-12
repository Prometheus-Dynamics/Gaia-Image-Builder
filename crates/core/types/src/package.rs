//! The control data a package carries: the component description's
//! `[package]` fields, checked, and the control file text opkg and dpkg
//! read.

use daedalus::prelude::Type;
use serde::{Deserialize, Serialize};

/// What a package says about itself, from the component description.
#[derive(Debug, Clone, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:package-control")]
pub struct Control {
    /// The package name: lowercase letters, digits, `+`, `-` and `.`.
    pub package: String,
    pub version: String,
    pub architecture: String,
    pub maintainer: String,
    /// The synopsis on the first line; further lines are the long text.
    pub description: String,
    /// Other packages this one needs, each one name or `name (constraint)`.
    pub depends: Vec<String>,
}

impl Control {
    /// Why these fields cannot make a package, or `Ok` when they can.
    pub fn check(&self) -> Result<(), String> {
        let name = &self.package;
        let legal = name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'));
        if name.is_empty() || !legal || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            return Err(format!("package name {name:?} is not a legal package name"));
        }
        for (field, value) in [
            ("version", &self.version),
            ("architecture", &self.architecture),
            ("maintainer", &self.maintainer),
        ] {
            if value.trim().is_empty() || value.contains('\n') {
                return Err(format!("{field} must be one non-empty line"));
            }
        }
        if self.description.trim().is_empty() {
            return Err("description must not be empty".into());
        }
        if let Some(dep) = self
            .depends
            .iter()
            .find(|d| d.trim().is_empty() || d.contains('\n'))
        {
            return Err(format!("dependency {dep:?} must be one non-empty line"));
        }
        Ok(())
    }

    /// The control file: fields in the order opkg and dpkg write them,
    /// `Installed-Size` (KiB, rounded up) after the maintainer, the
    /// description last.
    pub fn text(&self, installed_bytes: u64) -> String {
        let mut out = String::new();
        field(&mut out, "Package", &self.package);
        field(&mut out, "Version", &self.version);
        field(&mut out, "Architecture", &self.architecture);
        field(&mut out, "Maintainer", &self.maintainer);
        field(
            &mut out,
            "Installed-Size",
            &installed_bytes.div_ceil(1024).to_string(),
        );
        if !self.depends.is_empty() {
            field(&mut out, "Depends", &self.depends.join(", "));
        }
        let mut lines = self.description.trim_end().split('\n');
        field(
            &mut out,
            "Description",
            lines.next().unwrap_or_default().trim_end(),
        );
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
        out
    }
}

fn field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}
