//! Operations: the unit of change, storage and sync.
//!
//! On the wire an op is a generic `{entity, field, value}` triple so that
//! clients can store and forward ops whose field they don't understand
//! (forward compatibility). [`Change`] is the typed interpretation.

use crate::hlc::Hlc;
use crate::ids::{SessionId, TabId, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// A single last-writer-wins assignment `entity.field = value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Op {
    pub hlc: Hlc,
    /// Sequence number per (device, tab): 1, 2, 3, ... without gaps. Names sync segments.
    pub seq: u64,
    pub tab: TabId,
    pub entity: Uuid,
    pub field: String,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Todo,
    Done,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Todo => "todo",
            Status::Done => "done",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Status::Todo),
            "done" => Some(Status::Done),
            _ => None,
        }
    }
}

/// Typed view of an operation's field and value.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    TabName(String),
    TabDeleted(bool),

    TaskTitle(String),
    TaskNotes(String),
    TaskStatus(Status),
    TaskPriority(i64),
    /// Due date as `YYYY-MM-DD`.
    TaskDue(Option<jiff::civil::Date>),
    TaskParent(Option<TaskId>),
    TaskPosition(String),
    TaskTag {
        tag: String,
        on: bool,
    },
    TaskDeleted(bool),
    /// `task` must be done before this one ("blocked by" / "requires").
    TaskBlockedBy {
        task: TaskId,
        on: bool,
    },

    SessionTask(TaskId),
    SessionDevice(crate::ids::DeviceId),
    SessionUser(String),
    /// Unix milliseconds.
    SessionStart(i64),
    SessionEnd(Option<i64>),
    SessionNote(String),
    SessionDeleted(bool),
}

/// Kind of entity an op applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Tab,
    Task,
    Session,
}

const TAG_PREFIX: &str = "task.tag:";
const BLOCKED_BY_PREFIX: &str = "task.blocked_by:";

impl Change {
    pub fn kind(&self) -> EntityKind {
        use Change::*;
        match self {
            TabName(_) | TabDeleted(_) => EntityKind::Tab,
            TaskTitle(_)
            | TaskNotes(_)
            | TaskStatus(_)
            | TaskPriority(_)
            | TaskDue(_)
            | TaskParent(_)
            | TaskPosition(_)
            | TaskTag { .. }
            | TaskBlockedBy { .. }
            | TaskDeleted(_) => EntityKind::Task,
            SessionTask(_) | SessionDevice(_) | SessionUser(_) | SessionStart(_) | SessionEnd(_) | SessionNote(_)
            | SessionDeleted(_) => EntityKind::Session,
        }
    }

    /// The field name (the LWW register key, together with the entity id).
    pub fn field(&self) -> String {
        use Change::*;
        match self {
            TabName(_) => "tab.name".into(),
            TabDeleted(_) => "tab.deleted".into(),
            TaskTitle(_) => "task.title".into(),
            TaskNotes(_) => "task.notes".into(),
            TaskStatus(_) => "task.status".into(),
            TaskPriority(_) => "task.priority".into(),
            TaskDue(_) => "task.due".into(),
            TaskParent(_) => "task.parent".into(),
            TaskPosition(_) => "task.position".into(),
            TaskTag { tag, .. } => format!("{TAG_PREFIX}{tag}"),
            TaskBlockedBy { task, .. } => format!("{BLOCKED_BY_PREFIX}{task}"),
            TaskDeleted(_) => "task.deleted".into(),
            SessionTask(_) => "session.task".into(),
            SessionDevice(_) => "session.device".into(),
            SessionUser(_) => "session.user".into(),
            SessionStart(_) => "session.start".into(),
            SessionEnd(_) => "session.end".into(),
            SessionNote(_) => "session.note".into(),
            SessionDeleted(_) => "session.deleted".into(),
        }
    }

    pub fn value(&self) -> Value {
        use Change::*;
        match self {
            TabName(s) | TaskTitle(s) | TaskNotes(s) | TaskPosition(s) | SessionUser(s) | SessionNote(s) => json!(s),
            TabDeleted(b) | TaskDeleted(b) | SessionDeleted(b) => json!(b),
            TaskTag { on, .. } | TaskBlockedBy { on, .. } => json!(on),
            TaskStatus(s) => json!(s.as_str()),
            TaskPriority(p) => json!(p),
            TaskDue(d) => match d {
                Some(d) => json!(d.to_string()),
                None => Value::Null,
            },
            TaskParent(p) => match p {
                Some(p) => json!(p.0.to_string()),
                None => Value::Null,
            },
            SessionTask(t) => json!(t.0.to_string()),
            SessionDevice(d) => json!(d.0.to_string()),
            SessionStart(t) => json!(t),
            SessionEnd(t) => match t {
                Some(t) => json!(t),
                None => Value::Null,
            },
        }
    }

    /// Interpret a wire `(field, value)`. Returns `None` for unknown fields or
    /// malformed values (such ops are kept but have no effect on this client).
    pub fn parse(field: &str, value: &Value) -> Option<Change> {
        use Change::*;
        let s = || value.as_str().map(str::to_owned);
        let b = || value.as_bool();
        let uuid = || value.as_str().and_then(|s| s.parse::<Uuid>().ok());
        if let Some(tag) = field.strip_prefix(TAG_PREFIX) {
            return Some(TaskTag {
                tag: tag.to_owned(),
                on: b()?,
            });
        }
        if let Some(task) = field.strip_prefix(BLOCKED_BY_PREFIX) {
            return Some(TaskBlockedBy {
                task: task.parse().ok()?,
                on: b()?,
            });
        }
        Some(match field {
            "tab.name" => TabName(s()?),
            "tab.deleted" => TabDeleted(b()?),
            "task.title" => TaskTitle(s()?),
            "task.notes" => TaskNotes(s()?),
            "task.status" => TaskStatus(Status::parse(value.as_str()?)?),
            "task.priority" => TaskPriority(value.as_i64()?),
            "task.due" => TaskDue(if value.is_null() {
                None
            } else {
                Some(value.as_str()?.parse().ok()?)
            }),
            "task.parent" => TaskParent(if value.is_null() { None } else { Some(TaskId(uuid()?)) }),
            "task.position" => TaskPosition(s()?),
            "task.deleted" => TaskDeleted(b()?),
            "session.task" => SessionTask(TaskId(uuid()?)),
            "session.device" => SessionDevice(crate::ids::DeviceId(uuid()?)),
            "session.user" => SessionUser(s()?),
            "session.start" => SessionStart(value.as_i64()?),
            "session.end" => SessionEnd(if value.is_null() { None } else { Some(value.as_i64()?) }),
            "session.note" => SessionNote(s()?),
            "session.deleted" => SessionDeleted(b()?),
            _ => return None,
        })
    }
}

/// Something an op can target.
pub trait Entity {
    fn uuid(&self) -> Uuid;
}
impl Entity for TabId {
    fn uuid(&self) -> Uuid {
        self.0
    }
}
impl Entity for TaskId {
    fn uuid(&self) -> Uuid {
        self.0
    }
}
impl Entity for SessionId {
    fn uuid(&self) -> Uuid {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let changes = vec![
            Change::TabName("x".into()),
            Change::TaskTag {
                tag: "work".into(),
                on: true,
            },
            Change::TaskDue(Some(jiff::civil::date(2026, 12, 1))),
            Change::TaskDue(None),
            Change::TaskBlockedBy {
                task: TaskId::new(),
                on: false,
            },
            Change::TaskParent(Some(TaskId::new())),
            Change::TaskParent(None),
            Change::TaskStatus(Status::Done),
            Change::SessionEnd(Some(42)),
            Change::SessionEnd(None),
        ];
        for c in changes {
            assert_eq!(Change::parse(&c.field(), &c.value()), Some(c));
        }
        assert_eq!(Change::parse("task.future_field", &json!(1)), None);
    }
}
