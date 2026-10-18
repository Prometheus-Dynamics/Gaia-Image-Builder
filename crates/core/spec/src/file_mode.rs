//! File modes of assembly entries: an octal `mode` string, checked and kept as bits.

use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FileMode(u32);

impl FileMode {
    pub fn bits(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FileModeParseError {
    Empty,
    InvalidOctal(String),
    OutOfRange(u32),
}

impl std::fmt::Display for FileModeParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("file mode cannot be empty"),
            Self::InvalidOctal(value) => {
                write!(formatter, "file mode '{value}' must be an octal value")
            }
            Self::OutOfRange(value) => {
                write!(formatter, "file mode '{value:o}' must not exceed 07777")
            }
        }
    }
}

impl FromStr for FileMode {
    type Err = FileModeParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(FileModeParseError::Empty);
        }
        if !trimmed
            .chars()
            .all(|character| matches!(character, '0'..='7'))
        {
            return Err(FileModeParseError::InvalidOctal(raw.into()));
        }
        let parsed = u32::from_str_radix(trimmed, 8)
            .map_err(|_| FileModeParseError::InvalidOctal(raw.into()))?;
        if parsed > 0o7777 {
            return Err(FileModeParseError::OutOfRange(parsed));
        }
        Ok(Self(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_mode_parses_octal_modes_including_zero() {
        assert_eq!("0000".parse::<FileMode>().expect("0000").bits(), 0o0000);
        assert_eq!("0644".parse::<FileMode>().expect("0644").bits(), 0o0644);
        assert!("0888".parse::<FileMode>().is_err());
        assert!("10000".parse::<FileMode>().is_err());
    }
}
