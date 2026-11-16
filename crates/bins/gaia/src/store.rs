//! `gaia store verify`: the content store checked in full. Each object is
//! read and compared with its digest (see `gaia_store::Store::verify`).

use crate::{Stop, args, project};
use gaia_store::Store;

pub fn verify(options: &args::Verify) -> Result<(), Stop> {
    let dir = project::store_dir(options.store.as_deref())?;
    if !dir.is_dir() {
        println!("no store at {}", dir.display());
        return Ok(());
    }
    let store = Store::open(&dir).map_err(Stop::usage)?;
    let report = store.verify(options.fix).map_err(Stop::usage)?;
    for problem in &report.problems {
        println!("bad  {}: {}", problem.path.display(), problem.reason);
    }
    println!(
        "checked {} objects: {} verified, {} do not match{}",
        report.checked,
        report.verified,
        report.problems.len(),
        if options.fix {
            format!(", {} removed", report.removed)
        } else {
            String::new()
        }
    );
    if report.problems.is_empty() {
        return Ok(());
    }
    Err(Stop::usage(format!(
        "{} objects in {} do not match their digests",
        report.problems.len(),
        dir.display()
    )))
}
