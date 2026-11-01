//! The choice fields v2 reads as free strings and checks later. A v3 file
//! gets them checked as it is read, so a typo is an error at its line that
//! lists the values the field takes. The spellings are v2's.

use crate::ConfigError;

const SIGN_KINDS: &[&str] = &["minisign", "gpg", "cosign-blob"];

/// The model filters of the Raspberry Pi firmware's `config.txt`.
const PI_MODELS: &[&str] = &[
    "pi0", "pi0w", "pi02", "pi1", "pi2", "pi3", "pi3+", "pi4", "pi400", "pi5", "pi500", "cm1",
    "cm3", "cm3+", "cm4", "cm4s", "cm5",
];

/// A field of every entry of an array of tables, or of one table.
enum Field {
    Each(&'static str, &'static str),
    One(&'static str),
}

const FIELDS: &[(Field, &[&str])] = &[
    (Field::Each("checkpoints", "backend"), &["local", "oci"]),
    (Field::Each("checkpoints", "sign_kind"), SIGN_KINDS),
    (Field::Each("checkpoints", "trust"), &["unsigned"]),
    (Field::One("image.signing.kind"), SIGN_KINDS),
    (Field::One("image.checks.pi_model"), PI_MODELS),
];

/// Every choice field in `table` holds one of its values. `pi_model` is
/// matched without case, as the firmware matches its filters.
pub(super) fn check(
    table: &toml::Table,
    error: &impl Fn(&str, String) -> ConfigError,
) -> Result<(), ConfigError> {
    for (field, allowed) in FIELDS {
        let values = match field {
            Field::One(key) => super::get(table, key)
                .map(|value| ((*key).to_string(), value))
                .into_iter()
                .collect::<Vec<_>>(),
            Field::Each(array, key) => super::get(table, array)
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(index, entry)| {
                    entry
                        .get(key)
                        .map(|value| (format!("{array}[{index}].{key}"), value))
                })
                .collect(),
        };
        for (key, value) in values {
            let known = value.as_str().is_some_and(|value| {
                allowed
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(value))
            });
            if !known {
                return Err(error(
                    &key,
                    format!(
                        "`{key}` is {value}, which is not one of: {}",
                        allowed.join(", ")
                    ),
                ));
            }
        }
    }
    Ok(())
}
