//! Task scores: how urgent a task is, used to pick the next task.
//!
//! - `(priority + 3) × 100`: priority dominates. Shifted so that scores are
//!   never negative (priorities go down to -3): a low-priority task that
//!   requires another then raises its blocker, instead of pulling it below
//!   itself;
//! - a due date adds `10 + 80 / (1 + days_left / 7)` (90 when due today or
//!   overdue, ~50 one week ahead, ~25 a month ahead, always ≥ 10): at equal
//!   priority a task with a deadline beats one without, and it rises as the
//!   deadline approaches, but never by a full priority step;
//! - a task that blocks others adds the (total) score of every open task that
//!   requires it, so work that unblocks important tasks comes first.

use jiff::civil::Date;
use serde::Serialize;

pub const PRIORITY_WEIGHT: i64 = 100;
pub const DUE_BASE: i64 = 10;
pub const DUE_RANGE: f64 = 80.0;
/// Days over which the due bonus halves.
pub const DUE_HALF_DAYS: f64 = 7.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Score {
    pub priority: i64,
    pub due: i64,
    /// Sum of the scores of the open tasks that require this one.
    pub blocking: i64,
}

impl Score {
    pub fn total(&self) -> i64 {
        self.priority + self.due + self.blocking
    }
}

pub fn due_bonus(due: Date, today: Date) -> i64 {
    let days = (due.duration_since(today).as_secs() / 86_400).max(0) as f64;
    DUE_BASE + (DUE_RANGE / (1.0 + days / DUE_HALF_DAYS)).round() as i64
}

/// The task's own score (without what it blocks).
/// Priorities set locally range from `PRIORITY_MIN` (lowest) to
/// `PRIORITY_MAX`; 0 is the default. (Synced values outside the range are
/// kept as they are.)
pub const PRIORITY_MIN: i64 = -3;
pub const PRIORITY_MAX: i64 = 3;

pub fn own_score(priority: i64, due: Option<crate::due::Due>, today: Date) -> Score {
    Score {
        priority: (priority.max(PRIORITY_MIN) - PRIORITY_MIN) * PRIORITY_WEIGHT,
        due: due.map(|d| due_bonus(d.date, today)).unwrap_or(0),
        blocking: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn due_bonus_grows_as_deadline_nears() {
        let today = date(2026, 11, 20);
        let b = |d| due_bonus(d, today);
        assert_eq!(b(date(2026, 11, 20)), 90);
        assert_eq!(b(date(2026, 11, 1)), 90, "overdue");
        assert_eq!(b(date(2026, 11, 27)), 50);
        assert!(b(date(2026, 11, 21)) > b(date(2026, 11, 27)));
        assert!(b(date(2026, 12, 20)) > b(date(2027, 11, 20)));
        assert!(b(date(2099, 1, 1)) >= DUE_BASE);
        // never a full priority step
        assert!(b(date(2026, 11, 20)) < PRIORITY_WEIGHT);
        let s = own_score(2, Some(crate::due::Due::day(date(2026, 11, 27))), today);
        assert_eq!((s.priority, s.due, s.total()), (500, 50, 550));
        // never negative
        assert_eq!(own_score(PRIORITY_MIN, None, today).total(), 0);
        assert_eq!(own_score(-7, None, today).total(), 0);
        assert_eq!(own_score(0, None, today).total(), 300);
    }
}
