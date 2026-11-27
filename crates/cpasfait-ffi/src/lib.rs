//! cpasfait for the mobile apps: a [`Cpasfait`] object over the core, with
//! plain records for what the screens show (Kotlin bindings by UniFFI).
//!
//! Ids are strings (UUIDs), times are Unix milliseconds. Each [`Cpasfait`]
//! is one database connection, used by one call at a time: sync (slow, it
//! talks to the remote) on a second instance from a background thread, as
//! the TUI does. [`Cpasfait::data_version`] tells when the other one wrote.
//!
//! Remotes: local directories (and Google Drive when built with it) work
//! here. Others are provided by the app through [`RemoteProvider`] (for
//! instance `rsync:host:path` served over SFTP on Android, where there is no
//! rsync): the same remote spec on every device, each reaching it its way.

use cpasfait_core::backend::{self, RemoteBackend};
use cpasfait_core::crypto::Identity;
use cpasfait_core::model::DepState;
use cpasfait_core::sync::{invited_to, remote_tabs, sync_tab};
use cpasfait_core::{Note, Place, RemoteConfig, Repo, Session, Status, TabId, TaskId};
use jiff::tz::TimeZone;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

uniffi::setup_scaffolding!();

// ------------------------------------------------------------------ errors

/// Kotlin: `CpasfaitException.Failed` / `.NotInvited`, the message for the user.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CpasfaitError {
    /// Anything else, with a message for the user.
    #[error("{message}")]
    Failed { message: String },
    /// This device's key is not a member of the tab yet: a member must
    /// invite `key`.
    #[error("waiting for an invitation: ask a member to invite {key}")]
    NotInvited { key: String },
}

impl From<cpasfait_core::Error> for CpasfaitError {
    fn from(e: cpasfait_core::Error) -> Self {
        match e {
            cpasfait_core::Error::NotInvited(key) => CpasfaitError::NotInvited { key },
            cpasfait_core::Error::Invalid(message) => CpasfaitError::Failed { message },
            e => CpasfaitError::Failed { message: e.to_string() },
        }
    }
}

fn failed(message: impl Into<String>) -> CpasfaitError {
    CpasfaitError::Failed {
        message: message.into(),
    }
}

type Res<T> = Result<T, CpasfaitError>;

// ------------------------------------------------------------------ remotes

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RemoteError {
    #[error("{reason}")]
    Failed { reason: String },
    /// `get` of a missing file.
    #[error("not found")]
    NotFound,
    /// `put_new` of a file that exists.
    #[error("already exists")]
    AlreadyExists,
}

impl From<uniffi::UnexpectedUniFFICallbackError> for RemoteError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        RemoteError::Failed { reason: e.reason }
    }
}

/// Remote storage implemented by the app. Paths are relative and
/// `/`-separated. Files are never modified or deleted once written.
#[uniffi::export(with_foreign)]
pub trait Remote: Send + Sync {
    /// All the file paths under `prefix` (recursively); none if it does not exist.
    fn list(&self, prefix: String) -> Result<Vec<String>, RemoteError>;
    fn get(&self, path: String) -> Result<Vec<u8>, RemoteError>;
    /// Create `path` (and its directories), failing with `AlreadyExists` if
    /// it exists. Readers must never see it partly written: write a
    /// temporary file, then rename it (SFTP's rename does not overwrite).
    fn put_new(&self, path: String, data: Vec<u8>) -> Result<(), RemoteError>;
}

/// Gives the [`Remote`] for a remote spec (`rsync:host:path`,
/// `gdrive:ID`, a directory), or none to use the built-in backend.
#[uniffi::export(with_foreign)]
pub trait RemoteProvider: Send + Sync {
    fn open(&self, spec: String) -> Result<Option<Arc<dyn Remote>>, RemoteError>;
}

struct Foreign(Arc<dyn Remote>);

fn remote_error(e: RemoteError, path: &str) -> cpasfait_core::Error {
    match e {
        RemoteError::NotFound => cpasfait_core::Error::NotFound(path.to_owned()),
        RemoteError::AlreadyExists => cpasfait_core::Error::AlreadyExists(path.to_owned()),
        RemoteError::Failed { reason } => cpasfait_core::Error::Backend(reason),
    }
}

impl RemoteBackend for Foreign {
    fn list(&self, prefix: &str) -> cpasfait_core::Result<Vec<String>> {
        self.0.list(prefix.to_owned()).map_err(|e| remote_error(e, prefix))
    }

    fn get(&self, path: &str) -> cpasfait_core::Result<Vec<u8>> {
        self.0.get(path.to_owned()).map_err(|e| remote_error(e, path))
    }

    fn put_new(&self, path: &str, data: &[u8]) -> cpasfait_core::Result<()> {
        self.0
            .put_new(path.to_owned(), data.to_vec())
            .map_err(|e| remote_error(e, path))
    }
}

fn open_remote(config: &RemoteConfig, provider: Option<&Arc<dyn RemoteProvider>>) -> Res<Box<dyn RemoteBackend>> {
    if let Some(p) = provider
        && let Some(r) = p.open(config.spec()).map_err(|e| failed(e.to_string()))?
    {
        return Ok(Box::new(Foreign(r)));
    }
    Ok(backend::open(config)?)
}

fn parse_remote(spec: &str) -> Res<RemoteConfig> {
    RemoteConfig::parse(spec).map_err(failed)
}

// ------------------------------------------------------------------ records

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TabView {
    pub id: String,
    pub name: String,
    /// Its remote spec, when shared.
    pub remote: Option<String>,
    /// A move of the tab to another remote, made on another device, that
    /// this one has not followed yet (see [`Cpasfait::accept_move`]).
    pub pending_move: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MemberView {
    pub key: String,
    pub name: String,
    /// False once revoked.
    pub active: bool,
}

/// A task line of a tab, in display order (siblings by decreasing score).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RowView {
    pub id: String,
    pub parent: Option<String>,
    pub depth: u32,
    pub has_children: bool,
    pub title: String,
    pub done: bool,
    /// Done, or under a done task.
    pub effectively_done: bool,
    /// Requires a task that is not done.
    pub blocked: bool,
    pub priority: i64,
    /// As typed back by [`Cpasfait::set_due`]: `2026-12-01` or `2026-12-01 14:30`.
    pub due: Option<String>,
    /// When it falls due (Unix ms; midnight for a date).
    pub due_ms: Option<i64>,
    pub tags: Vec<String>,
    pub score: i64,
    /// Has a description (free text).
    pub has_description: bool,
    /// Number of dated notes.
    pub note_count: u32,
    /// Users working on it right now (open sessions).
    pub working: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SessionView {
    pub id: String,
    pub task: Option<String>,
    pub user: String,
    /// Recorded by this user.
    pub mine: bool,
    pub start: Option<i64>,
    /// None while running.
    pub end: Option<i64>,
    pub note: String,
}

/// A dated note on a task (a status update).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NoteView {
    pub id: String,
    /// Unix ms.
    pub at: i64,
    pub author: String,
    /// Written by this user.
    pub mine: bool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DepStateView {
    Todo,
    Done,
    Deleted,
    /// Not on this device (not synced yet).
    Missing,
    /// In a tab this device does not have.
    UnknownTab,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DepView {
    pub task: String,
    pub title: Option<String>,
    /// For a task in another tab.
    pub tab_name: Option<String>,
    pub state: DepStateView,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TaskDetail {
    pub id: String,
    pub tab: String,
    pub parent: Option<String>,
    pub title: String,
    /// Free text about the task.
    pub description: String,
    /// Dated notes, newest first.
    pub notes: Vec<NoteView>,
    pub done: bool,
    pub priority: i64,
    pub due: Option<String>,
    pub due_ms: Option<i64>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    /// Score parts: priority, due date, tasks it blocks.
    pub score_priority: i64,
    pub score_due: i64,
    pub score_blocking: i64,
    /// Work sessions, latest first.
    pub sessions: Vec<SessionView>,
    /// Time worked on it, all users (Unix ms, running sessions until now).
    pub worked_ms: i64,
    pub requires: Vec<DepView>,
    pub blocks: Vec<DepView>,
}

/// Where to put a task.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PlaceArg {
    FirstChildOf { parent: Option<String> },
    LastChildOf { parent: Option<String> },
    Before { task: String },
    After { task: String },
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DueView {
    pub task: String,
    pub tab: String,
    pub tab_name: String,
    pub title: String,
    pub due: String,
    pub due_ms: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SyncSummary {
    pub pulled: u64,
    pub pushed: u64,
    /// Changes that need a key this device does not have yet.
    pub waiting_for_key: u64,
    /// One per tab that failed (with its name).
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Joined {
    pub tab: String,
    /// This device's key, to be invited, when no member did yet.
    pub waiting_for_invitation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchHit {
    pub task: String,
    pub tab: String,
    pub tab_name: String,
    pub title: String,
    /// Its ancestors' titles, from the top (`Home › Kitchen`), empty at the top.
    pub path: String,
    /// Done, or under a done task.
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ReportLine {
    /// None: the time on deleted tasks.
    pub task: Option<String>,
    pub title: String,
    pub depth: u32,
    /// On this task itself.
    pub own_ms: i64,
    /// With its subtasks.
    pub total_ms: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ReportTab {
    pub tab: String,
    pub name: String,
    pub total_ms: i64,
    pub lines: Vec<ReportLine>,
}

// ------------------------------------------------------------------ conversions

fn task_id(s: &str) -> Res<TaskId> {
    s.parse().map_err(|_| failed(format!("bad task id {s:?}")))
}

fn session_id(s: &str) -> Res<cpasfait_core::SessionId> {
    s.parse().map_err(|_| failed(format!("bad session id {s:?}")))
}

fn note_id(s: &str) -> Res<cpasfait_core::NoteId> {
    s.parse().map_err(|_| failed(format!("bad note id {s:?}")))
}

fn tab_id(s: &str) -> Res<TabId> {
    s.parse().map_err(|_| failed(format!("bad tab id {s:?}")))
}

fn place(p: PlaceArg) -> Res<Place> {
    let parent = |p: Option<String>| p.as_deref().map(task_id).transpose();
    Ok(match p {
        PlaceArg::FirstChildOf { parent: p } => Place::FirstChildOf(parent(p)?),
        PlaceArg::LastChildOf { parent: p } => Place::LastChildOf(parent(p)?),
        PlaceArg::Before { task } => Place::Before(task_id(&task)?),
        PlaceArg::After { task } => Place::After(task_id(&task)?),
    })
}

fn note_view(n: &Note, me: &str) -> NoteView {
    NoteView {
        id: n.id.to_string(),
        at: n.at,
        author: n.author.name.clone(),
        mine: n.author.name == me,
        text: n.text.clone(),
    }
}

fn session_view(s: &Session, me: &str) -> SessionView {
    SessionView {
        id: s.id.to_string(),
        task: s.task.map(|t| t.to_string()),
        user: s.user.clone(),
        mine: s.user == me,
        start: s.start,
        end: s.end,
        note: s.note.clone(),
    }
}

/// `own_tab`: the tab of the task whose dependency this is (its name is
/// only given for tasks elsewhere).
fn dep_view(d: &cpasfait_core::DepInfo, own_tab: TabId) -> DepView {
    DepView {
        task: d.task.to_string(),
        title: d.title.clone(),
        tab_name: if d.tab == Some(own_tab) {
            None
        } else {
            d.tab_name.clone()
        },
        state: match d.state {
            DepState::Todo => DepStateView::Todo,
            DepState::Done => DepStateView::Done,
            DepState::Deleted => DepStateView::Deleted,
            DepState::Missing => DepStateView::Missing,
            DepState::UnknownTab => DepStateView::UnknownTab,
        },
    }
}

// ------------------------------------------------------------------ the object

#[derive(uniffi::Object)]
pub struct Cpasfait {
    repo: Mutex<Repo>,
    identity: Mutex<Option<Identity>>,
    tz: TimeZone,
}

impl Cpasfait {
    fn repo(&self) -> MutexGuard<'_, Repo> {
        self.repo.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The repo and this device's identity (created if there is none).
    fn with_identity<T>(&self, f: impl FnOnce(&mut Repo, &Identity) -> Res<T>) -> Res<T> {
        let mut id = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        if id.is_none() {
            *id = Some(Identity::load_or_create_default()?.0);
        }
        f(&mut self.repo(), id.as_ref().expect("set"))
    }

    fn due_fields(&self, due: Option<cpasfait_core::Due>) -> (Option<String>, Option<i64>) {
        (due.map(|d| d.to_string()), due.and_then(|d| d.moment_ms(&self.tz)))
    }
}

#[uniffi::export]
impl Cpasfait {
    /// Open (or create) the database in `data_dir`, which also holds the
    /// key and the configuration. `time_zone`: an IANA name
    /// (`Europe/Paris`), else the system's.
    #[uniffi::constructor]
    pub fn open(data_dir: String, time_zone: Option<String>) -> Res<Arc<Self>> {
        let dir = PathBuf::from(&data_dir);
        std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{data_dir}: {e}")))?;
        cpasfait_core::paths::set_data_dir(dir.clone());
        if cpasfait_core::paths::data_dir() != dir {
            return Err(failed(format!(
                "already using {}: one data directory per process",
                cpasfait_core::paths::data_dir().display()
            )));
        }
        let mut repo = Repo::open(dir.join("cpasfait.db"))?;
        if repo.tabs()?.is_empty() {
            repo.create_tab("inbox")?;
        }
        let tz = match time_zone {
            Some(name) => TimeZone::get(&name).map_err(|e| failed(format!("time zone {name}: {e}")))?,
            None => TimeZone::system(),
        };
        Ok(Arc::new(Cpasfait {
            repo: Mutex::new(repo),
            identity: Mutex::new(None),
            tz,
        }))
    }

    // -------------------------------------------------------------- you

    /// The name recorded on work sessions.
    pub fn user(&self) -> String {
        self.repo().user().to_owned()
    }

    pub fn set_user(&self, name: String) -> Res<()> {
        Ok(self.repo().set_user(&name)?)
    }

    /// The email recorded as the author of new tasks.
    pub fn email(&self) -> Option<String> {
        self.repo().email().map(str::to_owned)
    }

    pub fn set_email(&self, email: Option<String>) -> Res<()> {
        Ok(self.repo().set_email(email.as_deref())?)
    }

    /// This device's public key (to be invited to shared tabs), creating
    /// the key pair if there is none.
    pub fn public_key(&self) -> Res<String> {
        self.with_identity(|_, me| Ok(me.recipient().to_owned()))
    }

    /// Changes when the database was written by another connection (the
    /// sync instance): time to reload what is shown.
    pub fn data_version(&self) -> Res<i64> {
        Ok(self.repo().data_version()?)
    }

    // -------------------------------------------------------------- tabs

    pub fn tabs(&self) -> Res<Vec<TabView>> {
        Ok(self
            .repo()
            .tabs()?
            .into_iter()
            .map(|t| TabView {
                id: t.id.to_string(),
                name: t.name,
                remote: t.remote.map(|r| r.spec()),
                pending_move: t.pending_move.map(|m| m.remote),
            })
            .collect())
    }

    pub fn create_tab(&self, name: String) -> Res<String> {
        Ok(self.repo().create_tab(&name)?.to_string())
    }

    pub fn rename_tab(&self, tab: String, name: String) -> Res<()> {
        Ok(self.repo().rename_tab(tab_id(&tab)?, &name)?)
    }

    /// Deletes it for everyone sharing it.
    pub fn delete_tab(&self, tab: String) -> Res<()> {
        Ok(self.repo().delete_tab(tab_id(&tab)?)?)
    }

    pub fn members(&self, tab: String) -> Res<Vec<MemberView>> {
        Ok(self
            .repo()
            .members(tab_id(&tab)?)?
            .into_iter()
            .map(|m| MemberView {
                key: m.recipient,
                name: m.name,
                active: m.active,
            })
            .collect())
    }

    // -------------------------------------------------------------- tasks

    /// The tab's tasks, in display order, with their depth (the app folds
    /// what it wants). `hide_done`: leave out done tasks and their subtrees.
    pub fn rows(&self, tab: String, hide_done: bool) -> Res<Vec<RowView>> {
        let tab = tab_id(&tab)?;
        let repo = self.repo();
        let tree = repo.tree(tab)?;
        let scores = tree.scores();
        let note_counts = repo.note_counts(tab)?;
        let mut working: HashMap<TaskId, Vec<String>> = HashMap::new();
        for s in repo.open_sessions(tab)? {
            if let Some(t) = s.task {
                working.entry(t).or_default().push(s.user);
            }
        }
        Ok(tree
            .rows(&HashSet::new(), hide_done)
            .into_iter()
            .filter_map(|r| {
                let t = tree.get(r.id)?;
                let (due, due_ms) = self.due_fields(t.due);
                Some(RowView {
                    id: t.id.to_string(),
                    parent: t.parent.map(|p| p.to_string()),
                    depth: r.depth as u32,
                    has_children: r.has_children,
                    title: t.title.clone(),
                    done: t.is_done(),
                    effectively_done: tree.is_effectively_done(t.id),
                    blocked: tree.is_blocked(t.id),
                    priority: t.priority,
                    due,
                    due_ms,
                    tags: t.tags.clone(),
                    score: scores.get(&t.id).map(|s| s.total()).unwrap_or_default(),
                    has_description: !t.notes.trim().is_empty(),
                    note_count: note_counts.get(&t.id).copied().unwrap_or(0),
                    working: working.remove(&t.id).unwrap_or_default(),
                })
            })
            .collect())
    }

    pub fn task(&self, id: String) -> Res<TaskDetail> {
        let id = task_id(&id)?;
        let repo = self.repo();
        let t = repo.task(id)?;
        let score = repo.tree(t.tab)?.scores().get(&id).copied().unwrap_or_default();
        let me = repo.user().to_owned();
        let now = repo.now_ms();
        let mut sessions: Vec<Session> = repo.sessions_for_task(id)?.into_iter().filter(|s| !s.deleted).collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.start));
        let deps = repo.dependencies(id)?;
        let (due, due_ms) = self.due_fields(t.due);
        Ok(TaskDetail {
            id: t.id.to_string(),
            tab: t.tab.to_string(),
            parent: t.parent.map(|p| p.to_string()),
            title: t.title.clone(),
            description: t.notes.clone(),
            notes: repo.notes_for_task(id)?.iter().map(|n| note_view(n, &me)).collect(),
            done: t.is_done(),
            priority: t.priority,
            due,
            due_ms,
            tags: t.tags.clone(),
            author: t.author.as_ref().map(|a| a.name.clone()),
            score_priority: score.priority,
            score_due: score.due,
            score_blocking: score.blocking,
            worked_ms: sessions.iter().map(|s| s.duration_ms(now)).sum(),
            sessions: sessions.iter().map(|s| session_view(s, &me)).collect(),
            requires: deps.requires.iter().map(|d| dep_view(d, t.tab)).collect(),
            blocks: deps.blocks.iter().map(|d| dep_view(d, t.tab)).collect(),
        })
    }

    /// The task to do next in `tab`: the most prioritary actionable one.
    pub fn next_task(&self, tab: String) -> Res<Option<String>> {
        Ok(self.repo().next_task(tab_id(&tab)?)?.map(|t| t.id.to_string()))
    }

    /// Tasks of all tabs whose title or ancestors contain all the words of
    /// `query` (any case), open ones first, at most `limit`.
    pub fn search(&self, query: String, limit: u32) -> Res<Vec<SearchHit>> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Ok(vec![]);
        }
        let repo = self.repo();
        let mut hits = vec![];
        for tab in repo.tabs()? {
            let tree = repo.tree(tab.id)?;
            for t in tree.iter() {
                let path = tree.path(t.id);
                let haystack = path.join(" ").to_lowercase();
                if !words.iter().all(|w| haystack.contains(w.as_str())) {
                    continue;
                }
                let in_title = words
                    .iter()
                    .filter(|w| t.title.to_lowercase().contains(w.as_str()))
                    .count();
                hits.push((
                    tree.is_effectively_done(t.id),
                    std::cmp::Reverse(in_title),
                    SearchHit {
                        task: t.id.to_string(),
                        tab: tab.id.to_string(),
                        tab_name: tab.name.clone(),
                        title: t.title.clone(),
                        path: path[..path.len() - 1].join(" › "),
                        done: tree.is_effectively_done(t.id),
                    },
                ));
            }
        }
        hits.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| a.2.title.cmp(&b.2.title)));
        Ok(hits.into_iter().take(limit as usize).map(|h| h.2).collect())
    }

    pub fn add_task(&self, tab: String, place: PlaceArg, title: String) -> Res<String> {
        Ok(self
            .repo()
            .add_task(tab_id(&tab)?, self::place(place)?, &title)?
            .to_string())
    }

    pub fn set_title(&self, task: String, title: String) -> Res<()> {
        Ok(self.repo().set_title(task_id(&task)?, &title)?)
    }

    pub fn set_description(&self, task: String, text: String) -> Res<()> {
        Ok(self.repo().set_notes(task_id(&task)?, &text)?)
    }

    /// Add a dated note (by this user, now). Returns its id.
    pub fn add_note(&self, task: String, text: String) -> Res<String> {
        Ok(self.repo().add_note(task_id(&task)?, &text)?.to_string())
    }

    /// Change a note's text (empty: delete it).
    pub fn set_note_text(&self, note: String, text: String) -> Res<()> {
        Ok(self.repo().set_note_text(note_id(&note)?, &text)?)
    }

    pub fn delete_note(&self, note: String) -> Res<()> {
        Ok(self.repo().delete_note(note_id(&note)?)?)
    }

    pub fn set_done(&self, task: String, done: bool) -> Res<()> {
        let status = if done { Status::Done } else { Status::Todo };
        Ok(self.repo().set_status(task_id(&task)?, status)?)
    }

    /// From -3 to 3.
    pub fn set_priority(&self, task: String, priority: i64) -> Res<()> {
        Ok(self.repo().set_priority(task_id(&task)?, priority)?)
    }

    /// `due` as typed: `2026-12-01`, `tom`, `+3d`, `fri 9:00`, `17:30`
    /// (today)… None removes it. Returns how it was understood.
    pub fn set_due(&self, task: String, due: Option<String>) -> Res<Option<String>> {
        let task = task_id(&task)?;
        let mut repo = self.repo();
        let due = match due.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) => Some(
                cpasfait_core::dates::parse_due(s, repo.today())
                    .ok_or_else(|| failed(format!("can't understand the due date {s:?}")))?,
            ),
        };
        repo.set_due(task, due)?;
        Ok(due.map(|d| d.to_string()))
    }

    pub fn set_tag(&self, task: String, tag: String, on: bool) -> Res<()> {
        Ok(self.repo().set_tag(task_id(&task)?, &tag, on)?)
    }

    /// `task` requires `blocker` (`on`), or no longer does.
    pub fn set_blocked_by(&self, task: String, blocker: String, on: bool) -> Res<()> {
        Ok(self.repo().set_blocked_by(task_id(&task)?, task_id(&blocker)?, on)?)
    }

    pub fn move_task(&self, task: String, place: PlaceArg) -> Res<()> {
        Ok(self.repo().move_task(task_id(&task)?, self::place(place)?)?)
    }

    /// Make it a child of its previous sibling. False: it has none.
    pub fn indent(&self, task: String) -> Res<bool> {
        Ok(self.repo().indent(task_id(&task)?)?)
    }

    /// Make it a sibling of its parent. False: it is at the top.
    pub fn outdent(&self, task: String) -> Res<bool> {
        Ok(self.repo().outdent(task_id(&task)?)?)
    }

    /// Move it, its subtasks and work log to another tab (at its end).
    /// Returns the new id of the task.
    pub fn move_to_tab(&self, task: String, tab: String) -> Res<String> {
        let moved = self
            .repo()
            .move_to_tab(task_id(&task)?, tab_id(&tab)?, Place::LastChildOf(None))?;
        Ok(moved.id.to_string())
    }

    pub fn delete_task(&self, task: String) -> Res<()> {
        Ok(self.repo().delete_task(task_id(&task)?)?)
    }

    pub fn undelete_task(&self, task: String) -> Res<()> {
        Ok(self.repo().undelete_task(task_id(&task)?)?)
    }

    // -------------------------------------------------------------- work log

    /// Start working on `task` (stopping what was running). Returns the
    /// new session.
    pub fn start_work(&self, task: String) -> Res<String> {
        Ok(self.repo().start_work(task_id(&task)?)?.to_string())
    }

    pub fn stop_work(&self) -> Res<()> {
        self.repo().stop_work()?;
        Ok(())
    }

    /// The running session of this device.
    pub fn active_session(&self) -> Res<Option<SessionView>> {
        let repo = self.repo();
        Ok(repo.active_session()?.map(|s| session_view(&s, repo.user())))
    }

    pub fn set_session_note(&self, session: String, note: String) -> Res<()> {
        Ok(self.repo().set_session_note(session_id(&session)?, &note)?)
    }

    /// Log time spent on `task` after the fact (Unix ms, not in the future).
    /// Returns the new session.
    pub fn add_session(&self, task: String, start: i64, end: i64) -> Res<String> {
        Ok(self.repo().add_session(task_id(&task)?, start, end)?.to_string())
    }

    /// Correct a session's times; `end`: None only for the running session.
    pub fn set_session_span(&self, session: String, start: i64, end: Option<i64>) -> Res<()> {
        Ok(self.repo().set_session_span(session_id(&session)?, start, end)?)
    }

    pub fn delete_session(&self, session: String) -> Res<()> {
        Ok(self.repo().delete_session(session_id(&session)?)?)
    }

    /// Time spent in `[from, to)` (Unix ms), per tab and task, totals rolled
    /// up through subtasks. `tab`: only that one.
    pub fn time_report(&self, from: i64, to: i64, tab: Option<String>) -> Res<Vec<ReportTab>> {
        use cpasfait_core::report;
        let repo = self.repo();
        let filter = report::Filter {
            tab: tab.as_deref().map(tab_id).transpose()?,
            user: None,
        };
        let slices = report::slices(&repo, from, to, &filter)?;
        Ok(report::by_task(&repo, &slices)?
            .into_iter()
            .map(|t| ReportTab {
                tab: t.tab.to_string(),
                name: t.name,
                total_ms: t.total_ms,
                lines: t
                    .lines
                    .into_iter()
                    .map(|l| ReportLine {
                        task: l.task.map(|t| t.to_string()),
                        title: l.title,
                        depth: l.depth as u32,
                        own_ms: l.own_ms,
                        total_ms: l.total_ms,
                    })
                    .collect(),
            })
            .collect())
    }

    // -------------------------------------------------------------- due dates

    /// Tasks that fell due and were not announced yet (each due date is
    /// returned once, here or by the desktop apps on this database).
    pub fn take_due_notices(&self) -> Res<Vec<DueView>> {
        let notices = self.repo().take_due_notices(&self.tz)?;
        Ok(notices
            .into_iter()
            .map(|n| DueView {
                task: n.task.to_string(),
                tab: n.tab.to_string(),
                tab_name: n.tab_name,
                title: n.title,
                due: n.due.to_string(),
                due_ms: n.due.moment_ms(&self.tz).unwrap_or_default(),
            })
            .collect())
    }

    /// Open tasks falling due after now, soonest first (to schedule alarms).
    pub fn upcoming_dues(&self) -> Res<Vec<DueView>> {
        let repo = self.repo();
        let now = repo.now_ms();
        let mut out = vec![];
        for tab in repo.tabs()? {
            let tree = repo.tree(tab.id)?;
            for t in tree.iter() {
                let Some(due) = t.due else { continue };
                let Some(ms) = due.moment_ms(&self.tz) else { continue };
                if ms > now && !tree.is_effectively_done(t.id) {
                    out.push(DueView {
                        task: t.id.to_string(),
                        tab: tab.id.to_string(),
                        tab_name: tab.name.clone(),
                        title: t.title.clone(),
                        due: due.to_string(),
                        due_ms: ms,
                    });
                }
            }
        }
        out.sort_by_key(|d| d.due_ms);
        Ok(out)
    }

    // -------------------------------------------------------------- sharing

    /// Share `tab` through the remote `spec` (end-to-end encrypted; the data
    /// goes there at the next [`sync`](Self::sync)), or stop syncing it (None).
    pub fn set_remote(&self, tab: String, spec: Option<String>) -> Res<()> {
        let tab = tab_id(&tab)?;
        match spec {
            None => Ok(self.repo().set_remote(tab, None)?),
            Some(spec) => {
                let config = parse_remote(&spec)?;
                self.with_identity(|repo, me| {
                    repo.share_tab(tab, me.recipient())?;
                    Ok(repo.set_remote(tab, Some(&config))?)
                })
            }
        }
    }

    /// Let `key` (`age1…`, `ssh-ed25519 …`) read and write `tab`, from the
    /// next sync on.
    pub fn invite(&self, tab: String, key: String, name: Option<String>) -> Res<()> {
        let tab = tab_id(&tab)?;
        let mut repo = self.repo();
        if repo.epochs(tab)?.is_empty() {
            return Err(failed("this tab is not shared: give it a remote first"));
        }
        let key = cpasfait_core::crypto::parse_recipient(&key)?;
        if repo.members(tab)?.iter().any(|m| m.active && m.recipient == key) {
            return Err(failed("already a member"));
        }
        let name = name.unwrap_or_else(|| cpasfait_core::crypto::short_key(&key));
        Ok(repo.set_member(tab, &key, Some(&name))?)
    }

    /// Remove a member (by key): they no longer get the new changes (a new
    /// key is made at the next sync). Stop sharing the remote with them too.
    pub fn revoke(&self, tab: String, key: String) -> Res<()> {
        let tab = tab_id(&tab)?;
        let mut repo = self.repo();
        let active: Vec<_> = repo.members(tab)?.into_iter().filter(|m| m.active).collect();
        if !active.iter().any(|m| m.recipient == key) {
            return Err(failed("not a member"));
        }
        if active.len() == 1 {
            return Err(failed("that is the last member"));
        }
        Ok(repo.set_member(tab, &key, None)?)
    }

    /// Priority given to the tab's new tasks.
    pub fn default_priority(&self, tab: String) -> Res<i64> {
        Ok(self.repo().tab_settings(tab_id(&tab)?)?.default_priority)
    }

    /// None: back to the default (0).
    pub fn set_default_priority(&self, tab: String, priority: Option<i64>) -> Res<()> {
        let value = priority.map(|p| p.to_string());
        Ok(self
            .repo()
            .set_tab_setting(tab_id(&tab)?, "default_priority", value.as_deref())?)
    }

    /// Move a shared tab to another remote, for everyone: copies its files
    /// there and switches to it; the other devices are asked to follow.
    /// Returns the number of files copied. Slow: use the sync instance.
    pub fn move_tab(&self, tab: String, spec: String, remotes: Option<Arc<dyn RemoteProvider>>) -> Res<u64> {
        let tab = tab_id(&tab)?;
        let old = self
            .repo()
            .tabs()?
            .into_iter()
            .find(|t| t.id == tab)
            .and_then(|t| t.remote)
            .ok_or_else(|| failed("this tab has no remote"))?;
        let config = parse_remote(&spec)?;
        if config == old {
            return Err(failed("already there"));
        }
        let (from, to) = (
            open_remote(&old, remotes.as_ref())?,
            open_remote(&config, remotes.as_ref())?,
        );
        self.with_identity(|repo, me| {
            let n = cpasfait_core::sync::move_tab(repo, tab, from.as_ref(), to.as_ref(), &config, me, &mut |_| {})?;
            Ok(n as u64)
        })
    }

    /// Join a shared tab at `spec`: `tab` if given, else the one new tab
    /// there (the one this device was invited to, when there are several).
    pub fn join_tab(&self, spec: String, tab: Option<String>, remotes: Option<Arc<dyn RemoteProvider>>) -> Res<Joined> {
        let config = parse_remote(&spec)?;
        let b = open_remote(&config, remotes.as_ref())?;
        self.with_identity(|repo, me| {
            let tab = match tab {
                Some(t) => tab_id(&t)?,
                None => {
                    let known: HashSet<TabId> = repo.tabs()?.iter().map(|t| t.id).collect();
                    let mut ids: Vec<TabId> = remote_tabs(b.as_ref())?
                        .into_iter()
                        .filter(|t| !known.contains(t))
                        .collect();
                    if ids.len() > 1 {
                        let mut invited = vec![];
                        for t in &ids {
                            if invited_to(b.as_ref(), *t, me)? {
                                invited.push(*t);
                            }
                        }
                        if !invited.is_empty() {
                            ids = invited;
                        }
                    }
                    match ids.as_slice() {
                        [one] => *one,
                        [] => return Err(failed(format!("no new tab found in {spec}"))),
                        many => {
                            return Err(failed(format!(
                                "several tabs in {spec}, pick one: {}",
                                many.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" ")
                            )));
                        }
                    }
                }
            };
            repo.add_known_tab(tab)?;
            repo.set_remote(tab, Some(&config))?;
            let res = sync_tab(repo, tab, b.as_ref(), me);
            // Joined where a move pointed to: nothing left to follow.
            if let Some(m) = repo
                .tabs()?
                .into_iter()
                .find(|t| t.id == tab)
                .and_then(|t| t.pending_move)
                && m.remote == config.spec()
            {
                repo.mark_move_done(tab, m.id)?;
            }
            let waiting_for_invitation = match res {
                Ok(_) => None,
                Err(cpasfait_core::Error::NotInvited(key)) => Some(key),
                Err(e) => return Err(e.into()),
            };
            Ok(Joined {
                tab: tab.to_string(),
                waiting_for_invitation,
            })
        })
    }

    /// Follow a move of `tab` made on another device: to `spec` if given
    /// (where it is from here), else where they said.
    pub fn accept_move(
        &self,
        tab: String,
        spec: Option<String>,
        remotes: Option<Arc<dyn RemoteProvider>>,
    ) -> Res<Vec<String>> {
        let tab = tab_id(&tab)?;
        let info = self
            .repo()
            .tabs()?
            .into_iter()
            .find(|t| t.id == tab)
            .ok_or_else(|| failed("no such tab"))?;
        let pending = info
            .pending_move
            .ok_or_else(|| failed("no move to follow for this tab"))?;
        let config = parse_remote(spec.as_deref().unwrap_or(&pending.remote))?;
        let old = info.remote.and_then(|r| open_remote(&r, remotes.as_ref()).ok());
        let new = open_remote(&config, remotes.as_ref())?;
        self.with_identity(|repo, me| {
            let r =
                cpasfait_core::sync::accept_move(repo, tab, old.as_deref(), new.as_ref(), &config, me, &mut |_| {})?;
            Ok(r.warnings)
        })
    }

    /// Sync every shared tab. Slow: call it from a background thread, on
    /// its own [`Cpasfait`] instance.
    pub fn sync(&self, remotes: Option<Arc<dyn RemoteProvider>>) -> Res<SyncSummary> {
        let tabs: Vec<_> = self.repo().tabs()?.into_iter().filter(|t| t.remote.is_some()).collect();
        let mut sum = SyncSummary {
            pulled: 0,
            pushed: 0,
            waiting_for_key: 0,
            errors: vec![],
        };
        if tabs.is_empty() {
            return Ok(sum);
        }
        self.with_identity(|repo, me| {
            for tab in tabs {
                let remote = tab.remote.as_ref().expect("filtered");
                let res =
                    open_remote(remote, remotes.as_ref()).and_then(|b| Ok(sync_tab(repo, tab.id, b.as_ref(), me)?));
                match res {
                    Ok(s) => {
                        sum.pulled += s.pulled as u64;
                        sum.pushed += s.pushed as u64;
                        sum.waiting_for_key += s.waiting_for_key as u64;
                    }
                    Err(e) => sum.errors.push(format!("{}: {e}", tab.name)),
                }
            }
            Ok(sum)
        })
    }
}
