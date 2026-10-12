//! Printing of a build with variants: each variant's own outcome, in order,
//! under its header, then the primary output of each finished variant.

use crate::commands::{CommandOutcome, VariantOutcome};
use crate::output::print_outcome;

pub(crate) fn print_variants(header: &[String], outcomes: &[VariantOutcome]) {
    for line in header {
        println!("{line}");
    }
    for item in outcomes {
        println!("== variant {} ==", item.id);
        print_outcome(&item.outcome);
    }
    for item in outcomes {
        if let CommandOutcome::Ran { report, .. } = &item.outcome {
            println!(
                "variant {}: primary output {}",
                item.id,
                report
                    .summary
                    .primary_image_output
                    .as_deref()
                    .unwrap_or("none")
            );
        }
    }
}
