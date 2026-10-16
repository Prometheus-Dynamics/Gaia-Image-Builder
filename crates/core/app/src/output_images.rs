//! The image lines of a `gaia run` report: the primary image output with its
//! size and digest, then each other image the run published.

use std::fs;
use std::path::Path;

use gaia_report::RunSummary;

use crate::export::sha256_file;

pub(crate) fn print_image_outputs(summary: &RunSummary) {
    if let Some(primary_image_output) = &summary.primary_image_output {
        println!("primary image output: {primary_image_output}");
        if let Ok(bytes) = fs::metadata(primary_image_output).map(|metadata| metadata.len()) {
            println!("primary image size: {} bytes", bytes);
        }
        if let Ok(sha256) = sha256_file(Path::new(primary_image_output)) {
            println!("primary image sha256: {sha256}");
        }
    }
    for output in summary
        .published_outputs
        .iter()
        .filter(|path| Some(*path) != summary.primary_image_output.as_ref())
    {
        match fs::metadata(output) {
            Ok(metadata) if metadata.is_file() => {
                println!("image output: {output} ({} bytes)", metadata.len())
            }
            _ => println!("image output: {output}"),
        }
    }
}
