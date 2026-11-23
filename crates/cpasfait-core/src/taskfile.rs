//! A task as a text file, to edit in a text editor (like `task edit`):
//!
//! ```text
//! title: Write the report
//!
//! work log:
//! 2026-11-20 09:00:00 - 2026-11-20 10:30:00
//! 2026-11-23 14:00:00 -                       # running
//! ```
//!
//! [`render`] writes it, [`parse`] reads it back and [`apply`] records the
//! differences: changed title, and sessions added, changed or deleted.

use crate::error::{Error, Result};
use crate::ids::{SessionId, TaskId};
use crate::model::Session;
use crate::op::Change;
use crate::repo::{Edit, Repo};
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

const FMT: &str = "%Y-%m-%d %H:%M:%S";

/// A work session line: Unix ms (whole seconds), no end for a running session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: i64,
    pub end: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskFile {
    pub title: String,
    /// Sessions, with the line they are on (1-based, for error messages).
    pub work: Vec<(usize, Span)>,
}

/// What [`apply`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub title: bool,
    pub added: usize,
    pub changed: usize,
    pub deleted: usize,
}

impl std::fmt::Display for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = vec![];
        if self.title {
            parts.push("title changed".to_owned());
        }
        for (n, what) in [(self.added, "added"), (self.changed, "changed"), (self.deleted, "deleted")] {
            if n > 0 {
                parts.push(format!("{n} session{} {what}", if n > 1 { "s" } else { "" }));
            }
        }
        if parts.is_empty() {
            f.write_str("no changes")
        } else {
            f.write_str(&parts.join(", "))
        }
    }
}

fn fmt_ms(ms: i64, tz: &TimeZone) -> String {
    match jiff::Timestamp::from_millisecond(ms) {
        Ok(t) => t.to_zoned_datetime(tz.clone()).strftime(FMT).to_string(),
        Err(_) => ms.to_string(),
    }
}

/// Whole seconds (what the file shows).
fn secs(ms: i64) -> i64 {
    ms.div_euclid(1000)
}

/// The file for `task`: its title and work log (all users, oldest first).
pub fn render(repo: &Repo, task: TaskId, tz: &TimeZone) -> Result<String> {
    let t = repo.task(task)?;
    let mut sessions: Vec<Session> = repo
        .sessions_for_task(task)?
        .into_iter()
        .filter(|s| s.start.is_some())
        .collect();
    sessions.sort_by_key(|s| s.start);
    let mut out = format!(
        "title: {}\n\n\
         # Work log: one session per line, \"start - end\" in local time ({}).\n\
         # Change a line to correct it, delete it to delete the session, add one\n\
         # to log time. The running session has no end. Seconds are optional.\n\
         # Lines starting with #, and anything after a # on a session line, are\n\
         # ignored. Save an empty file to cancel.\n\
         work log:\n",
        t.title.replace('\n', " "),
        tz.iana_name().unwrap_or("system time zone"),
    );
    for s in &sessions {
        let start = fmt_ms(s.start.unwrap_or_default(), tz);
        let end = s.end.map(|e| fmt_ms(e, tz)).unwrap_or_default();
        let mut notes = vec![];
        if s.end.is_none() {
            notes.push("running".to_owned());
        }
        if s.user != repo.user() {
            notes.push(s.user.clone());
        }
        if !s.note.is_empty() {
            notes.push(s.note.replace('\n', " "));
        }
        let line = format!("{start} - {end}");
        if notes.is_empty() {
            out.push_str(line.trim_end());
        } else {
            out.push_str(&format!("{line:<41}  # {}", notes.join(", ")));
        }
        out.push('\n');
    }
    Ok(out)
}

fn parse_dt(s: &str, tz: &TimeZone) -> Option<i64> {
    let dt: DateTime = s.trim().parse().ok()?;
    Some(dt.to_zoned_datetime(tz.clone()).ok()?.timestamp().as_millisecond())
}

/// Read a file written by [`render`] (and edited). `Ok(None)`: the file is
/// empty (cancel).
pub fn parse(text: &str, tz: &TimeZone) -> Result<Option<TaskFile>> {
    if text.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')) {
        return Ok(None);
    }
    let err = |n: usize, msg: String| Error::Invalid(format!("line {n}: {msg}"));
    let mut title = None;
    let mut in_log = false;
    let mut work = vec![];
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(t) = trimmed.strip_prefix("title:") {
            if title.is_some() {
                return Err(err(n, "a second title".into()));
            }
            let t = t.trim();
            if t.is_empty() {
                return Err(err(n, "empty title".into()));
            }
            title = Some(t.to_owned());
            continue;
        }
        if trimmed.eq_ignore_ascii_case("work log:") {
            in_log = true;
            continue;
        }
        if !in_log {
            return Err(err(n, format!("expected \"title: …\" or \"work log:\", got {trimmed:?}")));
        }
        let span = trimmed.split('#').next().unwrap_or_default().trim();
        let bad = || err(n, format!("expected \"YYYY-MM-DD HH:MM:SS - YYYY-MM-DD HH:MM:SS\", got {span:?}"));
        let (start, end) = match span.strip_suffix('-') {
            Some(start) => (start, ""),
            None => span.split_once(" - ").ok_or_else(bad)?,
        };
        let start = parse_dt(start, tz).ok_or_else(bad)?;
        let end = match end.trim() {
            "" => None,
            e => Some(parse_dt(e, tz).ok_or_else(bad)?),
        };
        if let Some(e) = end
            && e <= start
        {
            return Err(err(n, "the session ends before it starts".into()));
        }
        work.push((n, Span { start, end }));
    }
    let title = title.ok_or_else(|| Error::Invalid("no \"title: …\" line".into()))?;
    Ok(Some(TaskFile { title, work }))
}

/// Record the differences between `task` and `file`, all at once (nothing
/// is changed on error). Lines identical to an existing session leave it
/// alone; changed lines update the remaining sessions in time order (keeping
/// their user and note); extra lines are new sessions of this device; missing
/// ones are deleted. Only the running session can stay without an end.
pub fn apply(repo: &mut Repo, task: TaskId, file: &TaskFile) -> Result<Applied> {
    let t = repo.task(task)?;
    let now = repo.now_ms();
    let mut done = Applied::default();
    let mut edits: Vec<Edit> = vec![];
    if file.title != t.title {
        edits.push((t.tab, task.0, Change::TaskTitle(file.title.clone())));
        done.title = true;
    }

    let mut old: Vec<Session> = repo
        .sessions_for_task(task)?
        .into_iter()
        .filter(|s| s.start.is_some())
        .collect();
    old.sort_by_key(|s| s.start);
    let key = |start: i64, end: Option<i64>| (secs(start), end.map(secs));
    // Unchanged lines.
    let mut new: Vec<(usize, Span)> = vec![];
    for &(n, span) in &file.work {
        match old.iter().position(|s| key(s.start.unwrap_or_default(), s.end) == key(span.start, span.end)) {
            Some(i) => {
                old.remove(i);
            }
            None => new.push((n, span)),
        }
    }
    new.sort_by_key(|(_, s)| s.start);
    for (n, s) in &new {
        if s.start > now || s.end.is_some_and(|e| e > now) {
            return Err(Error::Invalid(format!("line {n}: that is in the future")));
        }
    }
    // Lines without an end continue the running sessions; the others take
    // the remaining sessions in order.
    let mut pairs: Vec<(Session, Span)> = vec![];
    let mut closed = vec![];
    for (n, span) in new {
        if span.end.is_some() {
            closed.push(span);
            continue;
        }
        match old.iter().position(|s| s.end.is_none()) {
            Some(i) => pairs.push((old.remove(i), span)),
            None => {
                return Err(Error::Invalid(format!(
                    "line {n}: only the running session can have no end (start working with s)"
                )));
            }
        }
    }
    let mut old = old.into_iter();
    let mut added = vec![];
    for span in closed {
        match old.next() {
            Some(s) => pairs.push((s, span)),
            None => added.push(span),
        }
    }
    for (s, span) in pairs {
        if s.start.map(secs) != Some(secs(span.start)) {
            edits.push((s.tab, s.id.0, Change::SessionStart(span.start)));
        }
        if s.end.map(secs) != span.end.map(secs) {
            edits.push((s.tab, s.id.0, Change::SessionEnd(span.end)));
        }
        done.changed += 1;
    }
    for s in old {
        edits.push((s.tab, s.id.0, Change::SessionDeleted(true)));
        done.deleted += 1;
    }
    for span in added {
        let id = SessionId::new();
        edits.extend([
            (t.tab, id.0, Change::SessionTask(task)),
            (t.tab, id.0, Change::SessionDevice(repo.device())),
            (t.tab, id.0, Change::SessionUser(repo.user().to_owned())),
            (t.tab, id.0, Change::SessionStart(span.start)),
            (t.tab, id.0, Change::SessionEnd(span.end)),
        ]);
        done.added += 1;
    }
    repo.commit(edits)?;
    Ok(done)
}
