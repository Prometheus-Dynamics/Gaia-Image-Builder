#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdError {
    message: &'static str,
}

impl IdError {
    pub fn empty() -> Self {
        Self {
            message: "id must not be empty",
        }
    }
}

impl std::fmt::Display for IdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for IdError {}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn try_new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                if value.trim().is_empty() {
                    Err(IdError::empty())
                } else {
                    Ok(Self(value))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn is_valid(&self) -> bool {
                !self.as_str().trim().is_empty()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::try_new(value)
            }
        }
    };
}

id_type!(BuildId);
id_type!(SourceId);
id_type!(ArtifactId);
id_type!(InstallId);
id_type!(StageItemId);
id_type!(AssemblyTreeId);
id_type!(AssemblyFilesystemId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_newtypes_display_and_borrow_as_str() {
        use std::borrow::Borrow;

        let source = SourceId::new("workspace-root");

        assert_eq!(source.to_string(), "workspace-root");
        assert_eq!(source.as_ref(), "workspace-root");
        assert_eq!(Borrow::<str>::borrow(&source), "workspace-root");
        assert_eq!("workspace-root".parse::<SourceId>().unwrap(), source);
    }

    #[test]
    fn id_newtypes_reject_empty_fallible_construction() {
        assert_eq!(SourceId::try_new(""), Err(IdError::empty()));
        assert_eq!(SourceId::try_new(" \t\n"), Err(IdError::empty()));
        assert!("".parse::<SourceId>().is_err());
        assert!(!SourceId::new("").is_valid());
    }
}
