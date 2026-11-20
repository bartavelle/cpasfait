//! The "time spent" box: time logged per task of the current tab over a
//! period (today, this week, this month), largest first.

use cpasfait_core::report::{Filter, slices};
use cpasfait_core::{Repo, TabId, TaskId, Tree};
use jiff::civil::Weekday;
use jiff::tz::TimeZone;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimePeriod {
    #[default]
    Hidden,
    Today,
    Week,
    Month,
}

impl TimePeriod {
    /// The next state of the toggle key.
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Today,
            Self::Today => Self::Week,
            Self::Week => Self::Month,
            Self::Month => Self::Hidden,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Today => "today",
            Self::Week => "this week",
            Self::Month => "this month",
        }
    }

    /// Start of the period containing `now` (local midnight; weeks start on Monday).
    pub fn start(self, now_ms: i64, tz: &TimeZone) -> Option<i64> {
        let date = jiff::Timestamp::from_millisecond(now_ms)
            .ok()?
            .to_zoned_datetime(tz.clone())
            .date();
        let day = match self {
            Self::Hidden => return None,
            Self::Today => date,
            Self::Week => {
                let mut d = date;
                while d.weekday() != Weekday::Monday {
                    d = d.yesterday().ok()?;
                }
                d
            }
            Self::Month => date.first_of_month(),
        };
        Some(day.to_zoned_datetime(tz.clone()).ok()?.timestamp().as_millisecond())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeLine {
    pub task: Option<TaskId>,
    pub title: String,
    pub ms: i64,
}

/// Time logged directly on each task of `tab` (all users) since the start of
/// `period`, largest first. Parents are not credited with their subtasks'
/// time, so the lines add up to the total.
pub fn compute(repo: &Repo, tree: &Tree, tab: TabId, period: TimePeriod, tz: &TimeZone) -> Vec<TimeLine> {
    let now = repo.now_ms();
    let Some(from) = period.start(now, tz) else {
        return vec![];
    };
    let filter = Filter {
        tab: Some(tab),
        user: None,
    };
    let Ok(slices) = slices(repo, from, now, &filter) else {
        return vec![];
    };
    let mut per_task: HashMap<Option<TaskId>, i64> = HashMap::new();
    for s in &slices {
        *per_task.entry(s.task).or_default() += s.ms();
    }
    let mut lines: Vec<TimeLine> = per_task
        .into_iter()
        .map(|(task, ms)| {
            let title = match task {
                None => "(no task)".to_owned(),
                Some(id) => match tree.get(id) {
                    Some(t) if t.title.is_empty() => "(untitled)".to_owned(),
                    Some(t) => t.title.clone(),
                    // Deleted, or hidden under a deleted parent.
                    None => match repo.task(id) {
                        Ok(t) => format!("{} (deleted)", t.title),
                        Err(_) => "(unknown task)".to_owned(),
                    },
                },
            };
            TimeLine { task, title, ms }
        })
        .collect();
    lines.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.title.cmp(&b.title)));
    lines
}

/// `45s`, `12m`, `1h 05m`, `2d 3h` (calendar days of 24h).
pub fn fmt_human(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (d, h, m) = (s / 86_400, (s / 3600) % 24, (s / 60) % 60);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{m}m")
    } else if d == 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{d}d {h}h")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_durations() {
        assert_eq!(fmt_human(0), "0s");
        assert_eq!(fmt_human(45_000), "45s");
        assert_eq!(fmt_human(12 * 60_000 + 59_000), "12m");
        assert_eq!(fmt_human(65 * 60_000), "1h 05m");
        assert_eq!(fmt_human(27 * 3_600_000), "1d 3h");
    }

    #[test]
    fn period_starts() {
        let tz = TimeZone::UTC;
        // Thursday 2026-11-19 15:30 UTC
        let now = jiff::civil::date(2026, 11, 19)
            .at(15, 30, 0, 0)
            .to_zoned_datetime(tz.clone())
            .unwrap()
            .timestamp()
            .as_millisecond();
        let day = |d: i8| {
            jiff::civil::date(2026, 11, d)
                .to_zoned_datetime(tz.clone())
                .unwrap()
                .timestamp()
                .as_millisecond()
        };
        assert_eq!(TimePeriod::Today.start(now, &tz), Some(day(19)));
        assert_eq!(TimePeriod::Week.start(now, &tz), Some(day(16)));
        assert_eq!(TimePeriod::Month.start(now, &tz), Some(day(1)));
        assert_eq!(TimePeriod::Hidden.start(now, &tz), None);
        let mut p = TimePeriod::Hidden;
        let mut seen = vec![];
        for _ in 0..4 {
            p = p.next();
            seen.push(p);
        }
        assert_eq!(
            seen,
            [
                TimePeriod::Today,
                TimePeriod::Week,
                TimePeriod::Month,
                TimePeriod::Hidden
            ]
        );
    }
}
