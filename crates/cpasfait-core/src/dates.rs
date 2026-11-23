//! Human-friendly date parsing shared by the TUI and the tools.

use jiff::ToSpan;
use jiff::civil::{Date, Weekday};

/// Parse a date relative to `today`:
/// `YYYY-MM-DD`, `today`, `tomorrow`/`tom`, `yesterday`, `+3d`, `-2w`, `+1m`,
/// or a weekday name/prefix (`fri`, `monday`: the next such day, never today).
pub fn parse_date(s: &str, today: Date) -> Option<Date> {
    let s = s.trim().to_lowercase();
    if let Ok(d) = s.parse::<Date>() {
        return Some(d);
    }
    match s.as_str() {
        "today" | "tod" => return Some(today),
        "tomorrow" | "tom" => return today.tomorrow().ok(),
        "yesterday" => return today.yesterday().ok(),
        _ => {}
    }
    if let Some(sign) = s.chars().next().filter(|c| *c == '+' || *c == '-') {
        let body = &s[1..];
        let unit = body.chars().last()?;
        let n: i64 = body[..body.len() - unit.len_utf8()].parse().ok()?;
        let n = if sign == '-' { -n } else { n };
        let span = match unit {
            'd' => n.days(),
            'w' => n.weeks(),
            'm' => n.months(),
            'y' => n.years(),
            _ => return None,
        };
        return today.checked_add(span).ok();
    }
    const DAYS: [(&str, Weekday); 7] = [
        ("monday", Weekday::Monday),
        ("tuesday", Weekday::Tuesday),
        ("wednesday", Weekday::Wednesday),
        ("thursday", Weekday::Thursday),
        ("friday", Weekday::Friday),
        ("saturday", Weekday::Saturday),
        ("sunday", Weekday::Sunday),
    ];
    if s.len() >= 2
        && let Some((_, wd)) = DAYS.iter().find(|(name, _)| name.starts_with(&s))
    {
        let mut d = today.tomorrow().ok()?;
        while d.weekday() != *wd {
            d = d.tomorrow().ok()?;
        }
        return Some(d);
    }
    None
}

/// Parse a moment, at or before `now` (Unix ms), in `tz`. Returns Unix ms.
///
/// - `now`;
/// - a time of day, `14:30`, `14h30`, `14h`, `9:05`: today, or yesterday if
///   that is still to come;
/// - a time ago: `-20m`, `-1h`, `-1h30` (`-1h30m`);
/// - a date (as in [`parse_date`]) then a time: `yesterday 18:00`,
///   `2026-11-22 18:00`.
pub fn parse_time(s: &str, now_ms: i64, tz: &jiff::tz::TimeZone) -> Option<i64> {
    let s = s.trim().to_lowercase();
    let now = jiff::Timestamp::from_millisecond(now_ms)
        .ok()?
        .to_zoned_datetime(tz.clone());
    let at = |date: Date, (h, m): (i8, i8)| -> Option<i64> {
        let dt = date.at(h, m, 0, 0);
        Some(dt.to_zoned_datetime(tz.clone()).ok()?.timestamp().as_millisecond())
    };
    if s == "now" {
        return Some(now_ms);
    }
    if let Some(ago) = s.strip_prefix('-') {
        let (h, m) = match ago.split_once('h') {
            Some((h, m)) => (h.parse::<i64>().ok()?, m.strip_suffix('m').unwrap_or(m)),
            None => (0, ago.strip_suffix('m')?),
        };
        let m: i64 = if m.is_empty() { 0 } else { m.parse().ok()? };
        return Some(now_ms - (h * 60 + m) * 60_000);
    }
    if let Some((day, time)) = s.rsplit_once(' ') {
        let date = parse_date(day, now.date())?;
        return at(date, time_of_day(time)?);
    }
    let t = time_of_day(&s)?;
    let today = at(now.date(), t)?;
    if today <= now_ms {
        Some(today)
    } else {
        at(now.date().yesterday().ok()?, t)
    }
}

/// `14:30`, `14h30`, `14h`, `9:05`.
fn time_of_day(s: &str) -> Option<(i8, i8)> {
    let (h, m) = s.split_once([':', 'h'])?;
    let h: i8 = h.parse().ok()?;
    let m: i8 = if m.is_empty() { 0 } else { m.parse().ok()? };
    ((0..24).contains(&h) && (0..60).contains(&m) && h.to_string().len() <= 2).then_some((h, m))
}

pub fn today_local() -> Date {
    jiff::ZonedDateTime::now().date()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn parses() {
        let today = date(2026, 11, 20); // a Friday
        assert_eq!(parse_date("2026-12-01", today), Some(date(2026, 12, 1)));
        assert_eq!(parse_date("today", today), Some(today));
        assert_eq!(parse_date("tom", today), Some(date(2026, 11, 21)));
        assert_eq!(parse_date("+3d", today), Some(date(2026, 11, 23)));
        assert_eq!(parse_date("-1w", today), Some(date(2026, 11, 13)));
        assert_eq!(parse_date("+1m", today), Some(date(2026, 12, 20)));
        assert_eq!(parse_date("fri", today), Some(date(2026, 11, 27)));
        assert_eq!(parse_date("Monday", today), Some(date(2026, 11, 23)));
        assert_eq!(parse_date("m", today), None);
        assert_eq!(parse_date("garbage", today), None);
    }

    #[test]
    fn times() {
        let tz = jiff::tz::TimeZone::UTC;
        let ms = |s: &str| {
            s.parse::<jiff::civil::DateTime>()
                .unwrap()
                .to_zoned_datetime(tz.clone())
                .unwrap()
                .timestamp()
                .as_millisecond()
        };
        let now = ms("2026-11-23T10:15");
        let p = |s| parse_time(s, now, &tz);
        assert_eq!(p("now"), Some(now));
        assert_eq!(p("9:05"), Some(ms("2026-11-23T09:05")));
        assert_eq!(p("10h15"), Some(now));
        assert_eq!(p("09h"), Some(ms("2026-11-23T09:00")));
        // later today has not happened yet: yesterday
        assert_eq!(p("18:00"), Some(ms("2026-11-22T18:00")));
        assert_eq!(p("-20m"), Some(ms("2026-11-23T09:55")));
        assert_eq!(p("-1h"), Some(ms("2026-11-23T09:15")));
        assert_eq!(p("-1h30"), Some(ms("2026-11-23T08:45")));
        assert_eq!(p("-1h30m"), Some(ms("2026-11-23T08:45")));
        assert_eq!(p("yesterday 18:30"), Some(ms("2026-11-22T18:30")));
        assert_eq!(p("2026-11-20 7:00"), Some(ms("2026-11-20T07:00")));
        for bad in ["", "25:00", "10:60", "-", "-xm", "noon", "10", "tomorrow x"] {
            assert_eq!(p(bad), None, "{bad}");
        }
    }
}
