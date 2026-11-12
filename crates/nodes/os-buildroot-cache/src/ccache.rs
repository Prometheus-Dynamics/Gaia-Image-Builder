//! `gaia.os.buildroot.ccache`: the config fragment that turns on Buildroot's
//! ccache and points it at one directory.
//!
//! ccache keys each compilation on its preprocessed input and its compiler,
//! so two products that compile the same source with the same compiler share
//! its objects, and a product that compiles something else misses. The
//! directory is the caller's (a host path); nothing about it is written into
//! the output.

use daedalus::prelude::*;
use gaia_os_buildroot::ConfigFragment;
use gaia_step::StepError;
use std::path::Path;

const NAME: &str = "gaia.os.buildroot.ccache";

/// The fragment for `dir`, an absolute path. It goes into a Buildroot config
/// string, so a quote, a backslash or a line break in it is refused.
pub fn fragment(dir: &str) -> Result<ConfigFragment, String> {
    if !Path::new(dir).is_absolute() {
        return Err(format!("ccache directory {dir:?} is not an absolute path"));
    }
    if dir.contains(['"', '\\', '\n', '\r']) {
        return Err(format!(
            "ccache directory {dir:?} has a quote, a backslash or a line break"
        ));
    }
    Ok(ConfigFragment {
        name: "ccache".into(),
        text: format!("BR2_CCACHE=y\nBR2_CCACHE_DIR=\"{dir}\"\n"),
    })
}

/// The ccache fragment for `dir`, as a Buildroot node output.
#[node(id = "gaia.os.buildroot.ccache", inputs(dir), outputs(fragment))]
pub fn os_buildroot_ccache(dir: &str) -> Result<ConfigFragment, StepError> {
    fragment(dir).map_err(|why| StepError::body(NAME, why))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fragment_turns_ccache_on_at_the_directory() {
        let fragment = fragment("/mnt/cache/ccache").expect("fragment");
        assert_eq!(fragment.name, "ccache");
        assert_eq!(
            fragment.text,
            "BR2_CCACHE=y\nBR2_CCACHE_DIR=\"/mnt/cache/ccache\"\n"
        );
    }

    #[test]
    fn a_relative_directory_is_refused() {
        let why = fragment("ccache").expect_err("relative");
        assert!(why.contains("not an absolute path"), "{why}");
    }

    #[test]
    fn a_directory_that_would_break_the_config_string_is_refused() {
        for dir in ["/a\"b", "/a\\b", "/a\nb"] {
            assert!(fragment(dir).is_err(), "{dir:?}");
        }
    }
}
