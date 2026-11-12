//! Size budgets: an image's partitions and the components it holds, against
//! the sizes the product allows. An image over budget fails its step, and the
//! failure names every item over its budget and the largest contributors.
//!
//! - **`gaia.size.budget`**: checks the image's total size, each partition the
//!   budget names, and each component the budget names. A component's size is
//!   the bytes of the regular files it owns in the merged tree (`owners.json`
//!   from `gaia.tree.merge`; a file with several owners counts for the first).
//!   Gives the image unchanged.
//!
//! The budget is JSON text from the product's description. Sizes are bytes or
//! text such as `"64M"`, as the descriptions write them. Budgets are inclusive:
//! a size equal to its budget is within it.

mod budget;

pub use budget::{Budget, size_budget};

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 1] = [&budget::SIZE_BUDGET_NODE];
