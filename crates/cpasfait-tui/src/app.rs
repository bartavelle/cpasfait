//! Application state and behaviour (no terminal I/O here, so it is testable).

use crate::editor::{EditResult, LineEditor};
use crate::keymap::{self, Action, Lookup};
use crate::timebox::TimePeriod;
use anyhow::{Context, Result, anyhow, bail};
use cpasfait_core::backend;
use cpasfait_core::dates::{parse_date, today_local};
use cpasfait_core::sync::remote_tabs;
use cpasfait_core::{Place, RemoteConfig, Repo, Row, Session, TabId, TabInfo, TaskId, Tree};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use std::collections::HashSet;

pub enum Mode {
    Normal,
    /// Editing a task title. `new`: the task was just created (Esc on empty deletes it).
    Insert {
        task: TaskId,
        editor: LineEditor,
        new: bool,
    },
    Command(LineEditor),
    /// Editing a task's tags as a space-separated list.
    Tags {
        task: TaskId,
        editor: LineEditor,
    },
    Help,
}

pub struct Message {
    pub text: String,
    pub error: bool,
}

/// Requests the app makes to the outer loop (things needing the terminal or threads).
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    EditNotes(TaskId),
    Sync,
}

pub struct App {
    pub repo: Repo,
    pub tabs: Vec<TabInfo>,
    pub tab_idx: usize,
    pub tree: Tree,
    pub rows: Vec<Row>,
    pub list: ListState,
    pub collapsed: HashSet<TaskId>,
    pub hide_done: bool,
    pub show_detail: bool,
    /// The "time spent" box (lower right).
    pub time_period: TimePeriod,
    /// Time zone for day/week/month boundaries.
    pub tz: jiff::tz::TimeZone,
    pub mode: Mode,
    pub pending: String,
    pub message: Option<Message>,
    /// This device's running session.
    pub active: Option<Session>,
    /// Running sessions of all users in the current tab.
    pub open_sessions: Vec<Session>,
    /// Sessions of the selected task.
    pub detail_sessions: Vec<Session>,
    pub next: Option<TaskId>,
    pub sync_status: Option<String>,
    pub requests: Vec<Request>,
    pub quit: bool,
    deleted_stack: Vec<TaskId>,
    data_version: i64,
    /// Rows visible in the list (set by the renderer, for paging).
    pub page_height: usize,
}

impl App {
    pub fn new(mut repo: Repo) -> Result<App> {
        if repo.tabs()?.is_empty() {
            repo.create_tab("inbox")?;
        }
        let mut app = App {
            repo,
            tabs: vec![],
            tab_idx: 0,
            tree: Tree::default(),
            rows: vec![],
            list: ListState::default(),
            collapsed: HashSet::new(),
            hide_done: false,
            show_detail: true,
            time_period: TimePeriod::Hidden,
            tz: jiff::tz::TimeZone::system(),
            mode: Mode::Normal,
            pending: String::new(),
            message: None,
            active: None,
            open_sessions: vec![],
            detail_sessions: vec![],
            next: None,
            sync_status: None,
            requests: vec![],
            quit: false,
            deleted_stack: vec![],
            data_version: 0,
            page_height: 20,
        };
        app.refresh()?;
        Ok(app)
    }

    pub fn tab(&self) -> Option<&TabInfo> {
        self.tabs.get(self.tab_idx)
    }

    fn tab_id(&self) -> Result<TabId> {
        self.tab().map(|t| t.id).ok_or_else(|| anyhow!("no tab"))
    }

    pub fn selected(&self) -> Option<TaskId> {
        self.list.selected().and_then(|i| self.rows.get(i)).map(|r| r.id)
    }

    fn select(&mut self, id: TaskId) {
        // Make sure it is visible: unfold ancestors.
        for a in self.tree.ancestors(id) {
            self.collapsed.remove(&a);
        }
        self.rebuild_rows();
        if let Some(i) = self.rows.iter().position(|r| r.id == id) {
            self.list.select(Some(i));
        }
        self.refresh_detail();
    }

    pub fn info(&mut self, s: impl Into<String>) {
        self.message = Some(Message {
            text: s.into(),
            error: false,
        });
    }

    pub fn error(&mut self, s: impl Into<String>) {
        self.message = Some(Message {
            text: s.into(),
            error: true,
        });
    }

    /// Reload everything from the database, keeping the selection.
    pub fn refresh(&mut self) -> Result<()> {
        let keep = self.selected();
        let keep_tab = self.tab().map(|t| t.id);
        self.tabs = self.repo.tabs()?;
        if let Some(t) = keep_tab
            && let Some(i) = self.tabs.iter().position(|x| x.id == t)
        {
            self.tab_idx = i;
        }
        self.tab_idx = self.tab_idx.min(self.tabs.len().saturating_sub(1));
        match self.tab().map(|t| t.id) {
            Some(tab) => {
                self.tree = self.repo.tree(tab)?;
                self.open_sessions = self.repo.open_sessions(tab)?;
                self.next = self.tree.next_task();
            }
            None => {
                self.tree = Tree::default();
                self.open_sessions = vec![];
                self.next = None;
            }
        }
        self.active = self.repo.active_session()?;
        let old_idx = self.list.selected();
        self.rebuild_rows();
        let idx = keep
            .and_then(|k| self.rows.iter().position(|r| r.id == k))
            .or(old_idx.map(|i| i.min(self.rows.len().saturating_sub(1))))
            .or((!self.rows.is_empty()).then_some(0));
        self.list.select(idx.filter(|_| !self.rows.is_empty()));
        self.refresh_detail();
        self.data_version = self.repo.data_version()?;
        Ok(())
    }

    /// Refresh if another connection changed the database.
    pub fn refresh_if_changed(&mut self) -> Result<bool> {
        if self.repo.data_version()? != self.data_version {
            self.refresh()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn rebuild_rows(&mut self) {
        self.rows = self.tree.rows(&self.collapsed, self.hide_done);
    }

    fn refresh_detail(&mut self) {
        self.detail_sessions = self
            .selected()
            .map(|t| self.repo.sessions_for_task(t).unwrap_or_default())
            .unwrap_or_default();
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.list.selected().unwrap_or(0) as isize;
        let n = (cur + delta).clamp(0, self.rows.len() as isize - 1);
        self.list.select(Some(n as usize));
        self.refresh_detail();
    }

    // ------------------------------------------------------------ key input

    pub fn on_key(&mut self, key: KeyEvent) {
        if let Err(e) = self.on_key_inner(key) {
            self.error(format!("{e:#}"));
        }
    }

    fn on_key_inner(&mut self, key: KeyEvent) -> Result<()> {
        match &mut self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                Ok(())
            }
            Mode::Insert { editor, task, new } => {
                let (task, new) = (*task, *new);
                match editor.handle(key) {
                    EditResult::Continue => Ok(()),
                    // Like vim, Esc leaves insert mode keeping what was typed.
                    EditResult::Submit | EditResult::Cancel => {
                        let title = editor.text().trim().to_owned();
                        self.mode = Mode::Normal;
                        if title.is_empty() {
                            if new {
                                self.repo.delete_task(task)?;
                            }
                        } else if self.tree.get(task).is_none_or(|t| t.title != title) {
                            self.repo.set_title(task, &title)?;
                        }
                        self.refresh()
                    }
                }
            }
            Mode::Tags { editor, task } => {
                let task = *task;
                match editor.handle(key) {
                    EditResult::Continue => Ok(()),
                    EditResult::Cancel => {
                        self.mode = Mode::Normal;
                        Ok(())
                    }
                    EditResult::Submit => {
                        let text = editor.text().to_owned();
                        self.mode = Mode::Normal;
                        self.replace_tags(task, &text)
                    }
                }
            }
            Mode::Command(editor) => match editor.handle(key) {
                EditResult::Continue => {
                    if editor.text().is_empty() && key.code == KeyCode::Backspace {
                        self.mode = Mode::Normal;
                    }
                    Ok(())
                }
                EditResult::Submit => {
                    let cmd = editor.text().to_owned();
                    self.mode = Mode::Normal;
                    self.run_command(&cmd)
                }
                EditResult::Cancel => {
                    self.mode = Mode::Normal;
                    Ok(())
                }
            },
            Mode::Normal => {
                if key.code == KeyCode::Esc {
                    self.pending.clear();
                    self.message = None;
                    return Ok(());
                }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.pending.clear();
                    return Ok(());
                }
                let Some(name) = keymap::key_name(key) else {
                    return Ok(());
                };
                self.pending.push_str(&name);
                match keymap::lookup(&self.pending) {
                    Lookup::Pending => Ok(()),
                    Lookup::None => {
                        self.pending.clear();
                        Ok(())
                    }
                    Lookup::Action(a) => {
                        self.pending.clear();
                        self.message = None;
                        self.run(a)
                    }
                }
            }
        }
    }

    fn start_tags(&mut self, task: TaskId) {
        let current = self.tree.get(task).map(|t| t.tags.join(" ")).unwrap_or_default();
        let text = if current.is_empty() {
            current
        } else {
            format!("{current} ")
        };
        self.mode = Mode::Tags {
            task,
            editor: LineEditor::new(&text),
        };
    }

    /// Set the tags of `task` to the words of `text` (`#` prefixes allowed).
    fn replace_tags(&mut self, task: TaskId, text: &str) -> Result<()> {
        let wanted: std::collections::BTreeSet<&str> = text
            .split([' ', ','])
            .map(|w| w.trim_start_matches(['#', '+']))
            .filter(|w| !w.is_empty())
            .collect();
        let current: Vec<String> = self.tree.get(task).map(|t| t.tags.clone()).unwrap_or_default();
        for old in &current {
            if !wanted.contains(old.as_str()) {
                self.repo.set_tag(task, old, false)?;
            }
        }
        for new in &wanted {
            if !current.iter().any(|c| c == new) {
                self.repo.set_tag(task, new, true)?;
            }
        }
        self.refresh()
    }

    fn start_insert(&mut self, task: TaskId, text: &str, new: bool) {
        self.mode = Mode::Insert {
            task,
            editor: LineEditor::new(text),
            new,
        };
    }

    fn add(&mut self, place: impl FnOnce(Option<TaskId>) -> Place) -> Result<()> {
        let tab = self.tab_id()?;
        let id = self.repo.add_task(tab, place(self.selected()), "")?;
        self.refresh()?;
        self.select(id);
        self.start_insert(id, "", true);
        Ok(())
    }

    pub fn run(&mut self, action: Action) -> Result<()> {
        use Action::*;
        let sel = self.selected();
        match action {
            Down => self.move_cursor(1),
            Up => self.move_cursor(-1),
            Top => self.move_cursor(isize::MIN / 2),
            Bottom => self.move_cursor(isize::MAX / 2),
            PageDown => self.move_cursor((self.page_height / 2).max(1) as isize),
            PageUp => self.move_cursor(-((self.page_height / 2).max(1) as isize)),
            CollapseOrParent => {
                if let Some(id) = sel {
                    let row = &self.rows[self.list.selected().unwrap_or(0)];
                    if row.has_children && !self.collapsed.contains(&id) {
                        self.collapsed.insert(id);
                        self.rebuild_rows();
                    } else if let Some(p) = self.tree.parent(id) {
                        self.select(p);
                    }
                }
            }
            ExpandOrChild => {
                if let Some(id) = sel {
                    if self.collapsed.remove(&id) {
                        self.rebuild_rows();
                    } else if self
                        .rows
                        .get(self.list.selected().unwrap_or(0))
                        .is_some_and(|r| r.has_children)
                    {
                        self.move_cursor(1);
                    }
                }
            }
            ToggleFold => {
                if let Some(id) = sel {
                    if !self.collapsed.remove(&id) {
                        self.collapsed.insert(id);
                    }
                    self.rebuild_rows();
                }
            }
            NextTab | PrevTab => {
                if !self.tabs.is_empty() {
                    let n = self.tabs.len();
                    self.tab_idx = if action == NextTab {
                        (self.tab_idx + 1) % n
                    } else {
                        (self.tab_idx + n - 1) % n
                    };
                    self.switch_tab()?;
                }
            }
            GotoTab(i) => {
                if i < self.tabs.len() {
                    self.tab_idx = i;
                    self.switch_tab()?;
                }
            }
            AddBelow => self.add(|sel| match sel {
                Some(s) => Place::After(s),
                None => Place::LastChildOf(None),
            })?,
            AddAbove => self.add(|sel| match sel {
                Some(s) => Place::Before(s),
                None => Place::FirstChildOf(None),
            })?,
            AddChild => {
                if let Some(s) = sel {
                    self.collapsed.remove(&s);
                }
                self.add(Place::LastChildOf)?
            }
            EditTitle | ReplaceTitle => {
                if let Some(id) = sel {
                    let title = if action == EditTitle {
                        self.tree.get(id).map(|t| t.title.clone()).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    self.start_insert(id, &title, false);
                }
            }
            EditNotes => {
                if let Some(id) = sel {
                    self.requests.push(Request::EditNotes(id));
                }
            }
            ToggleDone => {
                if let Some(id) = sel {
                    self.repo.toggle_done(id)?;
                    self.refresh()?;
                    if self.hide_done {
                        // The task may have disappeared; stay at the same index.
                        self.move_cursor(0);
                    }
                }
            }
            Delete => {
                if let Some(id) = sel {
                    let title = self.tree.get(id).map(|t| t.title.clone()).unwrap_or_default();
                    self.repo.delete_task(id)?;
                    self.deleted_stack.push(id);
                    self.refresh()?;
                    self.info(format!("deleted {title:?} (u to undo)"));
                }
            }
            UndoDelete => match self.deleted_stack.pop() {
                Some(id) => {
                    self.repo.undelete_task(id)?;
                    self.refresh()?;
                    self.select(id);
                    self.info("restored");
                }
                None => self.info("nothing to undo"),
            },
            Indent | Outdent | MoveUp | MoveDown => {
                if let Some(id) = sel {
                    let moved = match action {
                        Indent => {
                            let r = self.repo.indent(id)?;
                            if let Some(p) = self.repo.task(id)?.parent {
                                self.collapsed.remove(&p);
                            }
                            r
                        }
                        Outdent => self.repo.outdent(id)?,
                        MoveUp => self.repo.shift(id, true)?,
                        _ => self.repo.shift(id, false)?,
                    };
                    if moved {
                        self.refresh()?;
                        self.select(id);
                    }
                }
            }
            PriorityUp | PriorityDown => {
                if let Some(id) = sel {
                    let p = self.repo.task(id)?.priority;
                    let np = if action == PriorityUp {
                        (p + 1).min(9)
                    } else {
                        (p - 1).max(0)
                    };
                    if np != p {
                        self.repo.set_priority(id, np)?;
                        self.refresh()?;
                    }
                }
            }
            ToggleWork => {
                if let Some(id) = sel {
                    let running = self.repo.toggle_work(id)?;
                    self.refresh()?;
                    let title = self.tree.get(id).map(|t| t.title.clone()).unwrap_or_default();
                    self.info(if running {
                        format!("working on {title:?}")
                    } else {
                        format!("stopped {title:?}")
                    });
                }
            }
            StopWork => {
                let stopped = self.repo.stop_work()?;
                self.refresh()?;
                self.info(if stopped.is_empty() {
                    "nothing running"
                } else {
                    "stopped"
                });
            }
            // `select` unfolds the ancestors; they can't be done (see `Tree::next_task`).
            JumpNext => match self.next {
                Some(id) => self.select(id),
                None => self.info("nothing to do in this tab 🎉"),
            },
            JumpActive => match self.active.as_ref().and_then(|s| s.task) {
                Some(task) => {
                    let tab = self.active.as_ref().map(|s| s.tab);
                    if let Some(i) = self.tabs.iter().position(|t| Some(t.id) == tab)
                        && i != self.tab_idx
                    {
                        self.tab_idx = i;
                        self.switch_tab()?;
                    }
                    if self.tree.contains(task) {
                        self.select(task);
                    } else {
                        self.info("the task being worked on is not visible");
                    }
                }
                None => self.info("not working on anything"),
            },
            ToggleHideDone => {
                self.hide_done = !self.hide_done;
                self.rebuild_rows();
                self.move_cursor(0);
                self.info(if self.hide_done {
                    "hiding done tasks"
                } else {
                    "showing done tasks"
                });
            }
            ToggleDetail => self.show_detail = !self.show_detail,
            CycleTimeBox => {
                self.time_period = self.time_period.next();
                self.info(format!("time spent: {}", self.time_period.label()));
            }
            Command => self.mode = Mode::Command(LineEditor::default()),
            EditTags => match self.selected() {
                Some(t) => self.start_tags(t),
                None => self.info("no task selected"),
            },
            Help => self.mode = Mode::Help,
            Quit => self.quit = true,
        }
        Ok(())
    }

    fn switch_tab(&mut self) -> Result<()> {
        self.list.select(None);
        self.collapsed.clear();
        self.refresh()?;
        self.list.select((!self.rows.is_empty()).then_some(0));
        self.refresh_detail();
        Ok(())
    }

    // -------------------------------------------------------------- commands

    pub fn run_command(&mut self, line: &str) -> Result<()> {
        let line = line.trim();
        let (cmd, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let arg = arg.trim();
        let need_task = |app: &App| app.selected().ok_or_else(|| anyhow!("no task selected"));
        match cmd {
            "" => {}
            "q" | "quit" | "q!" | "wq" | "x" => self.quit = true,
            "w" => self.info("changes are saved automatically"),
            "h" | "help" => self.mode = Mode::Help,
            "due" => {
                let t = need_task(self)?;
                let due = match arg {
                    "" | "none" | "-" => None,
                    s => Some(parse_date(s, today_local()).ok_or_else(|| anyhow!("can't parse date {s:?}"))?),
                };
                self.repo.set_due(t, due)?;
                self.refresh()?;
            }
            "tag" | "tags" => {
                let t = need_task(self)?;
                if arg.is_empty() {
                    self.start_tags(t);
                    return Ok(());
                }
                for w in arg.split_whitespace() {
                    let (on, name) = match w.strip_prefix('-') {
                        Some(n) => (false, n),
                        None => (true, w.trim_start_matches(['+', '#'])),
                    };
                    let name = name.trim_start_matches('#');
                    if !name.is_empty() {
                        self.repo.set_tag(t, name, on)?;
                    }
                }
                self.refresh()?;
            }
            "prio" | "priority" => {
                let t = need_task(self)?;
                let p: i64 = arg.parse().context("usage: :prio N")?;
                self.repo.set_priority(t, p.clamp(0, 9))?;
                self.refresh()?;
            }
            "title" => {
                let t = need_task(self)?;
                if arg.is_empty() {
                    bail!("usage: :title TEXT");
                }
                self.repo.set_title(t, arg)?;
                self.refresh()?;
            }
            "worknote" => {
                let session = match &self.active {
                    Some(s) => s.id,
                    None => self
                        .detail_sessions
                        .first()
                        .map(|s| s.id)
                        .ok_or_else(|| anyhow!("no running session and no session on this task"))?,
                };
                self.repo.set_session_note(session, arg)?;
                self.refresh()?;
            }
            "user" => {
                if arg.is_empty() {
                    let u = self.repo.user().to_owned();
                    self.info(format!("user: {u}"));
                } else {
                    self.repo.set_user(arg)?;
                    self.info(format!("user set to {arg}"));
                }
            }
            "tabnew" => {
                if arg.is_empty() {
                    bail!("usage: :tabnew NAME");
                }
                let id = self.repo.create_tab(arg)?;
                self.refresh()?;
                self.tab_idx = self.tabs.iter().position(|t| t.id == id).unwrap_or(0);
                self.switch_tab()?;
            }
            "tabrename" => {
                if arg.is_empty() {
                    bail!("usage: :tabrename NAME");
                }
                let tab = self.tab_id()?;
                self.repo.rename_tab(tab, arg)?;
                self.refresh()?;
            }
            "tabclose" => bail!("this deletes the tab for everyone sharing it: use :tabclose! to confirm"),
            "tabclose!" => {
                let tab = self.tab_id()?;
                self.repo.delete_tab(tab)?;
                self.refresh()?;
                self.switch_tab()?;
            }
            "tabremote" => {
                let tab = self.tab_id()?;
                match arg {
                    "" => {
                        let r = self.tab().and_then(|t| t.remote.clone());
                        self.info(match r {
                            Some(r) => format!("remote: {r}"),
                            None => "no remote (use :tabremote DIR|gdrive:FOLDER-ID)".into(),
                        });
                    }
                    "none" => {
                        self.repo.set_remote(tab, None)?;
                        self.refresh()?;
                        self.info("remote removed");
                    }
                    spec => {
                        self.repo.set_remote(tab, Some(&parse_remote(spec)?))?;
                        self.refresh()?;
                        self.requests.push(Request::Sync);
                        self.info("remote set, syncing");
                    }
                }
            }
            "tabjoin" => {
                let mut parts = arg.split_whitespace();
                let dir = parts
                    .next()
                    .ok_or_else(|| anyhow!("usage: :tabjoin DIR|gdrive:FOLDER-ID [TAB-ID]"))?;
                let config = parse_remote(dir)?;
                let tab: TabId = match parts.next() {
                    Some(id) => id.parse().context("bad tab id")?,
                    None => {
                        let b = backend::open(&config)?;
                        let known: HashSet<TabId> = self.tabs.iter().map(|t| t.id).collect();
                        let ids: Vec<TabId> = remote_tabs(b.as_ref())?
                            .into_iter()
                            .filter(|t| !known.contains(t))
                            .collect();
                        match ids.as_slice() {
                            [one] => *one,
                            [] => bail!("no new tab found in {dir}"),
                            many => bail!(
                                "several tabs in {dir}, pick one: {}",
                                many.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" ")
                            ),
                        }
                    }
                };
                self.repo.add_known_tab(tab)?;
                self.repo.set_remote(tab, Some(&config))?;
                cpasfait_core::sync::sync_tab(&mut self.repo, tab, backend::open(&config)?.as_ref())?;
                self.refresh()?;
                if let Some(i) = self.tabs.iter().position(|t| t.id == tab) {
                    self.tab_idx = i;
                    self.switch_tab()?;
                }
                self.info("joined");
            }
            "sync" => {
                self.requests.push(Request::Sync);
                self.info("syncing…");
            }
            "rebuild" => {
                self.repo.rebuild()?;
                self.refresh()?;
                self.info("read model rebuilt from the op log");
            }
            other => bail!("unknown command :{other} (see :help)"),
        }
        Ok(())
    }
}

/// Parse a remote spec (`gdrive:ID`, a Drive folder URL, or a directory made absolute).
fn parse_remote(spec: &str) -> Result<RemoteConfig> {
    Ok(match RemoteConfig::parse(spec).map_err(|e| anyhow!(e))? {
        RemoteConfig::LocalDir { path } => RemoteConfig::LocalDir {
            path: std::path::absolute(path)?.to_string_lossy().into_owned(),
        },
        other => other,
    })
}
