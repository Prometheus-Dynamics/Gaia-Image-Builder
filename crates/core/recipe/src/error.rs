//! A recipe error: the file, the recipe, the key, what is wrong.

use std::fmt;
use std::path::{Path, PathBuf};

/// What is wrong, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The file the wrong value is written in: the recipe, or the file that
    /// gave a parameter's value.
    pub file: PathBuf,
    pub line: Option<usize>,
    /// The recipe, once its name is known.
    pub recipe: Option<String>,
    /// The key (`stages.pack`) or `parameters.<name>`; empty for the file.
    pub key: String,
    pub message: String,
}

impl Error {
    pub(crate) fn new(file: &Path, key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            file: file.to_path_buf(),
            line: None,
            recipe: None,
            key: key.into(),
            message: message.into(),
        }
    }

    pub(crate) fn recipe(mut self, name: &str) -> Self {
        self.recipe = Some(name.to_string());
        self
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
        if let Some(recipe) = &self.recipe {
            write!(f, ": recipe {recipe}")?;
        }
        if !self.key.is_empty() {
            write!(f, ": {}", self.key)?;
        }
        write!(f, ": {}", self.message)
    }
}

impl std::error::Error for Error {}
