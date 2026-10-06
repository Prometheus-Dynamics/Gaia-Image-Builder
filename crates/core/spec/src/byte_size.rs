use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSize(u64);

impl ByteSize {
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    pub fn bytes(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByteSizeParseError {
    Empty,
    InvalidNumber(String),
    Overflow(String),
}

impl std::fmt::Display for ByteSizeParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("byte size cannot be empty"),
            Self::InvalidNumber(value) => write!(formatter, "invalid byte size '{value}'"),
            Self::Overflow(value) => write!(formatter, "byte size '{value}' is too large"),
        }
    }
}

impl FromStr for ByteSize {
    type Err = ByteSizeParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ByteSizeParseError::Empty);
        }
        let (number, multiplier) = match trimmed.as_bytes().last().copied() {
            Some(b'K' | b'k') => (&trimmed[..trimmed.len() - 1], 1024u64),
            Some(b'M' | b'm') => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024),
            Some(b'G' | b'g') => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024 * 1024),
            Some(b'T' | b't') => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024 * 1024 * 1024),
            _ => (trimmed, 1u64),
        };
        let value = number
            .trim()
            .parse::<u64>()
            .map_err(|_| ByteSizeParseError::InvalidNumber(raw.into()))?;
        let bytes = value
            .checked_mul(multiplier)
            .ok_or_else(|| ByteSizeParseError::Overflow(raw.into()))?;
        Ok(Self(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_size_parses_suffixes_and_rejects_overflow() {
        assert_eq!("512".parse::<ByteSize>().expect("bytes").bytes(), 512);
        assert_eq!("1K".parse::<ByteSize>().expect("kib").bytes(), 1024);
        assert_eq!(
            "2M".parse::<ByteSize>().expect("mib").bytes(),
            2 * 1024 * 1024
        );
        assert_eq!(
            "3G".parse::<ByteSize>().expect("gib").bytes(),
            3 * 1024 * 1024 * 1024
        );
        assert_eq!(
            "2T".parse::<ByteSize>().expect("tib").bytes(),
            2 * 1024 * 1024 * 1024 * 1024
        );
        assert!("not-a-size".parse::<ByteSize>().is_err());
        assert!(format!("{}G", u64::MAX).parse::<ByteSize>().is_err());
    }
}
