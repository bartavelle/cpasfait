//! Due dates: a day, optionally with a time of day (local, civil time).

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;

/// When a task is due. Without a time, it is due from the start of the day.
/// Ordered by date, then time (a day without a time comes first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Due {
    pub date: Date,
    pub time: Option<Time>,
}

impl Due {
    pub fn day(date: Date) -> Self {
        Due { date, time: None }
    }

    pub fn at(date: Date, time: Time) -> Self {
        Due { date, time: Some(time) }
    }

    /// The moment it falls due (Unix ms) in `tz`: the time, or midnight.
    pub fn moment_ms(&self, tz: &TimeZone) -> Option<i64> {
        let dt = self.date.to_datetime(self.time.unwrap_or(Time::midnight()));
        Some(dt.to_zoned_datetime(tz.clone()).ok()?.timestamp().as_millisecond())
    }

    /// The stored and wire form: `2026-12-01` or `2026-12-01T14:30:00`.
    pub fn wire(&self) -> String {
        match self.time {
            None => self.date.to_string(),
            Some(t) => format!("{}T{}", self.date, t.strftime("%H:%M:%S")),
        }
    }
}

/// `2026-12-01`, `2026-12-01 14:30`, `2026-12-01 14:30:05` (seconds only
/// when not zero).
impl std::fmt::Display for Due {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.time {
            None => write!(f, "{}", self.date),
            Some(t) if t.second() == 0 => write!(f, "{} {}", self.date, t.strftime("%H:%M")),
            Some(t) => write!(f, "{} {}", self.date, t.strftime("%H:%M:%S")),
        }
    }
}

/// Reads [`Due::wire`] and [`Due`]'s display form.
impl std::str::FromStr for Due {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        // (jiff parses a date-time as a date, dropping the time)
        if !s.contains(':') {
            return s
                .parse::<Date>()
                .map(Due::day)
                .map_err(|_| format!("bad due date {s:?}"));
        }
        match s.parse::<DateTime>() {
            Ok(dt) => {
                let t = dt.time();
                let t = Time::new(t.hour(), t.minute(), t.second(), 0).map_err(|e| e.to_string())?;
                Ok(Due::at(dt.date(), t))
            }
            Err(_) => Err(format!("bad due date {s:?}")),
        }
    }
}

impl serde::Serialize for Due {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.wire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::{date, time};

    #[test]
    fn forms() {
        let d = Due::day(date(2026, 12, 1));
        let t = Due::at(date(2026, 12, 1), time(14, 30, 0, 0));
        let s = Due::at(date(2026, 12, 1), time(14, 30, 5, 0));
        assert_eq!((d.wire(), d.to_string()), ("2026-12-01".into(), "2026-12-01".into()));
        assert_eq!(t.wire(), "2026-12-01T14:30:00");
        assert_eq!(t.to_string(), "2026-12-01 14:30");
        assert_eq!(s.to_string(), "2026-12-01 14:30:05");
        for x in [d, t, s] {
            assert_eq!(x.wire().parse::<Due>(), Ok(x));
            assert_eq!(x.to_string().parse::<Due>(), Ok(x));
        }
        assert!(d < t && t < s && s < Due::day(date(2026, 12, 2)));
        let tz = TimeZone::UTC;
        assert_eq!(
            t.moment_ms(&tz).unwrap() - d.moment_ms(&tz).unwrap(),
            (14 * 60 + 30) * 60_000
        );
        assert!("2026-13-01".parse::<Due>().is_err());
    }
}
