//! Gaia's step kit: how a build step is written as a Daedalus node.
//!
//! Every step does the same three things, and this crate does them:
//!
//! 1. **Key.** A step's key is a digest of its node kind, its node version,
//!    its config and its inputs' digests ([`KeyBuilder`]). The same inputs
//!    give the same key on any machine.
//! 2. **Look up or make.** [`Step::look_up_or_make`] asks the content store
//!    for the key. When the entry is there and every output it names passes
//!    the check, those references are returned and the body never runs.
//!    Otherwise the body runs in a fresh output directory ([`Work`]), the
//!    outputs are committed, and only then is the entry written.
//! 3. **Tools.** [`Work::run`] runs one tool process ([`Tool`]) with a fixed
//!    argument list and environment, streams its output into bounded tails,
//!    kills its whole process group when the run is cancelled, and turns a
//!    failed exit into a [`StepError`] that names the step. Each run's whole
//!    output goes to a run log file next to the store ([`logs_dir`]), which
//!    a failed step keeps.
//! 4. **Step record.** Each look-up-or-make that ends with outputs adds a
//!    line to `steps.log` in the same directory: hit or made, the key and
//!    the outputs ([`step_records`]). It is a log: no step reads it.
//!
//! What a step does not do: retry, time out, schedule or keep state.
//! Retries and stopping are Daedalus step policy; the store is the only state.
//!
//! ```ignore
//! #[node(id = "gaia.example", inputs(store, source), outputs(out))]
//! fn example(store: &str, source: &str, cx: &Ctx) -> Result<String, StepError> {
//!     let store = Store::open(store).map_err(StepError::store("gaia.example"))?;
//!     let key = KeyBuilder::new("gaia.example", 1).input("source", digest).finish();
//!     let made = Step::new(&store, cx, "gaia.example", key)
//!         .look_up_or_make(&[("out", Path::new("out"))], |work| {
//!             work.run(&Tool::new("/usr/bin/tool").arg(work.dir().join("out")))?;
//!             Ok(())
//!         })?;
//!     Ok(made.outputs["out"].to_string())
//! }
//! ```

pub mod cargo_lock;
mod error;
mod key;
mod log;
mod step;
mod tool;

pub use daedalus_cpu_author::Ctx;
pub use error::{ErrorKind, StepError, code};
pub use gaia_store::{Check, Digest, ObjectRef, StepKey, Store};
pub use key::KeyBuilder;
pub use log::{StepRecord, logs_dir, now_ns, step_records, step_records_from};
pub use step::{Made, Step, Work};
pub use tool::{Stream, Tool, ToolOutput};
