//! `gaia clean`: store retention. What the latest N builds used stays;
//! the rest of the store, and older run records, go.

use crate::record::{self, Run};
use crate::{Stop, args, project};
use gaia_store::{Keep, Store};

/// Builds kept when `--keep` isn't given.
const KEEP: usize = 3;

pub fn clean(options: &args::Clean) -> Result<(), Stop> {
    let dir = project::store_dir(options.store.as_deref())?;
    if !dir.is_dir() {
        println!("no store at {}", dir.display());
        return Ok(());
    }
    let store = Store::open(&dir).map_err(Stop::usage)?;
    let runs = record::all(&store);
    let n = options.keep.unwrap_or(KEEP);
    let (old, kept) = runs.split_at(runs.len().saturating_sub(n));
    let mut keep = Keep::default();
    for path in kept {
        keep.extend(&Run::read(path)?.keep());
    }
    let apply = !options.dry_run;
    let report = store.retain(&keep, apply).map_err(Stop::usage)?;
    if apply {
        for path in old {
            std::fs::remove_file(path)
                .map_err(|e| Stop::usage(format!("{}: {e}", path.display())))?;
        }
    }
    let verb = if apply { "removed" } else { "would remove" };
    println!(
        "kept {} builds: {} entries, {} objects",
        kept.len(),
        report.entries_kept,
        report.objects_kept
    );
    println!(
        "{verb} {} entries, {} objects, {} leftovers, {} older build records: {:.1} MiB",
        report.entries_removed,
        report.objects_removed,
        report.leftovers_removed,
        old.len(),
        report.bytes_removed as f64 / (1024.0 * 1024.0)
    );
    Ok(())
}
