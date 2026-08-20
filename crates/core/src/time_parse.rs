// SPDX-License-Identifier: MIT

//! Timestamp parsing & serialization.
//!
//! - `extract_time`: try the full `TIME_FORMATS` list (mtga_follower.py:146-158).
//! - `maybe_get_utc_timestamp`: ms-since-epoch / .NET-ticks / ISO-8601 branches.
//! - Output matches Python `datetime.isoformat()`: naive (no tz offset), no microseconds
//!   when zero, exactly 6 fractional digits when present. Validated against the reference
//!   Python client - e.g. ticks `639179113099292149` → `2026-06-24T15:21:49.929214`.

#![allow(dead_code)]

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use serde_json::Value;

/// strptime formats tried in order (`mtga_follower.py:146-158`, including the duplicate).
const TIME_FORMATS: &[&str] = &[
    "%Y-%m-%d %I:%M:%S %p",
    "%Y-%m-%d %H:%M:%S",
    "%m/%d/%Y %I:%M:%S %p",
    "%m/%d/%Y %H:%M:%S",
    "%Y/%m/%d %I:%M:%S %p",
    "%Y/%m/%d %H:%M:%S",
    "%Y/%m/%d %I:%M:%S %p",
    "%d/%m/%Y %H:%M:%S",
    "%d/%m/%Y %I:%M:%S %p",
    "%d.%m.%Y %H:%M:%S",
    "%d.%m.%Y %I:%M:%S %p",
];

/// `datetime.datetime.fromtimestamp(0)` - local-naive epoch (the init value for
/// `cur_log_time` / `last_utc_time`).
pub fn epoch_zero() -> NaiveDateTime {
    Local
        .timestamp_opt(0, 0)
        .single()
        .expect("epoch 0 is a valid local time")
        .naive_local()
}

/// `int(1000 * datetime.datetime(3000, 1, 1).timestamp())` (`mtga_follower.py:160`).
///
/// Local-timezone dependent, like Python; computed at runtime so it matches the Python
/// client on the same machine. Below this value a timestamp is ms-since-epoch; above it,
/// .NET ticks.
fn max_milliseconds_since_epoch() -> i64 {
    let dt = NaiveDate::from_ymd_opt(3000, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let local = Local
        .from_local_datetime(&dt)
        .single()
        .expect("3000-01-01 is a valid local time");
    1000 * local.timestamp()
}

/// Convert a raw log time string to a datetime (port of `extract_time`).
///
/// Strips trailing `:`/` `/`/` (the `STRIPPED_TIMESTAMP_REGEX` run), cuts at the first
/// `": "`, then tries each `TIME_FORMATS` entry; errors if none match.
pub fn extract_time(time_str: &str) -> Result<NaiveDateTime, String> {
    // STRIPPED_TIMESTAMP_REGEX `^(.*?)[: /]*$`: drop the trailing run of ':' ' ' '/'.
    let stripped = time_str.trim_end_matches([':', ' ', '/']);

    // Python: if ": " in s: s = s.split(": ")[0]
    let candidate = match stripped.split_once(": ") {
        Some((head, _)) => head,
        None => stripped,
    };

    for fmt in TIME_FORMATS {
        if let Ok(dt) = NaiveDateTime::parse_from_str(candidate, fmt) {
            return Ok(dt);
        }
    }
    Err(format!("Unsupported time format: \"{candidate}\""))
}

/// Serialize a datetime the way Python `datetime.isoformat()` does: naive (no
/// offset), microseconds shown as exactly 6 digits only when non-zero.
pub fn isoformat(dt: &NaiveDateTime) -> String {
    let micros = dt.nanosecond() / 1_000; // truncate ns -> us, matching Python's resolution
    let base = dt.format("%Y-%m-%dT%H:%M:%S");
    if micros == 0 {
        base.to_string()
    } else {
        format!("{base}.{micros:06}")
    }
}

/// Pull a UTC timestamp out of a decoded blob (port of `__maybe_get_utc_timestamp`).
/// Looks at `timestamp`, then `payloadObject.timestamp`, then
/// `params.payloadObject.timestamp`; interprets it as ms-since-epoch, .NET ticks, or an
/// ISO-8601 string.
pub fn maybe_get_utc_timestamp(blob: &Value) -> Option<NaiveDateTime> {
    // Mirror Python's key-presence precedence: a present-but-null top-level `timestamp`
    // short-circuits to None (does not fall through to payloadObject).
    let timestamp = blob
        .get("timestamp")
        .or_else(|| blob.get("payloadObject").and_then(|p| p.get("timestamp")))
        .or_else(|| {
            blob.get("params")
                .and_then(|p| p.get("payloadObject"))
                .and_then(|p| p.get("timestamp"))
        })?;

    if timestamp.is_null() {
        return None;
    }

    // Python: try int(timestamp). Integers and all-digit strings take the numeric path;
    // a non-integer string raises ValueError and falls to isoparse.
    if let Some(value) = value_to_int(timestamp) {
        if value < max_milliseconds_since_epoch() {
            from_milliseconds(value)
        } else {
            from_dotnet_ticks(value)
        }
    } else if let Some(s) = timestamp.as_str() {
        isoparse(s)
    } else {
        None
    }
}

/// `int(timestamp)`: integer JSON numbers as-is, floats truncated toward zero, all-integer
/// strings parsed; anything else (non-integer string, etc.) is `None`.
fn value_to_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if let Some(u) = n.as_u64() {
                Some(u as i64)
            } else {
                n.as_f64().map(|f| f.trunc() as i64)
            }
        }
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// ms-since-epoch → local-naive datetime (Python `fromtimestamp(ms * 0.001)`).
fn from_milliseconds(ms: i64) -> Option<NaiveDateTime> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.naive_local())
}

/// .NET ticks (100ns since 0001-01-01) → naive datetime
/// (Python `fromordinal(1) + timedelta(seconds=value / 1e7)`).
///
/// Must reproduce Python's **lossy f64** path, not exact integer math: Python evaluates
/// `value / 10000000` as a correctly-rounded float of the big integer, so the result is
/// quantized to the f64 ulp (~15µs at this magnitude). Critically, `ticks as f64 / 1e7`
/// is *not* equivalent - casting the ~6.4e17 tick count to f64 first loses up to ~6µs and
/// can land on a different microsecond. We instead split into whole seconds + remainder so
/// the float division stays small and matches Python's `int / int`; then round the
/// fractional microseconds half-to-even like Python's `round()`.
// WIRE: must reproduce Python's lossy `int / int` float division (see doc above).
fn from_dotnet_ticks(ticks: i64) -> Option<NaiveDateTime> {
    let base = NaiveDate::from_ymd_opt(1, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let whole_seconds = ticks / 10_000_000; // exact in f64 (< 2^53)
    let remainder = ticks % 10_000_000;
    let x = whole_seconds as f64 + remainder as f64 / 10_000_000.0;
    let floor = x.floor();
    let mut secs = floor as i64;
    let mut micros = round_half_even((x - floor) * 1_000_000.0);
    if micros >= 1_000_000 {
        secs += 1;
        micros -= 1_000_000;
    }
    base.checked_add_signed(Duration::seconds(secs))?
        .checked_add_signed(Duration::microseconds(micros))
}

/// Round half-to-even (banker's rounding), matching Python's built-in `round()`, for the
/// non-negative microsecond fractions produced by the ticks conversion.
fn round_half_even(x: f64) -> i64 {
    let floor = x.floor();
    let diff = x - floor;
    if diff < 0.5 {
        floor as i64
    } else if diff > 0.5 {
        floor as i64 + 1
    } else {
        let f = floor as i64;
        if f % 2 == 0 { f } else { f + 1 }
    }
}

/// Best-effort ISO-8601 parse (Python `dateutil.parser.isoparse`). This branch is not
/// exercised by the available fixtures; revisit against the Python source if it ever fires.
fn isoparse(s: &str) -> Option<NaiveDateTime> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.naive_utc());
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_time_real_log_format() {
        let dt = extract_time("6/24/2026 4:21:38 PM").unwrap();
        assert_eq!(isoformat(&dt), "2026-06-24T16:21:38");
    }

    #[test]
    fn extract_time_strips_trailing_and_cuts_at_colon_space() {
        // Trailing ": " junk and a payload suffix are removed before parsing.
        let dt = extract_time("6/24/2026 4:21:38 PM: 3").unwrap();
        assert_eq!(isoformat(&dt), "2026-06-24T16:21:38");
        let dt2 = extract_time("2026-06-24 16:21:38 ").unwrap();
        assert_eq!(isoformat(&dt2), "2026-06-24T16:21:38");
    }

    #[test]
    fn extract_time_rejects_garbage() {
        assert!(extract_time("not a time").is_err());
    }

    #[test]
    fn isoformat_no_micros_when_zero() {
        let dt = NaiveDate::from_ymd_opt(1970, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(isoformat(&dt), "1970-01-01T00:00:00");
    }

    #[test]
    fn utc_timestamp_dotnet_ticks_matches_python() {
        // .NET ticks values from the real logs, each cross-checked against Python's lossy
        // float division (`fromordinal(1) + timedelta(seconds=value/1e7)`).
        for (ticks, expected) in [
            ("639179113099292149", "2026-06-24T15:21:49.929214"),
            ("639179145175405740", "2026-06-24T16:15:17.540573"), // exact math says .540574
            ("639179085349177814", "2026-06-24T14:35:34.917778"),
            ("639179102208187320", "2026-06-24T15:03:40.818733"),
        ] {
            let dt = maybe_get_utc_timestamp(&json!({ "timestamp": ticks })).unwrap();
            assert_eq!(isoformat(&dt), expected, "ticks={ticks}");
        }
    }

    /// Expected ms-path output for a given epoch-ms value, computed the same way
    /// `from_milliseconds` does. The ms branch mirrors Python `fromtimestamp(ms*0.001)`,
    /// which is **local-timezone dependent**, so the expectation must follow the local
    /// zone too - hardcoding an absolute string would break on any runner not in the
    /// author's zone (e.g. UTC CI). Validated against the Python client on the same machine.
    fn local_ms_isoformat(ms: i64) -> String {
        isoformat(
            &Local
                .timestamp_millis_opt(ms)
                .single()
                .unwrap()
                .naive_local(),
        )
    }

    #[test]
    fn utc_timestamp_milliseconds_matches_python() {
        let dt = maybe_get_utc_timestamp(&json!({"timestamp": "1782314510331"})).unwrap();
        assert_eq!(isoformat(&dt), local_ms_isoformat(1782314510331));
    }

    #[test]
    fn utc_timestamp_numeric_value() {
        // Integer JSON number, same ms value.
        let dt = maybe_get_utc_timestamp(&json!({"timestamp": 1782314510331i64})).unwrap();
        assert_eq!(isoformat(&dt), local_ms_isoformat(1782314510331));
    }

    #[test]
    fn utc_timestamp_from_payload_object_and_params() {
        let expected = local_ms_isoformat(1782314510331);
        let dt = maybe_get_utc_timestamp(&json!({"payloadObject": {"timestamp": "1782314510331"}}))
            .unwrap();
        assert_eq!(isoformat(&dt), expected);
        let dt2 = maybe_get_utc_timestamp(
            &json!({"params": {"payloadObject": {"timestamp": "1782314510331"}}}),
        )
        .unwrap();
        assert_eq!(isoformat(&dt2), expected);
    }

    #[test]
    fn utc_timestamp_absent_or_null_is_none() {
        assert!(maybe_get_utc_timestamp(&json!({"foo": 1})).is_none());
        // Present-but-null short-circuits, ignoring a nested payloadObject timestamp.
        assert!(
            maybe_get_utc_timestamp(
                &json!({"timestamp": null, "payloadObject": {"timestamp": "1782314510331"}})
            )
            .is_none()
        );
    }
}
