//! A task as a text file, to edit in a text editor (like `task edit`):
//!
//! ```text
//! title: Write the report
//! status: todo
//! priority: 1
//! # due: 2026-11-27 19:22:30
//! tags: work
//! # requires: TASK-ID
//!
//! # (explanations)
//!
//! description:
//! Free text, kept as is.
//!
//! notes:
//! @@ 1 · 2026-11-27 14:30 · alice
//! Sent the draft to Bob.
//!
//! work log:
//! 2026-11-20 09:00:00 - 2026-11-20 10:30:00
//! 2026-11-23 14:00:00 -                       # running
//! ```
//!
//! Optional fields that are unset are shown commented out (with an example
//! value): uncommenting sets them, commenting out or deleting a field line
//! unsets it. A section (description, notes, work log) left out entirely
//! is left alone. In the description and the notes, `#` lines are text;
//! text lines that would look like a section or note header get a `\` in
//! front, removed when reading back.
//!
//! [`render`] writes it, [`parse`] reads it back and [`apply`] records the
//! differences, all at once.

use crate::error::{Error, Result};
use crate::ids::{NoteId, SessionId, TaskId};
use crate::model::Session;
use crate::op::{Change, Status};
use crate::repo::{Edit, Repo};
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use std::collections::BTreeSet;

const FMT: &str = "%Y-%m-%d %H:%M:%S";
const DESCRIPTION: &str = "description:";
const NOTES: &str = "notes:";
const WORK_LOG: &str = "work log:";
const NOTE_HEADER: &str = "@@";

/// A work session line: Unix ms (whole seconds), no end for a running session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: i64,
    pub end: Option<i64>,
}

/// A note block: `number` refers to the notes as rendered (1-based, newest
/// first), none for a new note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteBlock {
    pub line: usize,
    pub number: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskFile {
    pub title: String,
    pub status: Status,
    pub priority: i64,
    pub due: Option<crate::due::Due>,
    pub tags: BTreeSet<String>,
    /// With the line they are on.
    pub requires: Vec<(usize, TaskId)>,
    /// None: no description section (left alone).
    pub description: Option<String>,
    /// None: no notes section (left alone).
    pub notes: Option<Vec<NoteBlock>>,
    /// Sessions, with the line they are on (1-based, for error messages).
    /// None: no work log section (left alone).
    pub work: Option<Vec<(usize, Span)>>,
}

/// What [`apply`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// The fields changed: title, status, priority, due, tags, requires, description.
    pub fields: Vec<&'static str>,
    pub notes_added: usize,
    pub notes_changed: usize,
    pub notes_deleted: usize,
    /// Lines merged into another one because they overlapped.
    pub merged: usize,
    pub added: usize,
    pub changed: usize,
    pub deleted: usize,
}

impl std::fmt::Display for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = vec![];
        if !self.fields.is_empty() {
            parts.push(format!("{} changed", self.fields.join(", ")));
        }
        let plural = |n: usize, what: &str| format!("{n} {what}{}", if n > 1 { "s" } else { "" });
        for (n, what) in [
            (self.notes_added, "added"),
            (self.notes_changed, "changed"),
            (self.notes_deleted, "deleted"),
        ] {
            if n > 0 {
                parts.push(format!("{} {what}", plural(n, "note")));
            }
        }
        for (n, what) in [
            (self.merged, "merged (overlapping)"),
            (self.added, "added"),
            (self.changed, "changed"),
            (self.deleted, "deleted"),
        ] {
            if n > 0 {
                parts.push(format!("{} {what}", plural(n, "session")));
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

/// Whether `s` is in the file for a window starting at `since`: it has a
/// start, and is running or ends after `since`.
fn in_window(s: &Session, since: Option<i64>) -> bool {
    s.start.is_some() && s.end.is_none_or(|e| since.is_none_or(|since| e > since))
}

/// `task`'s sessions in the window, oldest first.
fn window(repo: &Repo, task: TaskId, since: Option<i64>) -> Result<Vec<Session>> {
    let mut v: Vec<Session> = repo
        .sessions_for_task(task)?
        .into_iter()
        .filter(|s| in_window(s, since))
        .collect();
    v.sort_by_key(|s| s.start);
    Ok(v)
}

/// Whether a line of free text would be read as a header (possibly with
/// `\\`s in front already): it gets one more `\\`.
fn needs_escape(line: &str) -> bool {
    let l = line.trim_start_matches('\\');
    l.starts_with(NOTE_HEADER) || [DESCRIPTION, NOTES, WORK_LOG].contains(&l.trim_end())
}

fn push_text(out: &mut String, text: &str) {
    for line in text.lines() {
        if needs_escape(line) {
            out.push('\\');
        }
        out.push_str(line);
        out.push('\n');
    }
}

/// The file for `task`: its fields, description, notes (newest first) and
/// work log (all users, oldest first). With `since` (Unix ms), only the
/// sessions running or ending after it: the older ones are left out, and
/// [`apply`] (given the same `since`) leaves them alone.
pub fn render(repo: &Repo, task: TaskId, tz: &TimeZone, since: Option<i64>) -> Result<String> {
    let t = repo.task(task)?;
    let sessions = window(repo, task, since)?;
    let hidden = repo.sessions_for_task(task)?.len() - sessions.len();
    let mut out = format!("title: {}\nstatus: {}\n", t.title.replace('\n', " "), t.status.as_str());
    let opt = |set: bool| if set { "" } else { "# " };
    out.push_str(&format!("{}priority: {}\n", opt(t.priority != 0), t.priority));
    let due = match t.due {
        Some(d) => d.to_string(),
        None => fmt_ms(repo.now_ms(), tz),
    };
    out.push_str(&format!("{}due: {due}\n", opt(t.due.is_some())));
    if t.tags.is_empty() {
        out.push_str("# tags: TAG OTHER-TAG\n");
    } else {
        out.push_str(&format!("tags: {}\n", t.tags.join(" ")));
    }
    if t.blocked_by.is_empty() {
        out.push_str("# requires: TASK-ID\n");
    }
    for b in &t.blocked_by {
        match repo.task(*b) {
            Ok(b) => out.push_str(&format!("requires: {}  # {}\n", b.id, b.title.replace('\n', " "))),
            Err(_) => out.push_str(&format!("requires: {b}  # unknown task\n")),
        }
    }
    out.push_str(&format!(
        "\n\
         # Lines starting with # are ignored, except in the description and the notes.\n\
         # Uncomment an optional field to set it, comment it out to unset it.\n\
         # status: todo or done. priority: {} to {}. due: a date, optionally a time\n\
         # (also: tomorrow, fri 9:00, ...). tags: separated by spaces. requires: a\n\
         # task id per line (:require picks one by name). Save an empty file to cancel.\n\
         #\n\
         # Notes, newest first: change a note to correct it, delete its block (from its\n\
         # @@ line) to delete it, add one with a line \"@@\" followed by its text.\n\
         #\n\
         # Work log: one session per line, \"start - end\" in local time ({}).\n\
         # Change a line to correct it, delete it to delete the session, add one\n\
         # to log time. The running session has no end. Seconds are optional.\n\
         # Overlapping sessions are merged into one. Anything after a # on a\n\
         # session line is ignored.\n\
         \n{DESCRIPTION}\n",
        crate::score::PRIORITY_MIN,
        crate::score::PRIORITY_MAX,
        tz.iana_name().unwrap_or("system time zone"),
    ));
    push_text(&mut out, &t.notes);
    out.push_str(&format!("\n{NOTES}\n"));
    for (i, n) in repo.notes_for_task(task)?.iter().enumerate() {
        let at = match jiff::Timestamp::from_millisecond(n.at) {
            Ok(at) => at.to_zoned_datetime(tz.clone()).strftime("%Y-%m-%d %H:%M").to_string(),
            Err(_) => n.at.to_string(),
        };
        out.push_str(&format!("{NOTE_HEADER} {} · {at} · {}\n", i + 1, n.author.name));
        push_text(&mut out, &n.text);
        out.push('\n');
    }
    out.push_str(&format!("{WORK_LOG}\n"));
    if let Some(since) = since
        && hidden > 0
    {
        out.push_str(&format!(
            "# Not shown (and left alone): {hidden} session{} ended before {}. :edit all\n\
             # shows everything, :edit DATE from that date.\n",
            if hidden > 1 { "s" } else { "" },
            fmt_ms(since, tz)
        ));
    }
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

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Fields,
    Description,
    Notes,
    WorkLog,
}

/// Free text read back: unescaped, without the blank lines around it.
fn finish_text(lines: &[&str]) -> String {
    let lines: Vec<&str> = lines
        .iter()
        .map(|l| {
            if needs_escape(l) && l.starts_with('\\') {
                &l[1..]
            } else {
                l
            }
        })
        .collect();
    lines.join("\n").trim_matches('\n').trim_end().to_owned()
}

/// Read a file written by [`render`] (and edited). `Ok(None)`: the file is
/// empty (cancel). `today` is for relative due dates.
pub fn parse(text: &str, tz: &TimeZone, today: Date) -> Result<Option<TaskFile>> {
    if text
        .lines()
        .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
    {
        return Ok(None);
    }
    let err = |n: usize, msg: String| Error::Invalid(format!("line {n}: {msg}"));
    let mut title = None;
    let mut status = Status::Todo;
    let mut priority = 0;
    let mut due = None;
    let mut tags = BTreeSet::new();
    let mut requires = vec![];
    let mut description: Option<Vec<&str>> = None;
    // (line, number, text lines)
    type Block<'a> = (usize, Option<usize>, Vec<&'a str>);
    let mut notes: Option<Vec<Block>> = None;
    let mut work: Option<Vec<(usize, Span)>> = None;
    let mut seen = BTreeSet::new();
    let mut section = Section::Fields;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let trimmed = line.trim();
        let header = match trimmed {
            DESCRIPTION => Some(Section::Description),
            NOTES => Some(Section::Notes),
            WORK_LOG => Some(Section::WorkLog),
            _ => None,
        };
        if let Some(h) = header {
            let again = match h {
                Section::Description => description.replace(vec![]).is_some(),
                Section::Notes => notes.replace(vec![]).is_some(),
                Section::WorkLog => work.replace(vec![]).is_some(),
                Section::Fields => false,
            };
            if again {
                return Err(err(n, format!("a second \"{trimmed}\"")));
            }
            section = h;
            continue;
        }
        match section {
            Section::Description => {
                if let Some(d) = &mut description {
                    d.push(line);
                }
                continue;
            }
            Section::Notes => {
                let blocks = notes.get_or_insert_with(Vec::new);
                if let Some(rest) = line.strip_prefix(NOTE_HEADER) {
                    let num: String = rest.trim_start().chars().take_while(char::is_ascii_digit).collect();
                    let number = if num.is_empty() {
                        if !rest.trim().is_empty() && rest.trim() != "new" {
                            return Err(err(
                                n,
                                "a note header is \"@@ NUMBER ...\", or \"@@\" for a new note".into(),
                            ));
                        }
                        None
                    } else {
                        let k: usize = num.parse().map_err(|_| err(n, "bad note number".into()))?;
                        if blocks.iter().any(|b| b.1 == Some(k)) {
                            return Err(err(n, format!("note {k} appears twice")));
                        }
                        Some(k)
                    };
                    blocks.push((n, number, vec![]));
                } else if let Some(b) = blocks.last_mut() {
                    b.2.push(line);
                } else if !(trimmed.is_empty() || trimmed.starts_with('#')) {
                    return Err(err(n, "expected a note header (\"@@\" for a new note)".into()));
                }
                continue;
            }
            _ => {}
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if section == Section::WorkLog {
            let span = trimmed.split('#').next().unwrap_or_default().trim();
            let bad = || {
                err(
                    n,
                    format!("expected \"YYYY-MM-DD HH:MM:SS - YYYY-MM-DD HH:MM:SS\", got {span:?}"),
                )
            };
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
            if let Some(w) = &mut work {
                w.push((n, Span { start, end }));
            }
            continue;
        }
        // A field.
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(err(
                n,
                format!("expected \"field: value\" or a section, got {trimmed:?}"),
            ));
        };
        let name = name.trim().to_lowercase();
        let value = value.trim();
        if name != "requires" && !seen.insert(name.clone()) {
            return Err(err(n, format!("a second {name}")));
        }
        match name.as_str() {
            "title" => {
                if value.is_empty() {
                    return Err(err(n, "empty title".into()));
                }
                title = Some(value.to_owned());
            }
            "status" => {
                status = Status::parse(&value.to_lowercase())
                    .ok_or_else(|| err(n, format!("status is todo or done, not {value:?}")))?;
            }
            "priority" => {
                priority = value
                    .parse::<i64>()
                    .ok()
                    .filter(|p| (crate::score::PRIORITY_MIN..=crate::score::PRIORITY_MAX).contains(p))
                    .ok_or_else(|| {
                        err(
                            n,
                            format!(
                                "priority is a number from {} to {}, not {value:?}",
                                crate::score::PRIORITY_MIN,
                                crate::score::PRIORITY_MAX
                            ),
                        )
                    })?;
            }
            "due" => {
                if !value.is_empty() {
                    due = Some(
                        crate::dates::parse_due(value, today)
                            .ok_or_else(|| err(n, format!("can't read the due date {value:?}")))?,
                    );
                }
            }
            "tags" => {
                tags = value
                    .split_whitespace()
                    .map(|t| t.trim_start_matches(['+', '#']))
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "requires" => {
                let id = value.split_whitespace().next().unwrap_or_default();
                let id: TaskId = id
                    .parse()
                    .map_err(|_| err(n, format!("expected a task id, got {id:?}")))?;
                requires.push((n, id));
            }
            _ => {
                return Err(err(
                    n,
                    format!("unknown field {name:?} (title, status, priority, due, tags, requires)"),
                ));
            }
        }
    }
    let title = title.ok_or_else(|| Error::Invalid("no \"title: …\" line".into()))?;
    Ok(Some(TaskFile {
        title,
        status,
        priority,
        due,
        tags,
        requires,
        description: description.map(|d| finish_text(&d)),
        notes: notes.map(|blocks| {
            blocks
                .into_iter()
                .map(|(line, number, text)| NoteBlock {
                    line,
                    number,
                    text: finish_text(&text),
                })
                .collect()
        }),
        work,
    }))
}

/// The sessions sorted, overlapping ones merged into one (touching ones stay
/// apart: stopping and restarting makes those). A span without an end runs
/// until now, so it absorbs everything after its start. Each span keeps the
/// line number of its first line.
pub fn merge(work: &[(usize, Span)]) -> Vec<(usize, Span)> {
    let mut sorted = work.to_vec();
    sorted.sort_by_key(|(_, s)| s.start);
    let mut out: Vec<(usize, Span)> = vec![];
    for (n, span) in sorted {
        match out.last_mut() {
            Some((_, last)) if last.end.is_none_or(|e| span.start < e) => {
                last.end = match (last.end, span.end) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
            }
            _ => out.push((n, span)),
        }
    }
    out
}

/// Record the differences between `task` and `file`, all at once (nothing
/// is changed on error).
///
/// Notes are matched by their number: changed text updates the note, an
/// emptied or missing block deletes it, blocks without a number are new
/// notes (by this user, now).
///
/// Only the sessions [`render`] showed for `since` are compared with the
/// lines: older ones stay as they are. Overlapping lines are first merged
/// (see [`merge`]). Lines identical to an existing session leave it alone;
/// changed lines update the remaining sessions in time order (keeping their
/// user and note); extra lines are new sessions of this device; missing ones
/// are deleted. Only the running session can stay without an end.
pub fn apply(repo: &mut Repo, task: TaskId, file: &TaskFile, since: Option<i64>) -> Result<Applied> {
    let t = repo.task(task)?;
    let now = repo.now_ms();
    let mut done = Applied::default();
    let mut edits: Vec<Edit> = vec![];
    let mut field = |done: &mut Applied, name: &'static str, change: Change| {
        edits.push((t.tab, task.0, change));
        if !done.fields.contains(&name) {
            done.fields.push(name);
        }
    };
    if file.title != t.title {
        field(&mut done, "title", Change::TaskTitle(file.title.clone()));
    }
    if file.status != t.status {
        field(&mut done, "status", Change::TaskStatus(file.status));
    }
    if file.priority != t.priority {
        field(&mut done, "priority", Change::TaskPriority(file.priority));
    }
    if file.due != t.due {
        field(&mut done, "due", Change::TaskDue(file.due));
    }
    let old_tags: BTreeSet<String> = t.tags.iter().cloned().collect();
    for tag in old_tags.symmetric_difference(&file.tags) {
        let on = file.tags.contains(tag);
        field(&mut done, "tags", Change::TaskTag { tag: tag.clone(), on });
    }
    if let Some(d) = &file.description
        && *d != t.notes
    {
        field(&mut done, "description", Change::TaskNotes(d.clone()));
    }
    let required: BTreeSet<TaskId> = file.requires.iter().map(|(_, id)| *id).collect();
    for (n, id) in &file.requires {
        if !t.blocked_by.contains(id) {
            let edit = repo
                .blocked_by_edit(task, *id, true)
                .map_err(|e| Error::Invalid(format!("line {n}: {}", message(e))))?;
            edits.push(edit);
            if !done.fields.contains(&"requires") {
                done.fields.push("requires");
            }
        }
    }
    for id in &t.blocked_by {
        if !required.contains(id) {
            edits.push(repo.blocked_by_edit(task, *id, false)?);
            if !done.fields.contains(&"requires") {
                done.fields.push("requires");
            }
        }
    }

    if let Some(blocks) = &file.notes {
        let notes = repo.notes_for_task(task)?;
        let mut kept = BTreeSet::new();
        for b in blocks {
            match b.number {
                Some(k) => {
                    let note = notes.get(k.wrapping_sub(1)).ok_or_else(|| {
                        Error::Invalid(format!("line {}: there is no note {k} (\"@@\" adds one)", b.line))
                    })?;
                    kept.insert(k);
                    if b.text.is_empty() {
                        edits.push((note.tab, note.id.0, Change::NoteDeleted(true)));
                        done.notes_deleted += 1;
                    } else if b.text != note.text {
                        edits.push((note.tab, note.id.0, Change::NoteText(b.text.clone())));
                        done.notes_changed += 1;
                    }
                }
                None if b.text.is_empty() => {}
                None => {
                    let id = NoteId::new();
                    edits.extend([
                        (t.tab, id.0, Change::NoteTask(task)),
                        (t.tab, id.0, Change::NoteAt(now)),
                        (t.tab, id.0, Change::NoteAuthor(repo.author())),
                        (t.tab, id.0, Change::NoteText(b.text.clone())),
                    ]);
                    done.notes_added += 1;
                }
            }
        }
        for (i, note) in notes.iter().enumerate() {
            if !kept.contains(&(i + 1)) {
                edits.push((note.tab, note.id.0, Change::NoteDeleted(true)));
                done.notes_deleted += 1;
            }
        }
    }

    let Some(file_work) = &file.work else {
        repo.commit(edits)?;
        return Ok(done);
    };
    let mut old = window(repo, task, since)?;
    let key = |start: i64, end: Option<i64>| (secs(start), end.map(secs));
    let work = merge(file_work);
    done.merged = file_work.len() - work.len();
    // Unchanged lines.
    let mut new: Vec<(usize, Span)> = vec![];
    for &(n, span) in &work {
        match old
            .iter()
            .position(|s| key(s.start.unwrap_or_default(), s.end) == key(span.start, span.end))
        {
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

fn message(e: Error) -> String {
    match e {
        Error::Invalid(m) => m,
        e => e.to_string(),
    }
}
