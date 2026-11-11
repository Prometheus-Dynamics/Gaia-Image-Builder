//! A description error: the file, the key, and what is wrong.

use std::fmt;
use std::path::{Path, PathBuf};

/// What is wrong with a description, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The file the key is written in.
    pub file: PathBuf,
    /// The line, when it is known.
    pub line: Option<usize>,
    /// The key, as a dotted path (`kernel.fragments[1]`); empty for the
    /// file as a whole.
    pub key: String,
    pub message: String,
}

impl Error {
    pub(crate) fn new(file: &Path, key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            file: file.to_path_buf(),
            line: None,
            key: key.into(),
            message: message.into(),
        }
    }

    pub(crate) fn at_line(mut self, line: Option<usize>) -> Self {
        self.line = line;
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file.display())?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        if !self.key.is_empty() {
            write!(f, ": {}", self.key)?;
        }
        write!(f, ": {}", self.message)
    }
}

impl std::error::Error for Error {}
