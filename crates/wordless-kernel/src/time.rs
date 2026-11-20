//! Platform clock, identical on every target.
//!
//! Dates in D1 stay ISO-8601 text in UTC, as in v1 (`db.ts` `nowIso()`).
//! The civil-date conversion is Howard Hinnant's days-from-civil
//! algorithm, hand-rolled so no date crate is needed.

#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Current UTC time as an ISO-8601 string, e.g. `2026-11-19T17:00:00.000Z`.
pub fn now_iso() -> String {
    iso_from_ms(now_ms())
}

/// Format an epoch-millisecond timestamp as ISO-8601 UTC.
pub fn iso_from_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let millis = ms % 1000;
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (hour, minute, second) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, minute, second, millis
    )
}

/// Parse an ISO-8601 UTC string (`...T...:...:...[.fff]Z`) to epoch ms.
/// Returns `None` on malformed input. Only the format this codebase
/// writes is accepted.
pub fn ms_from_iso(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<u64> {
        std::str::from_utf8(&b[a..z]).ok()?.parse::<u64>().ok()
    };
    let year = num(0, 4)? as i64;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month as i64, day as i64);
    let mut ms = ((days * 86_400 + (hour * 3600 + minute * 60 + second) as i64) * 1000) as u64;
    // Optional fractional seconds (index 19 is '.', digits start at 20).
    if b.len() > 20 && b[19] == b'.' {
        let frac: String = s[20..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if !frac.is_empty() {
            let digits = frac.len() as u32;
            let f: u64 = frac.parse().ok()?;
            ms += if digits >= 3 {
                f / 10u64.pow(digits - 3)
            } else {
                f * 10u64.pow(3 - digits)
            };
        }
    }
    Some(ms)
}

fn civil_from_days(z_in: i64) -> (i64, u32, u32) {
    let z = z_in + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

fn days_from_civil(y_in: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y_in - 1 } else { y_in };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        // 1970-01-01T00:00:00.000Z
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00.000Z");
        // 2026-11-19T12:00:00.000Z — verified against `date -u`.
        assert_eq!(iso_from_ms(1_795_089_600_000), "2026-11-19T12:00:00.000Z");
        // Leap day.
        assert_eq!(iso_from_ms(1_709_164_800_000), "2024-02-29T00:00:00.000Z");
        // End of year.
        assert_eq!(iso_from_ms(1_767_225_599_999), "2025-12-31T23:59:59.999Z");
    }

    #[test]
    fn iso_round_trip() {
        for ms in [0u64, 1, 999, 1_795_089_600_123, 1_767_225_599_999] {
            let s = iso_from_ms(ms);
            assert_eq!(ms_from_iso(&s), Some(ms), "round trip failed for {s}");
        }
        assert_eq!(ms_from_iso("garbage"), None);
        assert_eq!(ms_from_iso("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn now_iso_is_well_formed() {
        let s = now_iso();
        assert_eq!(s.len(), 24);
        assert!(ms_from_iso(&s).is_some());
    }
}
