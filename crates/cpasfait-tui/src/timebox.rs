//! The "time spent" box: time logged per task, in all tabs, over a period
//! (today, this week, this month), largest first.

use cpasfait_core::report::{Filter, slices};
use cpasfait_core::{Repo, TabId, TabInfo, TaskId, Tree};
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

/// Whether `task` is deleted, or under a deleted parent.
fn is_deleted(repo: &Repo, task: TaskId) -> bool {
    let mut id = Some(task);
    // Bounded: parent links may form a cycle before the tree repairs it.
    for _ in 0..1000 {
        let Some(t) = id.and_then(|i| repo.task(i).ok()) else {
            return false;
        };
        if t.deleted {
            return true;
        }
        id = t.parent;
    }
    false
}

/// Time logged directly on each task, in all tabs (all users), since the
/// start of `period`, largest first. Deleted tasks (and their subtasks) and
/// closed tabs are left out. Parents are not credited with their subtasks'
/// time, so the lines add up to the total. `tree` is the current
/// tab's (`tab`); tasks of other tabs get their tab's name after the title.
pub fn compute(
    repo: &Repo,
    tree: &Tree,
    tab: TabId,
    tabs: &[TabInfo],
    period: TimePeriod,
    tz: &TimeZone,
) -> Vec<TimeLine> {
    let now = repo.now_ms();
    let Some(from) = period.start(now, tz) else {
        return vec![];
    };
    let filter = Filter::default();
    let Ok(slices) = slices(repo, from, now, &filter) else {
        return vec![];
    };
    let mut per_task: HashMap<(TabId, Option<TaskId>), i64> = HashMap::new();
    for s in &slices {
        let gone = !tabs.iter().any(|t| t.id == s.tab)
            || match s.task {
                Some(id) if s.tab == tab => tree.get(id).is_none() && is_deleted(repo, id),
                Some(id) => is_deleted(repo, id),
                None => false,
            };
        if gone {
            continue;
        }
        *per_task.entry((s.tab, s.task)).or_default() += s.ms();
    }
    let untitled = |t: &str| {
        if t.is_empty() {
            "(untitled)".to_owned()
        } else {
            t.to_owned()
        }
    };
    let mut lines: Vec<TimeLine> = per_task
        .into_iter()
        .map(|((task_tab, task), ms)| {
            let mut title = match task {
                None => "(no task)".to_owned(),
                Some(id) => match tree.get(id).filter(|_| task_tab == tab) {
                    Some(t) => untitled(&t.title),
                    // Another tab's (deleted ones were left out above).
                    None => match repo.task(id) {
                        Ok(t) => untitled(&t.title),
                        Err(_) => "(unknown task)".to_owned(),
                    },
                },
            };
            if task_tab != tab {
                let name = tabs.iter().find(|t| t.id == task_tab).map_or("?", |t| t.name.as_str());
                title = format!("{title} [{name}]");
            }
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
