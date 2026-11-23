//! Application state and behaviour (no terminal I/O here, so it is testable).

use crate::editor::{EditResult, LineEditor};
use crate::keymap::{self, Action, Lookup};
use crate::search::{Purpose, Search};
use crate::timebox::TimePeriod;
use anyhow::{Context, Result, anyhow, bail};
use cpasfait_core::backend;
use cpasfait_core::crypto::Identity;
use cpasfait_core::dates::{parse_date, today_local};
use cpasfait_core::report::fmt_duration;
use cpasfait_core::score::{PRIORITY_MAX, PRIORITY_MIN};
use cpasfait_core::sync::{invited_to, remote_tabs, sync_tab};
use cpasfait_core::taskfile;
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
    /// The Ctrl-P task finder.
    Search(Box<Search>),
    /// Editing a task's tags as a space-separated list.
    Tags {
        task: TaskId,
        editor: LineEditor,
    },
    /// The help screen. `scroll`: first line shown; `page`: lines visible
    /// (both kept in range by the renderer).
    Help {
        scroll: u16,
        page: u16,
    },
}

pub struct Message {
    pub text: String,
    pub error: bool,
}

/// Requests the app makes to the outer loop (things needing the terminal or threads).
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    EditNotes(TaskId),
    /// Title and work log in the editor (see [`App::task_file`]).
    EditTask(TaskId),
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
    /// Scores of the open tasks of the current tab.
    pub scores: std::collections::HashMap<TaskId, cpasfait_core::score::Score>,
    /// Requirements of the selected task and what it blocks.
    pub detail_deps: cpasfait_core::Dependencies,
    pub next: Option<TaskId>,
    pub sync_status: Option<String>,
    /// This user's secret key, loaded (or created) on first need.
    pub identity: Option<Identity>,
    /// Remote moves already announced in the message line.
    notified_moves: HashSet<String>,
    pub requests: Vec<Request>,
    /// A task file whose changes could not be applied, offered again on the
    /// next edit of that task.
    failed_edit: Option<(TaskId, String)>,
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
            scores: Default::default(),
            detail_deps: Default::default(),
            next: None,
            sync_status: None,
            identity: None,
            notified_moves: HashSet::new(),
            requests: vec![],
            failed_edit: None,
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

    /// Load the identity, creating one if there is none. Returns a note when
    /// it was just created.
    fn ensure_identity(&mut self) -> Result<Option<String>> {
        if self.identity.is_some() {
            return Ok(None);
        }
        let (id, created) = Identity::load_or_create_default()?;
        let note = created.then(|| {
            format!(
                "created your key in {}",
                id.path().map(|p| p.display().to_string()).unwrap_or_default()
            )
        });
        self.identity = Some(id);
        Ok(note)
    }

    fn me(&self) -> &Identity {
        self.identity.as_ref().expect("ensure_identity first")
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
        if let Some(m) = self.tab().and_then(|t| t.pending_move.clone())
            && self.notified_moves.insert(m.id.to_string())
        {
            self.info(format!(
                "{} moved this tab to {}: :tabaccept to follow (you can edit the remote first)",
                m.by, m.remote
            ));
        }
        match self.tab().map(|t| t.id) {
            Some(tab) => {
                self.tree = self.repo.tree(tab)?;
                self.open_sessions = self.repo.open_sessions(tab)?;
                self.next = self.tree.next_task();
                self.scores = self.tree.scores();
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
        self.detail_deps = self
            .selected()
            .and_then(|t| self.repo.dependencies(t).ok())
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
            Mode::Help { scroll, page } => {
                let half = (*page / 2).max(1);
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
                    KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                    KeyCode::Char('d') if ctrl => *scroll = scroll.saturating_add(half),
                    KeyCode::Char('u') if ctrl => *scroll = scroll.saturating_sub(half),
                    KeyCode::Char('f') if ctrl => *scroll = scroll.saturating_add(*page),
                    KeyCode::Char('b') if ctrl => *scroll = scroll.saturating_sub(*page),
                    KeyCode::PageDown | KeyCode::Char(' ') => *scroll = scroll.saturating_add(*page),
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(*page),
                    KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
                    KeyCode::Char('G') | KeyCode::End => *scroll = u16::MAX,
                    KeyCode::Char('q' | '?') | KeyCode::Esc | KeyCode::Enter => self.mode = Mode::Normal,
                    _ => {}
                }
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
            Mode::Search(search) => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Up | KeyCode::BackTab => search.move_selection(-1),
                    KeyCode::Char('p' | 'k') if ctrl => search.move_selection(-1),
                    KeyCode::Down | KeyCode::Tab => search.move_selection(1),
                    KeyCode::Char('n' | 'j') if ctrl => search.move_selection(1),
                    _ => {
                        let before = search.query().to_owned();
                        match search.editor.handle(key) {
                            EditResult::Continue => {
                                if search.query() != before {
                                    search.update();
                                }
                            }
                            EditResult::Cancel => self.mode = Mode::Normal,
                            EditResult::Submit => {
                                let chosen = search.chosen().map(|c| (c.tab, c.id));
                                let purpose = search.purpose;
                                self.mode = Mode::Normal;
                                if let Some((tab, id)) = chosen {
                                    self.choose(purpose, tab, id)?;
                                }
                            }
                        }
                    }
                }
                Ok(())
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

    fn open_search(&mut self, purpose: Purpose, query: &str) -> Result<()> {
        let s = Search::new(&self.repo, self.tab_id().ok(), purpose, query)?;
        if purpose != Purpose::Go && s.candidates.is_empty() {
            match purpose {
                Purpose::Unrequire(_) => bail!("this task has no requirement"),
                Purpose::Unblock(_) => bail!("this task blocks nothing"),
                _ => bail!("no other task in this tab"),
            }
        }
        self.mode = Mode::Search(Box::new(s));
        Ok(())
    }

    /// Act on a task chosen in the finder.
    fn choose(&mut self, purpose: Purpose, tab: TabId, id: TaskId) -> Result<()> {
        let title = |app: &App, t: TaskId| app.tree.get(t).map(|t| t.title.clone()).unwrap_or_default();
        let (task, blocker, on) = match purpose {
            Purpose::Go => return self.go_to_task(tab, id),
            Purpose::Require(t) => (t, id, true),
            Purpose::Block(t) => (id, t, true),
            Purpose::Unrequire(t) => (t, id, false),
            Purpose::Unblock(t) => (id, t, false),
        };
        self.repo.set_blocked_by(task, blocker, on)?;
        let msg = format!(
            "“{}” {} “{}”",
            title(self, task),
            if on { "now requires" } else { "no longer requires" },
            title(self, blocker)
        );
        self.refresh()?;
        self.info(msg);
        Ok(())
    }

    /// Switch to `tab` if needed and select `id`, unfolding and un-hiding as needed.
    fn go_to_task(&mut self, tab: TabId, id: TaskId) -> Result<()> {
        if self.tab_id().ok() != Some(tab) {
            let Some(i) = self.tabs.iter().position(|t| t.id == tab) else {
                bail!("tab not found");
            };
            self.tab_idx = i;
            self.switch_tab()?;
        }
        if !self.tree.contains(id) {
            bail!("task not found");
        }
        self.select(id);
        if self.selected() != Some(id) && self.hide_done {
            self.hide_done = false;
            self.select(id);
            self.info("showing done tasks (H to hide them again)");
        }
        Ok(())
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
            AddSibling => {
                // Its place among the siblings follows from its score.
                let parent = sel.and_then(|s| self.tree.parent(s));
                self.add(|_| Place::LastChildOf(parent))?
            }
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
            EditTask => {
                if let Some(id) = sel {
                    self.requests.push(Request::EditTask(id));
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
            Indent | Outdent => {
                if let Some(id) = sel {
                    let moved = match action {
                        Indent => {
                            let r = self.repo.indent(id)?;
                            if let Some(p) = self.repo.task(id)?.parent {
                                self.collapsed.remove(&p);
                            }
                            r
                        }
                        _ => self.repo.outdent(id)?,
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
                    let np = if action == PriorityUp { p + 1 } else { p - 1 }.clamp(PRIORITY_MIN, PRIORITY_MAX);
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
            // Both open the command line, prefilled with the current time, to edit.
            StopWork => {
                if self.active.is_none() {
                    self.info("nothing running");
                } else {
                    let now = self.fmt_time(self.repo.now_ms());
                    self.mode = Mode::Command(LineEditor::new(&format!("stop {now}")));
                }
            }
            SwitchWork => {
                if sel.is_some() {
                    let now = self.fmt_time(self.repo.now_ms());
                    self.mode = Mode::Command(LineEditor::new(&format!("switch {now}")));
                }
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
            Search => self.open_search(Purpose::Go, "")?,
            EditTags => match self.selected() {
                Some(t) => self.start_tags(t),
                None => self.info("no task selected"),
            },
            Help => self.mode = Mode::Help { scroll: 0, page: 0 },
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

    /// A moment as shown to (and typed by) the user: `14:30`, or with the
    /// date when not today.
    fn fmt_time(&self, ms: i64) -> String {
        let Ok(t) = jiff::Timestamp::from_millisecond(ms) else {
            return ms.to_string();
        };
        let z = t.to_zoned_datetime(self.tz.clone());
        let today = jiff::Timestamp::from_millisecond(self.repo.now_ms())
            .map(|n| n.to_zoned_datetime(self.tz.clone()).date())
            .ok();
        if Some(z.date()) == today {
            z.strftime("%H:%M").to_string()
        } else {
            z.strftime("%Y-%m-%d %H:%M").to_string()
        }
    }

    /// A time typed after `:stop` / `:switch` (empty: now).
    fn parse_when(&self, arg: &str) -> Result<i64> {
        if arg.is_empty() {
            return Ok(self.repo.now_ms());
        }
        let at = cpasfait_core::dates::parse_time(arg, self.repo.now_ms(), &self.tz)
            .ok_or_else(|| anyhow!("can't parse time {arg:?} (14:30, -20m, yesterday 18:00)"))?;
        if at > self.repo.now_ms() {
            bail!("{} is in the future", self.fmt_time(at));
        }
        Ok(at)
    }

    fn check_after_start(&self, s: &Session, at: i64, title: &str) -> Result<()> {
        match s.start {
            Some(start) if at < start => bail!(
                "{} is before {title:?} started ({})",
                self.fmt_time(at),
                self.fmt_time(start)
            ),
            _ => Ok(()),
        }
    }

    fn session_title(&self, s: &Session) -> String {
        s.task
            .and_then(|t| self.repo.task(t).ok())
            .map(|t| t.title)
            .unwrap_or_else(|| "(unknown task)".into())
    }

    /// The text to edit for `task`: its title and work log, or the last
    /// edit of it that failed.
    pub fn task_file(&mut self, task: TaskId) -> Result<String> {
        match self.failed_edit.take() {
            Some((t, text)) if t == task => Ok(text),
            _ => Ok(taskfile::render(&self.repo, task, &self.tz)?),
        }
    }

    /// Apply an edited task file (an error: the editor failed, drop it). On
    /// error, the text is kept for the next edit, with the error on top.
    pub fn finish_task_edit(&mut self, task: TaskId, text: std::result::Result<String, String>) {
        let text = match text {
            Ok(t) => t,
            Err(e) => {
                self.failed_edit = None;
                return self.error(format!("{e}: nothing changed"));
            }
        };
        let res = taskfile::parse(&text, &self.tz).and_then(|f| match f {
            Some(f) => taskfile::apply(&mut self.repo, task, &f).map(Some),
            None => Ok(None),
        });
        match res {
            Ok(None) => self.info("empty file: nothing changed"),
            Ok(Some(done)) => {
                if let Err(e) = self.refresh() {
                    return self.error(format!("{e:#}"));
                }
                self.info(done.to_string());
            }
            Err(e) => {
                let e = match e {
                    cpasfait_core::Error::Invalid(m) => m,
                    e => e.to_string(),
                };
                let body: String = text
                    .lines()
                    .filter(|l| !l.starts_with("# error: "))
                    .map(|l| format!("{l}\n"))
                    .collect();
                self.failed_edit = Some((task, format!("# error: {e}\n{body}")));
                self.error(format!("{e}: nothing changed, E to fix it"));
            }
        }
    }

    pub fn run_command(&mut self, line: &str) -> Result<()> {
        let line = line.trim();
        let (cmd, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let arg = arg.trim();
        let need_task = |app: &App| app.selected().ok_or_else(|| anyhow!("no task selected"));
        match cmd {
            "" => {}
            "q" | "quit" | "q!" | "wq" | "x" => self.quit = true,
            "w" => self.info("changes are saved automatically"),
            "h" | "help" => self.mode = Mode::Help { scroll: 0, page: 0 },
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
            "require" | "requires" | "req" | "block" | "blocks" | "unrequire" | "unblock" => {
                let t = need_task(self)?;
                let purpose = match cmd {
                    "block" | "blocks" => Purpose::Block(t),
                    "unrequire" => Purpose::Unrequire(t),
                    "unblock" => Purpose::Unblock(t),
                    _ => Purpose::Require(t),
                };
                self.open_search(purpose, arg)?;
            }
            "prio" | "priority" => {
                let t = need_task(self)?;
                let p: i64 = arg
                    .parse()
                    .with_context(|| format!("usage: :prio N ({PRIORITY_MIN} to {PRIORITY_MAX})"))?;
                self.repo.set_priority(t, p)?;
                self.refresh()?;
            }
            "edit" => {
                let t = need_task(self)?;
                self.requests.push(Request::EditTask(t));
            }
            "title" => {
                let t = need_task(self)?;
                if arg.is_empty() {
                    bail!("usage: :title TEXT");
                }
                self.repo.set_title(t, arg)?;
                self.refresh()?;
            }
            "stop" => {
                let at = self.parse_when(arg)?;
                let Some(s) = self.repo.active_session()? else {
                    bail!("nothing running");
                };
                let title = self.session_title(&s);
                self.check_after_start(&s, at, &title)?;
                self.repo.stop_work_at(at)?;
                self.refresh()?;
                let worked = fmt_duration(at - s.start.unwrap_or(at));
                self.info(format!("stopped {title:?} at {} ({worked})", self.fmt_time(at)));
            }
            "switch" => {
                let task = need_task(self)?;
                let at = self.parse_when(arg)?;
                let title = self.tree.get(task).map(|t| t.title.clone()).unwrap_or_default();
                let previous = self.repo.active_session()?;
                let mut msg = String::new();
                if let Some(s) = &previous {
                    if s.task == Some(task) {
                        bail!("already working on {title:?}");
                    }
                    let prev = self.session_title(s);
                    self.check_after_start(s, at, &prev)?;
                    msg = format!(", {prev:?} stopped ({})", fmt_duration(at - s.start.unwrap_or(at)));
                }
                if previous.is_none()
                    && let Some(end) = self.repo.last_work_end()?
                    && at < end
                {
                    bail!(
                        "{} is before your last session ended ({})",
                        self.fmt_time(at),
                        self.fmt_time(end)
                    );
                }
                self.repo.start_work_at(task, at)?;
                self.refresh()?;
                self.info(format!("working on {title:?} since {}{msg}", self.fmt_time(at)));
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
                            None => "no remote (use :tabremote DIR|gdrive:FOLDER-ID|rsync:HOST:PATH)".into(),
                        });
                    }
                    "none" => {
                        self.repo.set_remote(tab, None)?;
                        self.refresh()?;
                        self.info("remote removed");
                    }
                    spec => {
                        let config = parse_remote(spec)?;
                        let note = self.ensure_identity()?;
                        let me = self.me().recipient().to_owned();
                        self.repo.share_tab(tab, &me)?;
                        self.repo.set_remote(tab, Some(&config))?;
                        self.refresh()?;
                        self.requests.push(Request::Sync);
                        let msg = "remote set (end-to-end encrypted), syncing; add members with :tabinvite KEY";
                        self.info(match note {
                            Some(n) => format!("{n}; {msg}"),
                            None => msg.into(),
                        });
                    }
                }
            }
            "tabjoin" => {
                let mut parts = arg.split_whitespace();
                let dir = parts
                    .next()
                    .ok_or_else(|| anyhow!("usage: :tabjoin DIR|gdrive:FOLDER-ID|rsync:HOST:PATH [TAB-ID]"))?;
                let config = parse_remote(dir)?;
                self.ensure_identity()?;
                let b = backend::open(&config)?;
                let tab: TabId = match parts.next() {
                    Some(id) => id.parse().context("bad tab id")?,
                    None => {
                        let known: HashSet<TabId> = self.tabs.iter().map(|t| t.id).collect();
                        let mut ids: Vec<TabId> = remote_tabs(b.as_ref())?
                            .into_iter()
                            .filter(|t| !known.contains(t))
                            .collect();
                        if ids.len() > 1 {
                            let mut invited = vec![];
                            for t in &ids {
                                if invited_to(b.as_ref(), *t, self.me())? {
                                    invited.push(*t);
                                }
                            }
                            if !invited.is_empty() {
                                ids = invited;
                            }
                        }
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
                let res = sync_tab(&mut self.repo, tab, b.as_ref(), self.identity.as_ref().expect("loaded"));
                self.refresh()?;
                if let Some(i) = self.tabs.iter().position(|t| t.id == tab) {
                    self.tab_idx = i;
                    self.switch_tab()?;
                }
                // Joined where a move pointed to: nothing left to follow.
                if let Some(m) = self.tab().and_then(|t| t.pending_move.clone())
                    && m.remote == config.spec()
                {
                    self.repo.mark_move_done(tab, m.id)?;
                    self.refresh()?;
                }
                match res {
                    Ok(_) => self.info("joined"),
                    Err(cpasfait_core::Error::NotInvited(key)) => self.info(format!(
                        "joined, waiting for an invitation: ask a member to run :tabinvite {key}"
                    )),
                    Err(e) => return Err(e.into()),
                }
            }
            "tabmove" => {
                let tab = self.tab_id()?;
                let old = self
                    .tab()
                    .and_then(|t| t.remote.clone())
                    .ok_or_else(|| anyhow!("this tab has no remote: use :tabremote"))?;
                if arg.is_empty() {
                    bail!("usage: :tabmove REMOTE (moves the shared data there, for everyone)");
                }
                let config = parse_remote(arg)?;
                if config == old {
                    bail!("already there");
                }
                self.ensure_identity()?;
                let (from, to) = (backend::open(&old)?, backend::open(&config)?);
                let me = self.identity.as_ref().expect("loaded");
                let n = cpasfait_core::sync::move_tab(&mut self.repo, tab, from.as_ref(), to.as_ref(), &config, me)?;
                self.refresh()?;
                self.info(format!(
                    "moved to {} ({n} files copied); the other members are asked to follow. The old remote can be \
                     deleted once they have",
                    config.spec()
                ));
            }
            "tabaccept" => {
                let tab = self.tab_id()?;
                let pending = self
                    .tab()
                    .and_then(|t| t.pending_move.clone())
                    .ok_or_else(|| anyhow!("no remote move to follow for this tab"))?;
                if arg.is_empty() {
                    // Let the user check or edit where it is on this machine.
                    self.mode = Mode::Command(LineEditor::new(&format!("tabaccept {}", pending.remote)));
                    return Ok(());
                }
                let config = parse_remote(arg)?;
                self.ensure_identity()?;
                let old = self
                    .tab()
                    .and_then(|t| t.remote.clone())
                    .and_then(|r| backend::open(&r).ok());
                let new = backend::open(&config)?;
                let me = self.identity.as_ref().expect("loaded");
                let r =
                    cpasfait_core::sync::accept_move(&mut self.repo, tab, old.as_deref(), new.as_ref(), &config, me)?;
                self.refresh()?;
                let mut msg = format!("now using {}", config.spec());
                if r.copied > 0 {
                    msg.push_str(&format!(" ({} late files copied)", r.copied));
                }
                if r.warnings.is_empty() {
                    self.info(msg);
                } else {
                    self.error(format!("{msg}; {}", r.warnings.join("; ")));
                }
            }
            "tabinvite" => {
                let usage = "usage: :tabinvite KEY [NAME] (KEY: age1… or ssh-ed25519 AAAA…, see :mykey)";
                let tab = self.tab_id()?;
                let (key, name) = split_key(arg).ok_or_else(|| anyhow!(usage))?;
                if self.repo.epochs(tab)?.is_empty() {
                    bail!("this tab is not shared: set a remote first with :tabremote");
                }
                let key = cpasfait_core::crypto::parse_recipient(&key)?;
                let members = self.repo.members(tab)?;
                if members.iter().any(|m| m.active && m.recipient == key) {
                    bail!("already a member");
                }
                let name = name.unwrap_or_else(|| short_key(&key));
                self.repo.set_member(tab, &key, Some(&name))?;
                self.refresh()?;
                self.requests.push(Request::Sync);
                let remote = self.tab().and_then(|t| t.remote.clone());
                self.info(match remote {
                    Some(r) => format!("invited {name}: they can now run :tabjoin {} {tab}", r.spec()),
                    None => format!("invited {name}"),
                });
            }
            "tabrevoke" => {
                let tab = self.tab_id()?;
                if arg.is_empty() {
                    bail!("usage: :tabrevoke NAME|KEY (see :tabmembers)");
                }
                let members: Vec<_> = self.repo.members(tab)?.into_iter().filter(|m| m.active).collect();
                let found: Vec<_> = members
                    .iter()
                    .filter(|m| m.name == arg || m.recipient == arg)
                    .chain(
                        members
                            .iter()
                            .filter(|m| m.recipient.starts_with(arg) && arg.len() >= 8),
                    )
                    .collect();
                let m = match found.as_slice() {
                    [] => bail!("no member {arg:?} (see :tabmembers)"),
                    [m, rest @ ..] if rest.iter().all(|r| r.recipient == m.recipient) => (*m).clone(),
                    _ => bail!("several members match {arg:?}: use their key"),
                };
                if members.len() == 1 {
                    bail!("{} is the last member", m.name);
                }
                self.repo.set_member(tab, &m.recipient, None)?;
                self.refresh()?;
                self.requests.push(Request::Sync);
                self.info(format!(
                    "removed {}: a new key is made at the next sync (also stop sharing the remote folder with them)",
                    m.name
                ));
            }
            "tabset" | "tabunset" => {
                let tab = self.tab_id()?;
                let (name, value) = match arg.split_once(['=', ' ']) {
                    Some((n, v)) => (n.trim(), Some(v.trim())),
                    None => (arg, None),
                };
                if cmd == "tabunset" {
                    if name.is_empty() || value.is_some() {
                        bail!("usage: :tabunset NAME");
                    }
                    self.repo.set_tab_setting(tab, name, None)?;
                } else if let Some(v) = value {
                    self.repo.set_tab_setting(tab, name, Some(v))?;
                }
                let settings = self.repo.tab_settings(tab)?;
                let shown: Vec<String> = cpasfait_core::TAB_SETTINGS
                    .iter()
                    .filter(|s| name.is_empty() || s.name == name)
                    .map(|s| format!("{}={} ({})", s.name, settings.get(s.name).unwrap_or_default(), s.help))
                    .collect();
                if shown.is_empty() {
                    cpasfait_core::TabSettings::describe(name)?;
                }
                self.info(shown.join(", "));
            }
            "tabmembers" => {
                let tab = self.tab_id()?;
                let mine = self.identity.as_ref().map(|i| i.recipient().to_owned());
                let members: Vec<String> = self
                    .repo
                    .members(tab)?
                    .into_iter()
                    .filter(|m| m.active)
                    .map(|m| {
                        let you = if mine.as_deref() == Some(&m.recipient) {
                            ", you"
                        } else {
                            ""
                        };
                        format!("{} ({}{you})", m.name, short_key(&m.recipient))
                    })
                    .collect();
                self.info(if members.is_empty() {
                    "not shared (see :tabremote)".to_owned()
                } else {
                    format!("members: {}", members.join(", "))
                });
            }
            "mykey" => {
                let note = self.ensure_identity()?;
                let me = self.me();
                let key = me.recipient().to_owned();
                let path = me.path().map(|p| p.display().to_string()).unwrap_or_default();
                self.info(match note {
                    Some(n) => format!("{n}; your key: {key}"),
                    None => format!("your key: {key}  ({path})"),
                });
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

/// Split `KEY [NAME]`; an SSH key is two words (`ssh-ed25519 AAAA…`), and its
/// comment, if any, is the default name.
fn split_key(arg: &str) -> Option<(String, Option<String>)> {
    let arg = arg.trim();
    let n = if arg.starts_with("ssh-") { 2 } else { 1 };
    let words: Vec<&str> = arg.splitn(n + 1, char::is_whitespace).collect();
    if words.len() < n || words[0].is_empty() {
        return None;
    }
    let key = words[..n].join(" ");
    let name = words.get(n).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    Some((key, name))
}

/// `age1qyz…k3x9`, `ssh-ed25519 …AbCd`.
pub fn short_key(key: &str) -> String {
    match key.split_once(' ') {
        Some((kind, b64)) => format!("{kind} …{}", &b64[b64.len().saturating_sub(8)..]),
        None if key.len() > 16 => format!("{}…{}", &key[..8], &key[key.len() - 4..]),
        None => key.to_owned(),
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
