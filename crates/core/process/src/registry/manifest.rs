//! Entry and signature manifests: built as JSON bytes, read back for their
//! layers and annotations, and stamped with an RFC 3339 creation time.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::{ARTIFACT_TYPE, EMPTY_CONFIG_MEDIA_TYPE, MANIFEST_MEDIA_TYPE, RegistryError};

/// The config blob every entry and signature carries.
pub(super) const EMPTY_CONFIG: &[u8] = b"{}";

/// One blob a manifest refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Descriptor {
    pub(super) media_type: String,
    pub(super) digest: String,
    pub(super) size: u64,
}

/// `sha256:<hex>` of `bytes`.
pub(super) fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{}", crate::sha256_hex_bytes(bytes))
}

/// Serialises a manifest with `layer` as its single layer. The returned bytes
/// are what gets signed and stored, so they are never re-serialised afterwards.
pub(super) fn build(annotations: &BTreeMap<String, String>, layer: &Descriptor) -> Vec<u8> {
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "artifactType": ARTIFACT_TYPE,
        "config": {
            "mediaType": EMPTY_CONFIG_MEDIA_TYPE,
            "digest": digest_of(EMPTY_CONFIG),
            "size": EMPTY_CONFIG.len(),
        },
        "layers": [{
            "mediaType": layer.media_type,
            "digest": layer.digest,
            "size": layer.size,
        }],
        "annotations": annotations,
    });
    serde_json::to_vec(&manifest).unwrap_or_default()
}

/// The parts of a manifest the client reads.
pub(super) struct Parsed {
    pub(super) annotations: BTreeMap<String, String>,
    /// `(mediaType, digest)` of each layer, in order.
    pub(super) layers: Vec<(String, String)>,
}

pub(super) fn parse(bytes: &[u8]) -> Result<Parsed, RegistryError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| RegistryError::Protocol(format!("manifest is not JSON: {error}")))?;
    let annotations: BTreeMap<String, String> = value
        .get("annotations")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let layers = value
        .get("layers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|layer| (text(layer, "mediaType"), text(layer, "digest")))
                .collect()
        })
        .unwrap_or_default();
    Ok(Parsed {
        annotations,
        layers,
    })
}

fn text(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The current UTC time as `YYYY-MM-DDTHH:MM:SSZ` (RFC 3339, whole seconds).
pub(super) fn rfc3339_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs()) as i64;
    let (days, time) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

/// The epoch seconds of a time as `rfc3339_utc_now` writes it
/// (`YYYY-MM-DDTHH:MM:SSZ`). `None` for any other text.
pub fn created_epoch_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let separators = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ];
    if bytes.len() != 20 || separators.iter().any(|&(at, byte)| bytes[at] != byte) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Day count since 1970-01-01 of a civil date (the inverse of
/// [`civil_from_days`]).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Civil (proleptic Gregorian) date of a day count since 1970-01-01.
/// Howard Hinnant's `civil_from_days`, which avoids a date crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod created_tests {
    use super::*;

    #[test]
    fn created_times_round_trip_through_epoch_seconds() {
        assert_eq!(created_epoch_seconds("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(
            created_epoch_seconds("2000-03-01T00:00:00Z"),
            Some(951_868_800)
        );
        let now = rfc3339_utc_now();
        let parsed = created_epoch_seconds(&now).expect("parses");
        let actual = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()) as i64;
        assert!((parsed - actual).abs() < 5, "{now}");
        assert_eq!(created_epoch_seconds("yesterday"), None);
        assert_eq!(created_epoch_seconds("2026-13-01T00:00:00Z"), None);
    }
}
