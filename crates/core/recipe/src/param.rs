//! Parameter types, and a TOML value as a Daedalus constant.

use daedalus::layers::graph::Lit;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What a parameter holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    Bool,
    Int,
    Float,
    String,
    /// A list of strings.
    Strings,
}

impl fmt::Display for ParamType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bool => "a bool",
            Self::Int => "an int",
            Self::Float => "a float",
            Self::String => "a string",
            Self::Strings => "a list of strings",
        })
    }
}

impl ParamType {
    /// `value` as this type's constant, or what it was instead.
    pub fn lit(self, value: &toml::Value) -> Result<Lit, String> {
        match (self, value) {
            (Self::Bool, toml::Value::Boolean(v)) => Ok(Lit::Bool(*v)),
            (Self::Int, toml::Value::Integer(v)) => Ok(Lit::Int(*v)),
            (Self::Float, toml::Value::Float(v)) => Ok(Lit::Float(*v)),
            (Self::Float, toml::Value::Integer(v)) => Ok(Lit::Float(*v as f64)),
            (Self::String, toml::Value::String(v)) => Ok(Lit::Str(v.clone())),
            (Self::Strings, toml::Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    toml::Value::String(v) => Ok(Lit::Str(v.clone())),
                    other => Err(format!("wants {self}, but an item is {}", describe(other))),
                })
                .collect::<Result<_, _>>()
                .map(Lit::List),
            (_, other) => Err(format!("wants {self}, got {}", describe(other))),
        }
    }
}

/// A value, said with its type.
fn describe(value: &toml::Value) -> String {
    match value {
        toml::Value::String(v) => format!("the string {v:?}"),
        toml::Value::Integer(v) => format!("the int {v}"),
        toml::Value::Float(v) => format!("the float {v}"),
        toml::Value::Boolean(v) => format!("the bool {v}"),
        toml::Value::Datetime(v) => format!("the date {v}"),
        toml::Value::Array(_) => "a list".to_string(),
        toml::Value::Table(_) => "a table".to_string(),
    }
}
