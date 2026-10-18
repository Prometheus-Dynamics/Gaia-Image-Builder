//! Offline Buildroot runs (`[execution] offline`): a package whose source is
//! not in the download cache makes the make fail, and the error names it.
//!
//! The config settings that keep Buildroot off the network are written by
//! `buildroot_cache_overrides`. This module only explains the failure: make
//! prints `>>> <package> <version> Downloading` before every download, so the
//! last such line before a failure names the package that could not be
//! fetched.
use super::*;
use std::sync::{Arc, Mutex};

/// Taps the make output of one offline run.
pub(crate) struct OfflineDownloadGuard {
    /// The download cache the run reads.
    download_dir: PathBuf,
    /// The last `>>> ` line of the make output.
    last_package_line: Arc<Mutex<Option<String>>>,
}

impl OfflineDownloadGuard {
    pub(crate) fn new(request: &BuildrootRunRequest<'_>) -> Result<Self, ImageProviderError> {
        let policy = request.command.policy;
        let download_dir = match configured_download_dir(request.spec, policy)? {
            Some(dir) => dir,
            None => default_cache_dir(
                request.spec,
                DEFAULT_BUILDROOT_DOWNLOAD_DIR,
                "buildroot download",
            )?,
        };
        Ok(Self {
            download_dir,
            last_package_line: Arc::new(Mutex::new(None)),
        })
    }

    /// The request with its log sink tapped (and every line still forwarded).
    pub(crate) fn request<'a>(&self, request: BuildrootRunRequest<'a>) -> BuildrootRunRequest<'a> {
        let forwarded = request.command.log_sink.clone();
        let last = Arc::clone(&self.last_package_line);
        let sink: ProcessLogSink = Arc::new(move |line: gaia_process::ProcessLogLine| {
            // Lines arrive labelled ("<label> stdout: >>> <package> ..."), so
            // the make line starts wherever its `>>> ` marker is.
            if let Some(index) = line.line.find(">>> ")
                && let Ok(mut slot) = last.lock()
            {
                *slot = Some(line.line[index..].to_string());
            }
            if let Some(forward) = &forwarded {
                forward(line);
            }
        });
        BuildrootRunRequest {
            command: ImageCommandContext {
                log_sink: Some(sink),
                ..request.command
            },
            ..request
        }
    }

    /// The failure, naming the package when the make was downloading it.
    pub(crate) fn explain(&self, error: ImageProviderError) -> ImageProviderError {
        let last = self
            .last_package_line
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        match last.as_deref().and_then(download_package) {
            Some(package) => ImageProviderError {
                message: format!(
                    "offline: '{package}' is not in the download cache '{}' and the build has no network; \
                     run 'gaia prefetch' (or 'gaia run --prefetch') with network access first. {}",
                    self.download_dir.display(),
                    error.message
                ),
                ..error
            },
            None => error,
        }
    }
}

/// The package named by a `>>> <package> <version> Downloading` make line.
fn download_package(line: &str) -> Option<String> {
    let rest = line.strip_prefix(">>> ")?;
    let (names, _) = rest.split_once(" Downloading")?;
    let names = names.trim();
    (!names.is_empty()).then(|| names.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_line_names_the_package_and_version() {
        assert_eq!(
            download_package(">>> libfoo 1.2.3 Downloading").as_deref(),
            Some("libfoo 1.2.3")
        );
        assert_eq!(download_package(">>> libfoo 1.2.3 Building"), None);
        assert_eq!(download_package("plain line Downloading"), None);
    }
}
