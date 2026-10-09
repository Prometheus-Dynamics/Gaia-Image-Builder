//! Caches shared by every workspace of the user (downloads, the compiler
//! cache, the Buildroot package cache), so other projects and rebuilds
//! after a wipe reuse them.

use std::path::PathBuf;

/// `<user cache root>/buildroot/packages`, the default Buildroot package
/// cache.
pub const USER_BUILDROOT_PACKAGE_CACHE_DIR: &str = "buildroot/packages";
/// `<user cache root>/buildroot/ccache`, the default compiler cache.
pub const USER_BUILDROOT_CCACHE_DIR: &str = "buildroot/ccache";

/// `$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`.
pub fn user_cache_root() -> Option<PathBuf> {
    let from_env = |key: &str| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    from_env("GAIA_CACHE_DIR")
        .or_else(|| from_env("XDG_CACHE_HOME").map(|dir| dir.join("gaia")))
        .or_else(|| from_env("HOME").map(|home| home.join(".cache/gaia")))
}
