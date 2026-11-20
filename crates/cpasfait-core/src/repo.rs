//! [`Repo`]: the high-level API used by the TUI, the tools and (later) the FFI.

use crate::apply::{apply_op, apply_to_model};
use crate::error::{Error, Result};
use crate::hlc::{ClockState, Hlc};
use crate::ids::{DeviceId, SessionId, TabId, TaskId};
use crate::model::{DepInfo, DepState, Dependencies, RemoteConfig, Session, TabInfo, Task};
use crate::op::{Change, Entity, Op, Status};
use crate::position::key_between;
use crate::schema;
use crate::tree::Tree;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

type NowFn = Box<dyn Fn() -> i64 + Send>;

pub fn system_now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// Where to put a task among its siblings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    FirstChildOf(Option<TaskId>),
    LastChildOf(Option<TaskId>),
    Before(TaskId),
    After(TaskId),
}

pub struct Repo {
    conn: Connection,
    device: DeviceId,
    user: String,
    now: NowFn,
}

fn parse_id<T: std::str::FromStr>(s: &str) -> rusqlite::Result<T> {
    s.parse().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("bad id {s}").into())
    })
}

fn opt_id<T: std::str::FromStr>(s: Option<String>) -> rusqlite::Result<Option<T>> {
    s.map(|s| parse_id(&s)).transpose()
}

const TASK_COLS: &str = "t.id, t.tab, t.title, t.notes, t.status, t.priority, t.due, t.parent, t.position, t.deleted,
    (SELECT group_concat(tag, char(31)) FROM (SELECT tag FROM task_tag WHERE task = t.id ORDER BY tag)),
    (SELECT group_concat(blocker, ' ') FROM task_dep WHERE task = t.id)";

fn id_list(s: Option<String>) -> rusqlite::Result<Vec<TaskId>> {
    let mut v: Vec<TaskId> = s
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(parse_id)
        .collect::<rusqlite::Result<_>>()?;
    v.sort();
    Ok(v)
}

fn task_from_row(r: &Row) -> rusqlite::Result<Task> {
    let status: String = r.get(4)?;
    let due: Option<String> = r.get(6)?;
    let tags: Option<String> = r.get(10)?;
    Ok(Task {
        id: parse_id(&r.get::<_, String>(0)?)?,
        tab: parse_id(&r.get::<_, String>(1)?)?,
        title: r.get(2)?,
        notes: r.get(3)?,
        status: Status::parse(&status).unwrap_or(Status::Todo),
        priority: r.get(5)?,
        due: due.and_then(|d| d.parse().ok()),
        parent: opt_id(r.get(7)?)?,
        position: r.get(8)?,
        deleted: r.get(9)?,
        tags: tags
            .map(|t| t.split('\u{1f}').map(str::to_owned).collect())
            .unwrap_or_default(),
        blocked_by: id_list(r.get(11)?)?,
    })
}

const SESSION_COLS: &str = "id, tab, task, device, user, start, end, note, deleted";

fn session_from_row(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: parse_id(&r.get::<_, String>(0)?)?,
        tab: parse_id(&r.get::<_, String>(1)?)?,
        task: opt_id(r.get(2)?)?,
        device: opt_id(r.get(3)?)?,
        user: r.get(4)?,
        start: r.get(5)?,
        end: r.get(6)?,
        note: r.get(7)?,
        deleted: r.get(8)?,
    })
}

fn op_from_row(r: &Row) -> rusqlite::Result<Op> {
    let value: String = r.get(7)?;
    Ok(Op {
        hlc: Hlc {
            wall: r.get(0)?,
            counter: r.get(1)?,
            device: parse_id(&r.get::<_, String>(2)?)?,
        },
        seq: r.get::<_, i64>(3)? as u64,
        tab: parse_id(&r.get::<_, String>(4)?)?,
        entity: parse_id(&r.get::<_, String>(5)?)?,
        field: r.get(6)?,
        value: serde_json::from_str(&value)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e)))?,
    })
}

const OP_COLS: &str = "hlc_wall, hlc_counter, device, seq, tab, entity, field, value";

/// A local edit: `entity.field = value` in `tab`.
pub type Edit = (TabId, Uuid, Change);

impl Repo {
    /// Open (creating if needed) a read-write database.
    pub fn open(path: impl AsRef<Path>) -> Result<Repo> {
        if let Some(dir) = path.as_ref().parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// Open an in-memory database (tests).
    pub fn open_in_memory() -> Result<Repo> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Open an existing database read-only: fast path for tools. A database
    /// with an older schema is first migrated (once) through a normal open.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Repo> {
        let path = path.as_ref();
        let mut conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < schema::SCHEMA_VERSION {
            drop(conn);
            drop(Self::open(path)?);
            conn =
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
            conn.busy_timeout(Duration::from_secs(5))?;
        }
        let (device, user) = conn
            .query_row("SELECT id, user FROM device", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get(1)?))
            })
            .optional()?
            .ok_or_else(|| Error::NotFound("device row (empty database?)".into()))?;
        Ok(Repo {
            device: parse_id(&device)?,
            user,
            conn,
            now: Box::new(system_now_ms),
        })
    }

    fn init(mut conn: Connection) -> Result<Repo> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", false)?;
        let needs_rebuild = schema::migrate(&mut conn)?;
        let existing: Option<(String, String)> = conn
            .query_row("SELECT id, user FROM device", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let (device, user) = match existing {
            Some((id, user)) => (parse_id(&id)?, user),
            None => {
                let id = DeviceId::new();
                let user = default_user();
                conn.execute(
                    "INSERT INTO device(id, user, hlc_wall, hlc_counter) VALUES (?1, ?2, 0, 0)",
                    params![id.to_string(), user],
                )?;
                (id, user)
            }
        };
        let mut repo = Repo {
            conn,
            device,
            user,
            now: Box::new(system_now_ms),
        };
        if needs_rebuild {
            repo.rebuild()?;
        }
        Ok(repo)
    }

    pub fn device(&self) -> DeviceId {
        self.device
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn set_user(&mut self, user: &str) -> Result<()> {
        self.conn.execute("UPDATE device SET user = ?1", params![user])?;
        self.user = user.to_owned();
        Ok(())
    }

    /// Replace the time source (tests).
    pub fn set_now(&mut self, now: impl Fn() -> i64 + Send + 'static) {
        self.now = Box::new(now);
    }

    pub fn now_ms(&self) -> i64 {
        (self.now)()
    }

    /// Changes whenever another connection (process, sync thread) commits to the
    /// database: cheap way to know when to refresh.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "data_version", |r| r.get(0))?)
    }

    // ---------------------------------------------------------------- writes

    /// Record local edits atomically: stamps each with the clock, logs and applies it.
    pub fn commit(&mut self, edits: Vec<Edit>) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let now = (self.now)();
        let device = self.device;
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut clock = read_clock(&tx)?;
        let mut seqs: std::collections::HashMap<TabId, u64> = Default::default();
        for (tab, entity, change) in edits {
            let seq = match seqs.get_mut(&tab) {
                Some(s) => s,
                None => {
                    let next: Option<i64> = tx
                        .query_row(
                            "SELECT next_seq FROM tab_seq WHERE tab = ?1",
                            params![tab.to_string()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    seqs.entry(tab).or_insert(next.unwrap_or(1) as u64)
                }
            };
            let op = Op {
                hlc: clock.tick(now, device),
                seq: *seq,
                tab,
                entity,
                field: change.field(),
                value: change.value(),
            };
            *seq += 1;
            apply_op(&tx, &op, false)?;
        }
        for (tab, next) in seqs {
            tx.execute(
                "INSERT INTO tab_seq(tab, next_seq) VALUES (?1, ?2) ON CONFLICT(tab) DO UPDATE SET next_seq = ?2",
                params![tab.to_string(), next as i64],
            )?;
        }
        tx.execute(
            "UPDATE device SET hlc_wall = ?1, hlc_counter = ?2",
            params![clock.wall, clock.counter],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn edit1(&mut self, tab: TabId, entity: impl Entity, change: Change) -> Result<()> {
        self.commit(vec![(tab, entity.uuid(), change)])
    }

    pub fn create_tab(&mut self, name: &str) -> Result<TabId> {
        let id = TabId::new();
        self.edit1(id, id, Change::TabName(name.to_owned()))?;
        Ok(id)
    }

    pub fn rename_tab(&mut self, tab: TabId, name: &str) -> Result<()> {
        self.edit1(tab, tab, Change::TabName(name.to_owned()))
    }

    pub fn delete_tab(&mut self, tab: TabId) -> Result<()> {
        self.edit1(tab, tab, Change::TabDeleted(true))
    }

    /// Register a tab known only by id (joining a shared tab); its content comes from sync.
    pub fn add_known_tab(&mut self, tab: TabId) -> Result<()> {
        self.conn
            .execute("INSERT OR IGNORE INTO tab(id) VALUES (?1)", params![tab.to_string()])?;
        Ok(())
    }

    pub fn set_remote(&mut self, tab: TabId, remote: Option<&RemoteConfig>) -> Result<()> {
        let json = remote.map(serde_json::to_string).transpose()?;
        let n = self.conn.execute(
            "UPDATE tab SET remote = ?2 WHERE id = ?1",
            params![tab.to_string(), json],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("tab {tab}")));
        }
        Ok(())
    }

    /// Positions of the non-deleted children of `parent` (`None` = roots), sorted.
    fn sibling_positions(
        &self,
        tab: TabId,
        parent: Option<TaskId>,
        except: Option<TaskId>,
    ) -> Result<Vec<(TaskId, String)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, position FROM task WHERE tab = ?1 AND parent IS ?2 AND deleted = 0 AND id IS NOT ?3",
        )?;
        let rows = st.query_map(
            params![
                tab.to_string(),
                parent.map(|p| p.to_string()),
                except.map(|e| e.to_string())
            ],
            |r| Ok((parse_id::<TaskId>(&r.get::<_, String>(0)?)?, r.get::<_, String>(1)?)),
        )?;
        let mut v: Vec<(TaskId, String)> = rows.collect::<rusqlite::Result<_>>()?;
        v.sort_by(|a, b| (&a.1, a.0).cmp(&(&b.1, b.0)));
        Ok(v)
    }

    /// Compute the parent and position key for a placement.
    fn placement(&self, tab: TabId, place: Place, moving: Option<TaskId>) -> Result<(Option<TaskId>, String)> {
        Ok(match place {
            Place::FirstChildOf(p) => {
                let sib = self.sibling_positions(tab, p, moving)?;
                (p, key_between(None, sib.first().map(|s| s.1.as_str())))
            }
            Place::LastChildOf(p) => {
                let sib = self.sibling_positions(tab, p, moving)?;
                (p, key_between(sib.last().map(|s| s.1.as_str()), None))
            }
            Place::Before(t) | Place::After(t) => {
                let parent = self.task(t)?.parent;
                let sib = self.sibling_positions(tab, parent, moving)?;
                let i = sib
                    .iter()
                    .position(|s| s.0 == t)
                    .ok_or_else(|| Error::NotFound(format!("task {t}")))?;
                let pos = |j: usize| sib.get(j).map(|s| s.1.as_str());
                let (lo, hi) = if matches!(place, Place::Before(_)) {
                    (i.checked_sub(1).and_then(pos), pos(i))
                } else {
                    (pos(i), pos(i + 1))
                };
                (parent, between(lo, hi))
            }
        })
    }

    pub fn add_task(&mut self, tab: TabId, place: Place, title: &str) -> Result<TaskId> {
        let (parent, position) = self.placement(tab, place, None)?;
        let id = TaskId::new();
        let mut edits = vec![
            (tab, id.0, Change::TaskTitle(title.to_owned())),
            (tab, id.0, Change::TaskPosition(position)),
        ];
        if parent.is_some() {
            edits.push((tab, id.0, Change::TaskParent(parent)));
        }
        self.commit(edits)?;
        Ok(id)
    }

    pub fn move_task(&mut self, task: TaskId, place: Place) -> Result<()> {
        let t = self.task(task)?;
        if let Place::FirstChildOf(Some(p)) | Place::LastChildOf(Some(p)) = place {
            let tree = self.structure(t.tab)?;
            if p == task || tree.ancestors(p).contains(&task) {
                return Err(Error::Invalid("cannot move a task under itself".into()));
            }
        }
        let (parent, position) = self.placement(t.tab, place, Some(task))?;
        let mut edits = vec![(t.tab, task.0, Change::TaskPosition(position))];
        if parent != t.parent {
            edits.push((t.tab, task.0, Change::TaskParent(parent)));
        }
        self.commit(edits)
    }

    /// Make the task the last child of its previous sibling.
    pub fn indent(&mut self, task: TaskId) -> Result<bool> {
        let t = self.task(task)?;
        let tree = self.structure(t.tab)?;
        let sib = tree.children(tree.parent(task));
        let i = sib.iter().position(|&s| s == task).unwrap_or(0);
        if i == 0 {
            return Ok(false);
        }
        self.move_task(task, Place::LastChildOf(Some(sib[i - 1])))?;
        Ok(true)
    }

    /// Make the task the next sibling of its parent.
    pub fn outdent(&mut self, task: TaskId) -> Result<bool> {
        let t = self.task(task)?;
        let tree = self.structure(t.tab)?;
        match tree.parent(task) {
            None => Ok(false),
            Some(p) => {
                self.move_task(task, Place::After(p))?;
                Ok(true)
            }
        }
    }

    /// Swap with the previous (`up`) or next sibling.
    pub fn shift(&mut self, task: TaskId, up: bool) -> Result<bool> {
        let t = self.task(task)?;
        let tree = self.structure(t.tab)?;
        let sib = tree.children(tree.parent(task));
        let i = sib.iter().position(|&s| s == task).unwrap_or(0);
        let target = if up {
            i.checked_sub(1)
        } else {
            Some(i + 1).filter(|&j| j < sib.len())
        };
        match target {
            None => Ok(false),
            Some(j) => {
                let place = if up {
                    Place::Before(sib[j])
                } else {
                    Place::After(sib[j])
                };
                self.move_task(task, place)?;
                Ok(true)
            }
        }
    }

    pub fn set_task(&mut self, task: TaskId, change: Change) -> Result<()> {
        let t = self.task(task)?;
        self.edit1(t.tab, task, change)
    }

    pub fn set_title(&mut self, task: TaskId, title: &str) -> Result<()> {
        self.set_task(task, Change::TaskTitle(title.to_owned()))
    }

    pub fn set_notes(&mut self, task: TaskId, notes: &str) -> Result<()> {
        self.set_task(task, Change::TaskNotes(notes.to_owned()))
    }

    pub fn set_status(&mut self, task: TaskId, status: Status) -> Result<()> {
        self.set_task(task, Change::TaskStatus(status))
    }

    pub fn toggle_done(&mut self, task: TaskId) -> Result<Status> {
        let t = self.task(task)?;
        let s = if t.is_done() { Status::Todo } else { Status::Done };
        self.set_status(task, s)?;
        Ok(s)
    }

    pub fn set_priority(&mut self, task: TaskId, priority: i64) -> Result<()> {
        self.set_task(task, Change::TaskPriority(priority))
    }

    pub fn set_due(&mut self, task: TaskId, due: Option<jiff::civil::Date>) -> Result<()> {
        self.set_task(task, Change::TaskDue(due))
    }

    pub fn set_tag(&mut self, task: TaskId, tag: &str, on: bool) -> Result<()> {
        self.set_task(
            task,
            Change::TaskTag {
                tag: tag.to_owned(),
                on,
            },
        )
    }

    /// Make `task` require `blocker` (or remove that requirement). The
    /// blocker may be in another tab. Refuses links to itself, to its own
    /// ancestors (it could never become actionable) and links that would
    /// close a cycle.
    pub fn set_blocked_by(&mut self, task: TaskId, blocker: TaskId, on: bool) -> Result<()> {
        let mut tab = None;
        if on {
            let t = self.task(task)?;
            let b = self.task(blocker)?;
            if task == blocker {
                return Err(Error::Invalid("a task cannot require itself".into()));
            }
            if t.tab == b.tab && self.structure(t.tab)?.ancestors(task).contains(&blocker) {
                return Err(Error::Invalid("a task cannot require one of its parents".into()));
            }
            if self.requires_transitively(blocker, task)? {
                return Err(Error::Invalid("that would create a dependency cycle".into()));
            }
            tab = Some(b.tab);
        }
        self.set_task(task, Change::TaskBlockedBy { task: blocker, tab, on })
    }

    /// Does `from` require `to`, directly or transitively (across tabs)?
    pub fn requires_transitively(&self, from: TaskId, to: TaskId) -> Result<bool> {
        Ok(self.conn.query_row(
            "WITH RECURSIVE r(id) AS (SELECT ?1 UNION SELECT d.blocker FROM task_dep d JOIN r ON d.task = r.id)
             SELECT EXISTS(SELECT 1 FROM r WHERE id = ?2)",
            params![from.to_string(), to.to_string()],
            |r| r.get(0),
        )?)
    }

    /// Whether a task is done, looking at it and its ancestors: `None` when
    /// it is unknown, deleted, or under a deleted ancestor.
    pub fn task_done(&self, id: TaskId) -> Result<Option<bool>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT status, deleted, parent FROM task WHERE id = ?1")?;
        let mut cur = Some(id.to_string());
        let mut done = false;
        for _ in 0..1000 {
            let Some(c) = cur.take() else { break };
            let row: Option<(String, bool, Option<String>)> = st
                .query_row(params![c], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            match row {
                None if c == id.to_string() => return Ok(None),
                None => break, // unknown parent: shown at the root
                Some((_, true, _)) => return Ok(None),
                Some((status, false, parent)) => {
                    done |= status == "done";
                    cur = parent;
                }
            }
        }
        Ok(Some(done))
    }

    /// Record the state of blockers that live outside `tree` (other tabs).
    fn fill_external(&self, tree: &mut Tree) -> Result<()> {
        let outside: std::collections::BTreeSet<TaskId> = tree
            .iter()
            .flat_map(|t| t.blocked_by.iter().copied())
            .filter(|b| !tree.contains(*b))
            .collect();
        let mut m = std::collections::HashMap::new();
        for b in outside {
            if let Some(done) = self.task_done(b)? {
                m.insert(b, done);
            }
        }
        tree.set_external(m);
        tree.set_today(self.today());

        // Open tasks outside the tree that require tasks of the tree (other
        // tabs): their scores raise their blockers' scores.
        let Some(tab) = tree.iter().next().map(|t| t.tab) else {
            return Ok(());
        };
        let mut st = self.conn.prepare_cached(
            "SELECT d.blocker, d.task FROM task_dep d JOIN task b ON b.id = d.blocker WHERE b.tab = ?1",
        )?;
        let links: Vec<(String, String)> = st
            .query_map(params![tab.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut blocking: std::collections::HashMap<TaskId, i64> = Default::default();
        for (blocker, dependent) in links {
            let (blocker, dependent): (TaskId, TaskId) = (parse_id(&blocker)?, parse_id(&dependent)?);
            if tree.contains(blocker) && !tree.contains(dependent) {
                let s = self.total_score(dependent, &mut Default::default())?;
                *blocking.entry(blocker).or_default() += s;
            }
        }
        tree.set_external_blocking(blocking);
        Ok(())
    }

    /// Today's date in the local time zone, according to this repo's clock.
    pub fn today(&self) -> jiff::civil::Date {
        jiff::Timestamp::from_millisecond(self.now_ms())
            .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()).date())
            .unwrap_or_else(|_| crate::dates::today_local())
    }

    /// Total score of an open task computed from the database, following
    /// the tasks that require it across tabs (0 if done, deleted or unknown).
    fn total_score(&self, id: TaskId, seen: &mut std::collections::HashSet<TaskId>) -> Result<i64> {
        if !seen.insert(id) || self.task_done(id)? != Some(false) {
            return Ok(0);
        }
        let (priority, due): (i64, Option<String>) = self.conn.query_row(
            "SELECT priority, due FROM task WHERE id = ?1",
            params![id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut total = crate::score::own_score(priority, due.and_then(|d| d.parse().ok()), self.today()).total();
        let mut st = self
            .conn
            .prepare_cached("SELECT task FROM task_dep WHERE blocker = ?1")?;
        let deps: Vec<String> = st
            .query_map(params![id.to_string()], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for d in deps {
            total += self.total_score(parse_id(&d)?, seen)?;
        }
        Ok(total)
    }

    /// What `task` requires and what requires it, for display.
    pub fn dependencies(&self, task: TaskId) -> Result<Dependencies> {
        let tabs: std::collections::HashMap<TabId, String> = self.tabs()?.into_iter().map(|t| (t.id, t.name)).collect();
        let info = |id: TaskId, tab: Option<TabId>| -> Result<DepInfo> {
            let known = self.task(id).ok();
            let tab = known.as_ref().map(|t| t.tab).or(tab);
            let tab_name = tab.and_then(|t| tabs.get(&t).cloned());
            let state = match (&known, self.task_done(id)?) {
                (Some(_), Some(true)) => DepState::Done,
                (Some(_), Some(false)) => DepState::Todo,
                (Some(_), None) => DepState::Deleted,
                (None, _) if tab_name.is_some() => DepState::Missing,
                (None, _) => DepState::UnknownTab,
            };
            Ok(DepInfo {
                task: id,
                tab,
                tab_name,
                title: known.map(|t| t.title),
                state,
            })
        };
        let mut st = self
            .conn
            .prepare_cached("SELECT blocker, blocker_tab FROM task_dep WHERE task = ?1 ORDER BY blocker")?;
        let reqs: Vec<(String, Option<String>)> = st
            .query_map(params![task.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut requires = vec![];
        for (b, tab) in reqs {
            requires.push(info(parse_id(&b)?, opt_id(tab)?)?);
        }
        let mut st = self
            .conn
            .prepare_cached("SELECT task FROM task_dep WHERE blocker = ?1 ORDER BY task")?;
        let ids: Vec<String> = st
            .query_map(params![task.to_string()], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut blocks = vec![];
        for id in ids {
            let d = info(parse_id(&id)?, None)?;
            if d.state != DepState::Deleted {
                blocks.push(d);
            }
        }
        Ok(Dependencies { requires, blocks })
    }

    pub fn delete_task(&mut self, task: TaskId) -> Result<()> {
        self.set_task(task, Change::TaskDeleted(true))
    }

    pub fn undelete_task(&mut self, task: TaskId) -> Result<()> {
        self.set_task(task, Change::TaskDeleted(false))
    }

    // ------------------------------------------------------------- work log

    /// The open session of this device, if any.
    pub fn active_session(&self) -> Result<Option<Session>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {SESSION_COLS} FROM session WHERE device = ?1 AND end IS NULL AND deleted = 0
                     ORDER BY start DESC LIMIT 1"
                ),
                params![self.device.to_string()],
                session_from_row,
            )
            .optional()?)
    }

    fn open_sessions_of_device(&self) -> Result<Vec<Session>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {SESSION_COLS} FROM session WHERE device = ?1 AND end IS NULL AND deleted = 0"
        ))?;
        let rows = st.query_map(params![self.device.to_string()], session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Start working on `task`, closing any other open session of this device.
    /// No-op if already working on it. Returns the active session id.
    pub fn start_work(&mut self, task: TaskId) -> Result<SessionId> {
        let t = self.task(task)?;
        let now = self.now_ms();
        let mut edits = vec![];
        let mut current = None;
        for s in self.open_sessions_of_device()? {
            if s.task == Some(task) && current.is_none() {
                current = Some(s.id);
            } else {
                edits.push((s.tab, s.id.0, Change::SessionEnd(Some(now))));
            }
        }
        let id = match current {
            Some(id) => id,
            None => {
                let id = SessionId::new();
                edits.extend([
                    (t.tab, id.0, Change::SessionTask(task)),
                    (t.tab, id.0, Change::SessionDevice(self.device)),
                    (t.tab, id.0, Change::SessionUser(self.user.clone())),
                    (t.tab, id.0, Change::SessionStart(now)),
                ]);
                id
            }
        };
        self.commit(edits)?;
        Ok(id)
    }

    /// Stop all open sessions of this device. Returns the stopped sessions.
    pub fn stop_work(&mut self) -> Result<Vec<SessionId>> {
        let now = self.now_ms();
        let open = self.open_sessions_of_device()?;
        let ids = open.iter().map(|s| s.id).collect();
        self.commit(
            open.into_iter()
                .map(|s| (s.tab, s.id.0, Change::SessionEnd(Some(now))))
                .collect(),
        )?;
        Ok(ids)
    }

    /// Start working on `task` unless already doing so, in which case stop.
    /// Returns true if work is now running.
    pub fn toggle_work(&mut self, task: TaskId) -> Result<bool> {
        match self.active_session()? {
            Some(s) if s.task == Some(task) => {
                self.stop_work()?;
                Ok(false)
            }
            _ => {
                self.start_work(task)?;
                Ok(true)
            }
        }
    }

    pub fn set_session_note(&mut self, session: SessionId, note: &str) -> Result<()> {
        let s = self.session(session)?;
        self.edit1(s.tab, session, Change::SessionNote(note.to_owned()))
    }

    pub fn session(&self, id: SessionId) -> Result<Session> {
        self.conn
            .query_row(
                &format!("SELECT {SESSION_COLS} FROM session WHERE id = ?1"),
                params![id.to_string()],
                session_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("session {id}")))
    }

    /// Sessions of a task (not deleted), most recent first.
    pub fn sessions_for_task(&self, task: TaskId) -> Result<Vec<Session>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {SESSION_COLS} FROM session WHERE task = ?1 AND deleted = 0 ORDER BY start DESC"
        ))?;
        let rows = st.query_map(params![task.to_string()], session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Open sessions of any device in a tab (who is working on what).
    pub fn open_sessions(&self, tab: TabId) -> Result<Vec<Session>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {SESSION_COLS} FROM session WHERE tab = ?1 AND end IS NULL AND deleted = 0 AND start IS NOT NULL"
        ))?;
        let rows = st.query_map(params![tab.to_string()], session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Non-deleted sessions overlapping `[from, to)` (Unix ms), optionally in one tab.
    pub fn sessions_overlapping(&self, from: i64, to: i64, tab: Option<TabId>) -> Result<Vec<Session>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {SESSION_COLS} FROM session
             WHERE deleted = 0 AND start IS NOT NULL AND start < ?2 AND (end IS NULL OR end > ?1)
               AND (?3 IS NULL OR tab = ?3)
             ORDER BY start"
        ))?;
        let rows = st.query_map(params![from, to, tab.map(|t| t.to_string())], session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------------------------------------------------------------- reads

    pub fn tabs(&self) -> Result<Vec<TabInfo>> {
        let mut st = self
            .conn
            .prepare("SELECT id, name, remote FROM tab WHERE deleted = 0 ORDER BY id")?;
        let rows = st.query_map([], |r| {
            let remote: Option<String> = r.get(2)?;
            Ok(TabInfo {
                id: parse_id(&r.get::<_, String>(0)?)?,
                name: r.get(1)?,
                remote: remote.and_then(|s| serde_json::from_str(&s).ok()),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Find a tab by exact id, exact name, or unique case-insensitive name prefix.
    pub fn find_tab(&self, query: &str) -> Result<TabInfo> {
        let tabs = self.tabs()?;
        if let Some(t) = tabs.iter().find(|t| t.id.to_string() == query || t.name == query) {
            return Ok(t.clone());
        }
        let q = query.to_lowercase();
        let m: Vec<_> = tabs.iter().filter(|t| t.name.to_lowercase().starts_with(&q)).collect();
        match m.as_slice() {
            [t] => Ok((*t).clone()),
            [] => Err(Error::NotFound(format!("tab {query:?}"))),
            _ => Err(Error::Invalid(format!("ambiguous tab {query:?}"))),
        }
    }

    pub fn task(&self, id: TaskId) -> Result<Task> {
        self.conn
            .query_row(
                &format!("SELECT {TASK_COLS} FROM task t WHERE t.id = ?1"),
                params![id.to_string()],
                task_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("task {id}")))
    }

    /// All tasks of a tab, including deleted ones.
    pub fn tasks(&self, tab: TabId) -> Result<Vec<Task>> {
        let mut st = self
            .conn
            .prepare_cached(&format!("SELECT {TASK_COLS} FROM task t WHERE t.tab = ?1"))?;
        let rows = st.query_map(params![tab.to_string()], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn tree(&self, tab: TabId) -> Result<Tree> {
        let mut tree = Tree::build(self.tasks(tab)?);
        self.fill_external(&mut tree)?;
        Ok(tree)
    }

    /// The most prioritary actionable task of a tab (see [`Tree::next_task`]).
    /// Only loads the columns the ranking needs, so it stays fast on big tabs.
    pub fn next_task(&self, tab: TabId) -> Result<Option<Task>> {
        let tree = self.structure(tab)?;
        tree.next_task().map(|id| self.task(id)).transpose()
    }

    /// The tree of a tab without titles, notes and tags (fast; for ranking and
    /// structure queries).
    pub fn structure(&self, tab: TabId) -> Result<Tree> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, status, priority, due, parent, position, deleted,
                    (SELECT group_concat(blocker, ' ') FROM task_dep WHERE task = t.id)
                 FROM task t WHERE tab = ?1",
        )?;
        let rows = st.query_map(params![tab.to_string()], |r| {
            let status: String = r.get(1)?;
            let due: Option<String> = r.get(3)?;
            Ok(Task {
                id: parse_id(&r.get::<_, String>(0)?)?,
                tab,
                title: String::new(),
                notes: String::new(),
                status: Status::parse(&status).unwrap_or(Status::Todo),
                priority: r.get(2)?,
                due: due.and_then(|d| d.parse().ok()),
                parent: opt_id(r.get(4)?)?,
                position: r.get(5)?,
                deleted: r.get(6)?,
                tags: vec![],
                blocked_by: id_list(r.get(7)?)?,
            })
        })?;
        let mut tree = Tree::build(rows.collect::<rusqlite::Result<_>>()?);
        self.fill_external(&mut tree)?;
        Ok(tree)
    }

    // ----------------------------------------------------------------- sync

    /// This device's ops for `tab` not yet pushed, in seq order.
    pub fn unpushed_ops(&self, tab: TabId) -> Result<Vec<Op>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {OP_COLS} FROM op WHERE tab = ?1 AND pushed = 0 AND device = ?2 ORDER BY seq"
        ))?;
        let rows = st.query_map(params![tab.to_string(), self.device.to_string()], op_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn mark_pushed(&mut self, tab: TabId, up_to_seq: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE op SET pushed = 1 WHERE tab = ?1 AND device = ?2 AND seq <= ?3",
            params![tab.to_string(), self.device.to_string(), up_to_seq as i64],
        )?;
        Ok(())
    }

    /// Integrate ops received from elsewhere. Returns how many were new.
    pub fn ingest(&mut self, ops: &[Op]) -> Result<usize> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut clock = read_clock(&tx)?;
        let mut new = 0;
        for op in ops {
            if apply_op(&tx, op, true)? {
                new += 1;
            }
            clock.observe(&op.hlc);
        }
        tx.execute(
            "UPDATE device SET hlc_wall = ?1, hlc_counter = ?2",
            params![clock.wall, clock.counter],
        )?;
        tx.commit()?;
        Ok(new)
    }

    pub fn cursor(&self, tab: TabId, device: DeviceId) -> Result<u64> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_seq FROM remote_cursor WHERE tab = ?1 AND device = ?2",
                params![tab.to_string(), device.to_string()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u64)
    }

    pub fn set_cursor(&mut self, tab: TabId, device: DeviceId, last_seq: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO remote_cursor(tab, device, last_seq) VALUES (?1, ?2, ?3)
             ON CONFLICT(tab, device) DO UPDATE SET last_seq = max(last_seq, ?3)",
            params![tab.to_string(), device.to_string(), last_seq as i64],
        )?;
        Ok(())
    }

    /// All ops in HLC order (debugging, tests, rebuild).
    pub fn all_ops(&self) -> Result<Vec<Op>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {OP_COLS} FROM op ORDER BY hlc_wall, hlc_counter, device"
        ))?;
        let rows = st.query_map([], op_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Recompute the read model from the op log.
    pub fn rebuild(&mut self) -> Result<()> {
        let ops = self.all_ops()?;
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "DELETE FROM field_hlc; DELETE FROM task; DELETE FROM task_tag; DELETE FROM task_dep; DELETE FROM session;
             UPDATE tab SET name = '', deleted = 0;",
        )?;
        for op in &ops {
            apply_to_model(&tx, op)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// A canonical text dump of the read model (tests: convergence checks).
    pub fn dump_model(&self) -> Result<String> {
        let mut out = String::new();
        for (q, n) in [
            ("SELECT id, name, deleted FROM tab ORDER BY id", 3),
            (
                "SELECT id, tab, title, notes, status, priority, due, parent, position, deleted FROM task ORDER BY id",
                10,
            ),
            ("SELECT task, tag FROM task_tag ORDER BY task, tag", 2),
            ("SELECT task, blocker FROM task_dep ORDER BY task, blocker", 2),
            (
                "SELECT id, tab, task, device, user, start, end, note, deleted FROM session ORDER BY id",
                9,
            ),
        ] {
            let mut st = self.conn.prepare(q)?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                let cols: Vec<String> = (0..n)
                    .map(|i| format!("{:?}", r.get_ref(i).map(|v| v.to_owned_string()).unwrap_or_default()))
                    .collect();
                out.push_str(&cols.join("|"));
                out.push('\n');
            }
            out.push_str("--\n");
        }
        Ok(out)
    }
}

trait ValueRefExt {
    fn to_owned_string(&self) -> String;
}
impl ValueRefExt for rusqlite::types::ValueRef<'_> {
    fn to_owned_string(&self) -> String {
        use rusqlite::types::ValueRef::*;
        match self {
            Null => "NULL".into(),
            Integer(i) => i.to_string(),
            Real(f) => f.to_string(),
            Text(t) | Blob(t) => String::from_utf8_lossy(t).into_owned(),
        }
    }
}

fn between(a: Option<&str>, b: Option<&str>) -> String {
    match (a, b) {
        // Concurrent inserts can produce equal keys; fall back to an open bound.
        (Some(a), Some(b)) if a >= b => key_between(Some(a), None),
        _ => key_between(a, b),
    }
}

fn read_clock(conn: &Connection) -> Result<ClockState> {
    Ok(conn.query_row("SELECT hlc_wall, hlc_counter FROM device", [], |r| {
        Ok(ClockState {
            wall: r.get(0)?,
            counter: r.get(1)?,
        })
    })?)
}

fn default_user() -> String {
    std::env::var("CPASFAIT_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "me".into())
}
