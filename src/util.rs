//! Low-level decoders, ported from MPXJ's `ByteArrayHelper` and `MPPUtility`.
//!
//! MPP stores everything little-endian. Dates are an offset from a fixed epoch
//! of 1983-12-31, with the day count and a "tenths of a minute" time-of-day.

use chrono::{Duration, NaiveDate, NaiveDateTime};

/// MS Project's date epoch (`MicrosoftProjectConstants.EPOCH_DATE`).
fn epoch() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1983, 12, 31)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

#[inline]
pub fn get_u16(data: &[u8], offset: usize) -> Option<u16> {
    let b = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

#[inline]
#[allow(dead_code)] // extension surface (e.g. signed 16-bit fields)
pub fn get_i16(data: &[u8], offset: usize) -> Option<i16> {
    get_u16(data, offset).map(|v| v as i16)
}

#[inline]
pub fn get_u32(data: &[u8], offset: usize) -> Option<u32> {
    let b = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[inline]
pub fn get_i32(data: &[u8], offset: usize) -> Option<i32> {
    get_u32(data, offset).map(|v| v as i32)
}

#[inline]
pub fn get_f64(data: &[u8], offset: usize) -> Option<f64> {
    let b = data.get(offset..offset + 8)?;
    let value = f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    Some(if value.is_nan() { 0.0 } else { value })
}

/// `MPPUtility.getTimestamp(data, offset)`.
///
/// Layout at `offset`: `[time: u16][days: u16]`. `time` is in units of six
/// seconds (a tenth of a minute). Returns `None` for the sentinel values MS
/// Project uses to mean "NA", mirroring MPXJ's heuristics.
pub fn get_timestamp(data: &[u8], offset: usize) -> Option<NaiveDateTime> {
    let days = get_u16(data, offset + 2)? as i64;
    if days <= 1 || days == 65535 {
        return None;
    }
    let mut time = get_u16(data, offset)? as i64;
    if time == 65535 {
        time = 0;
    }
    let result = epoch() + Duration::days(days) + Duration::seconds(time * 6);

    // MPXJ heuristic: tiny day counts with a non-zero time component are
    // treated as NA rather than real dates.
    if days < 100 && result.and_utc().timestamp() % 60 != 0 {
        return None;
    }
    Some(result)
}

/// `MPPUtility.getUnicodeString(data, offset)` — NUL-terminated UTF-16LE.
pub fn get_unicode_string(data: &[u8], offset: usize) -> String {
    let mut units = Vec::new();
    let mut i = offset;
    while i + 1 < data.len() {
        let u = u16::from_le_bytes([data[i], data[i + 1]]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    String::from_utf16_lossy(&units)
}

/// Format a timestamp as ISO-8601 (no timezone), the shape Typst's
/// `datetime` helpers expect: `YYYY-MM-DDTHH:MM:SS`.
pub fn to_iso(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_endian_ints() {
        let d = [0x01, 0x02, 0x03, 0x04];
        assert_eq!(get_u16(&d, 0), Some(0x0201));
        assert_eq!(get_u32(&d, 0), Some(0x04030201));
        assert_eq!(get_u16(&d, 3), None); // out of bounds
    }

    #[test]
    fn timestamp_epoch_plus_days() {
        // days = 366 (epoch is 1983-12-31, so +366 days = 1984-12-31),
        // time = 480 tenths-of-a-minute = 480 * 6s = 48 min past midnight = 00:48.
        let mut d = [0u8; 4];
        d[0..2].copy_from_slice(&480u16.to_le_bytes());
        d[2..4].copy_from_slice(&366u16.to_le_bytes());
        let ts = get_timestamp(&d, 0).expect("valid timestamp");
        assert_eq!(to_iso(ts), "1984-12-31T00:48:00");
    }

    #[test]
    fn timestamp_na_sentinels() {
        let mut d = [0u8; 4];
        d[2..4].copy_from_slice(&65535u16.to_le_bytes());
        assert_eq!(get_timestamp(&d, 0), None);
        d[2..4].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(get_timestamp(&d, 0), None); // days <= 1
    }

    #[test]
    fn unicode_string_nul_terminated() {
        // "Hi" then a NUL word, then trailing junk that must be ignored.
        let mut d = Vec::new();
        for c in "Hi".encode_utf16() {
            d.extend_from_slice(&c.to_le_bytes());
        }
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&0x4242u16.to_le_bytes());
        assert_eq!(get_unicode_string(&d, 0), "Hi");
    }
}
