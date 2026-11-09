//! UTC timestamps without external crates.
//!
//! Session logs are named after the session start time and every event carries
//! an RFC 3339 timestamp. The prototype is `std`-only, so instead of pulling in
//! a date-time crate the civil-date conversion is implemented here (Howard
//! Hinnant's `civil_from_days`) and covered by tests with known instants.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current time.
pub fn now() -> SystemTime {
    SystemTime::now()
}

/// RFC 3339 in UTC: `2026-11-09T18:36:07Z`.
pub fn rfc3339(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = parts(time);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// File-name stamp in UTC: `20261109-183607`.
pub fn file_stamp(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = parts(time);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn parts(time: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let within_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        (within_day / 3_600) as u32,
        ((within_day % 3_600) / 60) as u32,
        (within_day % 60) as u32,
    )
}

/// Days since 1970-01-01 → (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, file_stamp, rfc3339};
    use std::time::{Duration, UNIX_EPOCH};

    fn at(seconds: u64) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(rfc3339(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(file_stamp(at(0)), "19700101-000000");
    }

    #[test]
    fn known_instants_are_formatted() {
        // 2023-11-14T22:13:20Z
        assert_eq!(rfc3339(at(1_700_000_000)), "2023-11-14T22:13:20Z");
        assert_eq!(file_stamp(at(1_700_000_000)), "20231114-221320");
        // 2024-02-29T12:00:00Z — leap day.
        assert_eq!(rfc3339(at(1_709_208_000)), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn civil_conversion_handles_month_boundaries() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(31), (1970, 2, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
