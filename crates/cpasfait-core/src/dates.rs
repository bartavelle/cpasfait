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
}
