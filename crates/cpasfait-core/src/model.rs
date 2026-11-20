//! Materialized (read-model) types.

use crate::ids::{DeviceId, SessionId, TabId, TaskId};
use crate::op::Status;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RemoteConfig {
    /// A directory, possibly synced by an external tool (Syncthing, Drive desktop, ...).
    LocalDir { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TabInfo {
    pub id: TabId,
    pub name: String,
    pub remote: Option<RemoteConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: TaskId,
    pub tab: TabId,
    pub title: String,
    pub notes: String,
    pub status: Status,
    pub priority: i64,
    pub due: Option<jiff::civil::Date>,
    pub parent: Option<TaskId>,
    pub position: String,
    pub deleted: bool,
    pub tags: Vec<String>,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.status == Status::Done
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Session {
    pub id: SessionId,
    pub tab: TabId,
    pub task: Option<TaskId>,
    pub device: Option<DeviceId>,
    pub user: String,
    /// Unix milliseconds.
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub note: String,
    pub deleted: bool,
}

impl Session {
    pub fn is_open(&self) -> bool {
        self.end.is_none()
    }
    /// Duration in milliseconds, open sessions counting until `now`.
    pub fn duration_ms(&self, now: i64) -> i64 {
        match self.start {
            Some(s) => (self.end.unwrap_or(now) - s).max(0),
            None => 0,
        }
    }
}
