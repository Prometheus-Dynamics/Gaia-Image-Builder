use std::fmt;

#[derive(Debug)]
pub struct Error {
    msg: String,
}

impl Error {
    pub fn msg<M: Into<String>>(msg: M) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::msg(err.to_string())
    }
}

impl From<toml::de::Error> for Error {
    fn from(err: toml::de::Error) -> Self {
        Self::msg(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
