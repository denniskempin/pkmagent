//! Civil-date windows, the frozen clock, and RFC3339 cursors.

use chrono::{DateTime, FixedOffset, NaiveDate, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

#[derive(Clone, Debug)]
pub struct Clock {
    pub zone: Tz,
    pub now: DateTime<FixedOffset>,
}

impl Clock {
    pub fn local() -> Self {
        let zone = system_zone();
        Self {
            now: freeze_now(zone),
            zone,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
    pub day: NaiveDate,
}

pub fn system_zone() -> Tz {
    if let Ok(name) = std::env::var("TZ") {
        if let Some(zone) = parse_zone_name(&name) {
            return zone;
        }
    }
    if let Ok(path) = std::fs::read_link("/etc/localtime") {
        let text = path.to_string_lossy();
        if let Some(idx) = text.find("zoneinfo/") {
            if let Some(zone) = parse_zone_name(&text[idx + "zoneinfo/".len()..]) {
                return zone;
            }
        }
    }
    chrono_tz::UTC
}

pub fn parse_zone_name(name: &str) -> Option<Tz> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    name.parse::<Tz>().ok()
}

pub fn freeze_now(zone: Tz) -> DateTime<FixedOffset> {
    floor_second(Utc::now().with_timezone(&zone).fixed_offset())
}

pub fn floor_second(dt: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    dt.with_nanosecond(0).unwrap_or(dt)
}

pub fn format_rfc3339(dt: DateTime<FixedOffset>) -> String {
    floor_second(dt).format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

pub fn run_folder_name(end: DateTime<FixedOffset>) -> String {
    let floored = floor_second(end);
    let secs = floored.offset().fix().local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let secs = secs.abs();
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    format!(
        "{}T{:02}{:02}{:02}{}{:02}{:02}",
        floored.format("%Y-%m-%d"),
        floored.hour(),
        floored.minute(),
        floored.second(),
        sign,
        hh,
        mm
    )
}

pub fn parse_offset_rfc3339(value: &str) -> Option<DateTime<FixedOffset>> {
    if !is_strict_offset_rfc3339(value) {
        return None;
    }
    DateTime::parse_from_rfc3339(value).ok()
}

fn is_strict_offset_rfc3339(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 25 {
        return false;
    }
    let digit = |i: usize| b[i].is_ascii_digit();
    digit(0)
        && digit(1)
        && digit(2)
        && digit(3)
        && b[4] == b'-'
        && digit(5)
        && digit(6)
        && b[7] == b'-'
        && digit(8)
        && digit(9)
        && b[10] == b'T'
        && digit(11)
        && digit(12)
        && b[13] == b':'
        && digit(14)
        && digit(15)
        && b[16] == b':'
        && digit(17)
        && digit(18)
        && (b[19] == b'+' || b[19] == b'-')
        && digit(20)
        && digit(21)
        && b[22] == b':'
        && digit(23)
        && digit(24)
}

pub fn local_midnight(date: NaiveDate, zone: Tz) -> DateTime<FixedOffset> {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight");
    match zone.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt.fixed_offset(),
        chrono::LocalResult::Ambiguous(early, _) => early.fixed_offset(),
        chrono::LocalResult::None => {
            let later = date.and_hms_opt(3, 0, 0).expect("hour");
            zone.from_local_datetime(&later)
                .earliest()
                .expect("zone offset")
                .fixed_offset()
        }
    }
}

pub fn civil_dates(
    earliest_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
    zone: Tz,
) -> Vec<NaiveDate> {
    if earliest_start >= window_end {
        return Vec::new();
    }
    let start_date = earliest_start.with_timezone(&zone).date_naive();
    let last = window_end.with_timezone(&zone) - chrono::Duration::nanoseconds(1);
    let end_date = last.date_naive();
    let mut out = Vec::new();
    let mut day = start_date;
    loop {
        out.push(day);
        if day >= end_date {
            break;
        }
        day = day.succ_opt().expect("civil date");
    }
    out
}

pub fn slice_bounds(
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
    day: NaiveDate,
    zone: Tz,
) -> Option<Slice> {
    let day_start = local_midnight(day, zone);
    let next = day.succ_opt()?;
    let day_end = local_midnight(next, zone);
    let start = max_instant(window_start, day_start);
    let end = min_instant(window_end, day_end);
    if start >= end {
        None
    } else {
        Some(Slice { start, end, day })
    }
}

pub fn whole_day_slice(day: NaiveDate, zone: Tz) -> Slice {
    let start = local_midnight(day, zone);
    let next = day.succ_opt().expect("next date");
    let end = local_midnight(next, zone);
    Slice { start, end, day }
}

fn max_instant(a: DateTime<FixedOffset>, b: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    if a >= b {
        a
    } else {
        b
    }
}

fn min_instant(a: DateTime<FixedOffset>, b: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    if a <= b {
        a
    } else {
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::America::Los_Angeles;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn la(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn folder_stamp_and_strict_rfc3339() {
        let end = la("2026-09-27T10:00:00-07:00");
        assert_eq!(run_folder_name(end), "2026-09-27T100000-0700");
        assert_eq!(format_rfc3339(end), "2026-09-27T10:00:00-07:00");
        let positive = la("2026-09-27T10:00:00+09:00");
        assert_eq!(run_folder_name(positive), "2026-09-27T100000+0900");
        assert!(parse_offset_rfc3339("2026-09-27T18:04:11-07:00").is_some());
        assert!(parse_offset_rfc3339("2026-09-27T18:04:11Z").is_none());
        assert!(parse_offset_rfc3339("2026-09-27T18:04:11.5-07:00").is_none());
        assert!(parse_offset_rfc3339("2026-02-31T00:00:00-07:00").is_none());
        let floored = floor_second(la("2026-09-27T10:00:00.900-07:00"));
        assert_eq!(floored, la("2026-09-27T10:00:00-07:00"));
    }

    #[test]
    fn worked_example_and_three_dates() {
        let zone = Los_Angeles;
        let end = la("2026-09-27T10:00:00-07:00");
        let since = local_midnight(date(2026, 9, 26), zone);
        assert_eq!(since, la("2026-09-26T00:00:00-07:00"));
        assert_eq!(
            civil_dates(since, end, zone),
            vec![date(2026, 9, 26), date(2026, 9, 27)]
        );
        let since_25 = local_midnight(date(2026, 9, 25), zone);
        assert_eq!(
            civil_dates(since_25, end, zone),
            vec![date(2026, 9, 25), date(2026, 9, 26), date(2026, 9, 27)]
        );
        assert!(civil_dates(end, end, zone).is_empty());
        let midnight = la("2026-09-27T00:00:00-07:00");
        assert_eq!(
            civil_dates(la("2026-09-26T00:00:00-07:00"), midnight, zone),
            vec![date(2026, 9, 26)]
        );
    }

    #[test]
    fn slice_clips_to_the_category_window() {
        let zone = Los_Angeles;
        let end = la("2026-09-27T10:00:00-07:00");
        let start = la("2026-09-27T10:00:00-07:00");
        assert!(slice_bounds(start, end, date(2026, 9, 27), zone).is_none());
        let slice = slice_bounds(
            la("2026-09-26T00:00:00-07:00"),
            end,
            date(2026, 9, 27),
            zone,
        )
        .unwrap();
        assert_eq!(slice.start, la("2026-09-27T00:00:00-07:00"));
        assert_eq!(slice.end, end);
        let whole = whole_day_slice(date(2026, 9, 26), zone);
        assert_eq!(whole.start, la("2026-09-26T00:00:00-07:00"));
        assert_eq!(whole.end, la("2026-09-27T00:00:00-07:00"));
    }

    #[test]
    fn dst_spring_forward_and_fall_back() {
        let zone = Los_Angeles;
        let spring_start = local_midnight(date(2026, 3, 7), zone);
        let spring_end = local_midnight(date(2026, 3, 10), zone);
        assert_eq!(
            civil_dates(spring_start, spring_end, zone),
            vec![date(2026, 3, 7), date(2026, 3, 8), date(2026, 3, 9)]
        );
        let spring = whole_day_slice(date(2026, 3, 8), zone);
        assert_eq!(
            (spring.end - spring.start).num_hours(),
            23,
            "spring forward is 23 hours"
        );

        let fall_start = local_midnight(date(2026, 10, 31), zone);
        let fall_end = local_midnight(date(2026, 11, 3), zone);
        assert_eq!(
            civil_dates(fall_start, fall_end, zone),
            vec![date(2026, 10, 31), date(2026, 11, 1), date(2026, 11, 2)]
        );
        let fall = whole_day_slice(date(2026, 11, 1), zone);
        assert_eq!(
            (fall.end - fall.start).num_hours(),
            25,
            "fall back is local midnight to the next local midnight"
        );
        assert_eq!(fall.start, la("2026-11-01T00:00:00-07:00"));
        assert_eq!(fall.end, la("2026-11-02T00:00:00-08:00"));
    }
}
