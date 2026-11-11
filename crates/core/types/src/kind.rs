//! Open kinds: an enum of known names plus `Other(name)`, written as the
//! plain name.

/// Declares an open kind. Parsing a known name gives its variant, so
/// `Other` never holds a known name.
macro_rules! open_kind {
    ($(#[$meta:meta])* $name:ident, $key:literal { $($(#[$vmeta:meta])* $variant:ident => $text:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::daedalus::prelude::Type)]
        #[type_key($key)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
            /// A name outside the known list.
            Other(String),
        }

        impl $name {
            /// The known names, in declaration order.
            pub const KNOWN: &[&str] = &[$($text),*];

            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $text,)*
                    Self::Other(name) => name,
                }
            }

            pub fn parse(name: &str) -> Self {
                match name {
                    $($text => Self::$variant,)*
                    other => Self::Other(other.to_string()),
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let name = <String as ::serde::Deserialize>::deserialize(deserializer)?;
                Ok(Self::parse(&name))
            }
        }
    };
}

pub(crate) use open_kind;
