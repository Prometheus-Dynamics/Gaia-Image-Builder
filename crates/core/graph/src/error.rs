//! A graph error, naming the description it came from.

use std::fmt;
use std::path::PathBuf;

/// What is wrong, and which description files it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The descriptions to look at; empty when none could be named.
    pub files: Vec<PathBuf>,
    /// The key in the first file, when one is known.
    pub key: String,
    pub message: String,
}

impl Error {
    pub(crate) fn at(
        file: &std::path::Path,
        key: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            files: vec![file.to_path_buf()],
            key: key.into(),
            message: message.into(),
        }
    }

    pub(crate) fn plan(files: Vec<PathBuf>, message: &str) -> Self {
        Self {
            files,
            key: String::new(),
            message: format!("the planner refused the graph: {message}"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let files: Vec<String> = self.files.iter().map(|p| p.display().to_string()).collect();
        if !files.is_empty() {
            write!(f, "{}: ", files.join(", "))?;
        }
        if !self.key.is_empty() {
            write!(f, "{}: ", self.key)?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
