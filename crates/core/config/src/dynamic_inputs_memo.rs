//! In-process memo of successful dynamic-choice fetches (see `memoized_fetch`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::fetch_choices;
use super::{RawInputChoicesFromConfig, RawInputChoicesFromKind, RawInputChoicesRefresh};

/// How long a successful fetch is reused within the process. Only the in-memory
/// memo expires; the on-disk cache keeps its own TTL semantics. A value
/// fetched for one resolution is reused by the next (variants, validate, plan)
/// for this long, so a choice added within the window appears after it.
const FETCH_MEMO_TTL: Duration = Duration::from_secs(60);

type FetchMemo = Mutex<HashMap<String, (Instant, Vec<String>)>>;

/// The choices of an input's source. An `auto` source (not a command) is
/// memoized in process; `always` and `never` always go to the source.
pub(crate) fn fetch_choices_memoized(
    config: &RawInputChoicesFromConfig,
    repo: &str,
    source_dir: &Path,
) -> Result<Vec<String>, String> {
    if config.refresh != RawInputChoicesRefresh::Auto
        || config.kind == RawInputChoicesFromKind::Command
    {
        return fetch_choices(config, repo);
    }
    let key = format!("{}\u{1f}{repo}\u{1f}{config:?}", source_dir.display());
    memoized_fetch(key, || fetch_choices(config, repo))
}

/// `fetch`, memoized under `key` for [`FETCH_MEMO_TTL`]. Only successes are
/// kept, so a failed fetch is retried by the next resolution.
pub(crate) fn memoized_fetch(
    key: String,
    fetch: impl FnOnce() -> Result<Vec<String>, String>,
) -> Result<Vec<String>, String> {
    static MEMO: OnceLock<FetchMemo> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let hit = memo.lock().ok().and_then(|memo| {
        memo.get(&key)
            .filter(|(made, _)| made.elapsed() < FETCH_MEMO_TTL)
            .map(|(_, choices)| choices.clone())
    });
    if let Some(choices) = hit {
        return Ok(choices);
    }
    let choices = fetch()?;
    if let Ok(mut memo) = memo.lock() {
        memo.insert(key, (Instant::now(), choices.clone()));
    }
    Ok(choices)
}
