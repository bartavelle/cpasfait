//! Materialized (read-model) types.

use crate::ids::{DeviceId, SessionId, TabId, TaskId};
use crate::op::Status;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RemoteConfig {
    /// A directory, possibly synced by an external tool (Syncthing, Drive desktop, ...).
    LocalDir { path: String },
    /// A Google Drive folder (by id), possibly shared with other users.
    GDrive { folder: String },
    /// A directory reached with rsync over SSH: `[user@]host:path`.
    Rsync { target: String },
}

impl RemoteConfig {
    /// Parse a user-supplied remote: `gdrive:FOLDER-ID` (or a Drive folder URL),
    /// `rsync:[user@]host:path`, otherwise a directory path (`~/` is expanded).
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if let Some(target) = spec.strip_prefix("rsync:") {
            let target = target.trim();
            let ok = match target.split_once(':') {
                Some((host, path)) => {
                    !host.is_empty() && !path.is_empty() && !host.contains('/') && !host.starts_with('-')
                }
                None => false,
            };
            if !ok {
                return Err(format!("expected rsync:[user@]host:path, got rsync:{target}"));
            }
            return Ok(Self::Rsync {
                target: target.to_owned(),
            });
        }
        if let Some(id) = spec.strip_prefix("gdrive:") {
            let id = id.trim();
            let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !valid {
                return Err(format!("bad Google Drive folder id {id:?}"));
            }
            return Ok(Self::GDrive { folder: id.to_owned() });
        }
        if let Some(rest) = spec.strip_prefix("https://drive.google.com/") {
            // https://drive.google.com/drive/folders/<id>?usp=sharing, .../drive/u/0/folders/<id>
            let id = rest
                .split("/folders/")
                .nth(1)
                .and_then(|r| r.split(['?', '/', '#']).next());
            return match id {
                Some(id) => Self::parse(&format!("gdrive:{id}")),
                None => Err("not a Google Drive folder URL".into()),
            };
        }
        if spec.is_empty() {
            return Err("empty remote".into());
        }
        let path = match spec.strip_prefix("~/") {
            Some(rest) => match std::env::var_os("HOME") {
                Some(home) => std::path::Path::new(&home).join(rest).to_string_lossy().into_owned(),
                None => spec.to_owned(),
            },
            None => spec.to_owned(),
        };
        Ok(Self::LocalDir { path })
    }
}

impl RemoteConfig {
    /// The spec this remote was parsed from (round-trips through [`RemoteConfig::parse`]).
    pub fn spec(&self) -> String {
        match self {
            Self::LocalDir { path } => path.clone(),
            Self::GDrive { folder } => format!("gdrive:{folder}"),
            Self::Rsync { target } => format!("rsync:{target}"),
        }
    }
}

impl std::fmt::Display for RemoteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalDir { path } => write!(f, "directory {path}"),
            Self::GDrive { folder } => write!(f, "gdrive:{folder}"),
            Self::Rsync { target } => write!(f, "rsync:{target}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TabInfo {
    pub id: TabId,
    pub name: String,
    pub remote: Option<RemoteConfig>,
    /// A move to another remote, made elsewhere, that this device has not
    /// followed yet.
    pub pending_move: Option<RemoteMove>,
}

/// "This tab now lives at `remote`" (a spec as accepted by [`RemoteConfig::parse`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RemoteMove {
    pub id: uuid::Uuid,
    pub remote: String,
    /// Who moved it.
    pub by: String,
}

/// A member of a shared tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Member {
    /// Public key (age `age1…` or SSH).
    pub recipient: String,
    pub name: String,
    /// False once removed.
    pub active: bool,
    /// When the membership was last changed.
    pub hlc: crate::hlc::Hlc,
}

/// A generation of a shared tab's encryption key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epoch {
    pub id: uuid::Uuid,
    pub key: crate::crypto::Key,
    /// The epoch this one replaces (`None` for the first one).
    pub prev: Option<uuid::Uuid>,
    /// Length of the chain from the first epoch.
    pub depth: u32,
}

/// State of the other end of a dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DepState {
    Todo,
    Done,
    /// Deleted (or under a deleted parent): no longer blocks.
    Deleted,
    /// In a tab this device has, but not here (not synced yet, or never existed).
    Missing,
    /// In a tab this device does not have.
    UnknownTab,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DepInfo {
    pub task: TaskId,
    pub tab: Option<TabId>,
    pub tab_name: Option<String>,
    /// `None` when the task is not known here.
    pub title: Option<String>,
    pub state: DepState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Dependencies {
    /// Tasks this one requires.
    pub requires: Vec<DepInfo>,
    /// Tasks that require this one (only those known here).
    pub blocks: Vec<DepInfo>,
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
    /// Tasks that must be done before this one (may reference deleted or
    /// unknown tasks: those are ignored).
    pub blocked_by: Vec<TaskId>,
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

#[cfg(test)]
mod tests {
    use super::RemoteConfig;

    #[test]
    fn parse_remote() {
        let g = |f: &str| RemoteConfig::GDrive { folder: f.into() };
        assert_eq!(RemoteConfig::parse("gdrive:1AbC-d_9"), Ok(g("1AbC-d_9")));
        assert_eq!(
            RemoteConfig::parse("https://drive.google.com/drive/folders/1AbC?usp=sharing"),
            Ok(g("1AbC"))
        );
        assert_eq!(
            RemoteConfig::parse("https://drive.google.com/drive/u/0/folders/XyZ"),
            Ok(g("XyZ"))
        );
        assert!(RemoteConfig::parse("gdrive:a'b").is_err());
        assert!(RemoteConfig::parse("https://drive.google.com/file/d/x").is_err());
        assert_eq!(
            RemoteConfig::parse("/srv/x"),
            Ok(RemoteConfig::LocalDir { path: "/srv/x".into() })
        );
        assert_eq!(g("x").to_string(), "gdrive:x");
        let r = RemoteConfig::parse("rsync:me@box:cpasfait/shared").unwrap();
        assert_eq!(
            r,
            RemoteConfig::Rsync {
                target: "me@box:cpasfait/shared".into()
            }
        );
        assert_eq!(r.to_string(), "rsync:me@box:cpasfait/shared");
        for spec in ["rsync:me@box:x", "gdrive:abc", "/srv/x"] {
            let r = RemoteConfig::parse(spec).unwrap();
            assert_eq!(RemoteConfig::parse(&r.spec()).unwrap(), r);
        }
        assert!(RemoteConfig::parse("rsync:box:/srv/x").is_ok());
        for bad in ["rsync:/local/path", "rsync:box:", "rsync::x", "rsync:-oProxy=x:y"] {
            assert!(RemoteConfig::parse(bad).is_err(), "{bad}");
        }
    }
}
