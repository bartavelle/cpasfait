//! Normal-mode key bindings, including multi-key sequences (`dd`, `gt`, ...).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Top,
    Bottom,
    PageDown,
    PageUp,
    CollapseOrParent,
    ExpandOrChild,
    ToggleFold,
    NextTab,
    PrevTab,
    GotoTab(usize),
    AddSibling,
    AddChild,
    EditTitle,
    ReplaceTitle,
    EditNotes,
    EditTask,
    EditTags,
    Search,
    ToggleDone,
    Delete,
    UndoDelete,
    Indent,
    Outdent,
    PriorityUp,
    PriorityDown,
    ToggleWork,
    StopWork,
    SwitchWork,
    JumpNext,
    JumpActive,
    ToggleHideDone,
    ToggleDetail,
    CycleTimeBox,
    Command,
    Help,
    Quit,
}

pub enum Lookup {
    Action(Action),
    /// A prefix of a longer binding: wait for more keys.
    Pending,
    None,
}

const BINDINGS: &[(&str, Action)] = &[
    ("j", Action::Down),
    ("<down>", Action::Down),
    ("k", Action::Up),
    ("<up>", Action::Up),
    ("gg", Action::Top),
    ("G", Action::Bottom),
    ("<c-d>", Action::PageDown),
    ("<c-u>", Action::PageUp),
    ("h", Action::CollapseOrParent),
    ("<left>", Action::CollapseOrParent),
    ("l", Action::ExpandOrChild),
    ("<right>", Action::ExpandOrChild),
    ("<tab>", Action::ToggleFold),
    ("za", Action::ToggleFold),
    ("gt", Action::NextTab),
    ("gT", Action::PrevTab),
    ("<a-right>", Action::NextTab),
    ("<a-left>", Action::PrevTab),
    ("n", Action::AddSibling),
    ("a", Action::AddChild),
    ("i", Action::EditTitle),
    ("<enter>", Action::EditTitle),
    ("cc", Action::ReplaceTitle),
    ("e", Action::EditNotes),
    ("E", Action::EditTask),
    ("t", Action::EditTags),
    ("x", Action::ToggleDone),
    (" ", Action::ToggleDone),
    ("dd", Action::Delete),
    ("u", Action::UndoDelete),
    (">>", Action::Indent),
    ("<<", Action::Outdent),
    ("+", Action::PriorityUp),
    ("-", Action::PriorityDown),
    ("s", Action::ToggleWork),
    ("S", Action::StopWork),
    ("W", Action::SwitchWork),
    ("gn", Action::JumpNext),
    ("gs", Action::JumpActive),
    ("H", Action::ToggleHideDone),
    ("D", Action::ToggleDetail),
    ("T", Action::CycleTimeBox),
    ("<c-p>", Action::Search),
    ("/", Action::Search),
    (":", Action::Command),
    ("?", Action::Help),
    ("q", Action::Quit),
];

/// Encode a key event into the notation used in [`BINDINGS`].
pub fn key_name(key: KeyEvent) -> Option<String> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    Some(match key.code {
        KeyCode::Char(c) if ctrl => format!("<c-{}>", c.to_ascii_lowercase()),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Down => "<down>".into(),
        KeyCode::Up => "<up>".into(),
        KeyCode::Left if alt => "<a-left>".into(),
        KeyCode::Right if alt => "<a-right>".into(),
        KeyCode::Left => "<left>".into(),
        KeyCode::Right => "<right>".into(),
        KeyCode::Tab => "<tab>".into(),
        KeyCode::Enter => "<enter>".into(),
        KeyCode::PageDown => "<c-d>".into(),
        KeyCode::PageUp => "<c-u>".into(),
        _ => return None,
    })
}

pub fn lookup(seq: &str) -> Lookup {
    if let Some((_, a)) = BINDINGS.iter().find(|(k, _)| *k == seq) {
        return Lookup::Action(*a);
    }
    if seq.len() == 1
        && let Some(d) = seq.chars().next().and_then(|c| c.to_digit(10)).filter(|&d| d >= 1)
    {
        return Lookup::Action(Action::GotoTab(d as usize - 1));
    }
    if BINDINGS.iter().any(|(k, _)| k.starts_with(seq)) {
        return Lookup::Pending;
    }
    Lookup::None
}

pub const HELP: &str = "\
Navigation    j/k ↓/↑  move        gg/G  top/bottom     ^d/^u  page
              h/l ←/→  fold/parent, unfold/child        Tab/za toggle fold
              gt/gT    next/prev tab (also Alt-→/Alt-←) 1-9    go to tab
              gn       jump to next (most prioritary) task
              gs       jump to the task being worked on
              ^p /     find a task (fuzzy, all tabs): ↑/↓ choose, Enter go
Editing       n        new task (beside this one)       a      new subtask
              i Enter  edit title    cc  replace title   e      notes in $EDITOR
              x Space  toggle done   dd  delete          u      undo delete
              >>/<<    indent/outdent       (tasks are sorted by score: + - :due)
              +/-      priority up/down (-3 to 3)       t      edit tags
              E :edit  title and work log in $EDITOR, last 30 days (:edit all, :edit DATE)
Work log      s        start/stop working on the task   S      stop running work at…
              W        switch to this task at…          (times: 14:30, -20m, yesterday 18:00)
View          H        hide/show done tasks             D      toggle detail pane
              T        time spent (all tabs): hidden → today → this week → month
Commands      ↑/↓ history (starting with what is typed)   Tab/S-Tab complete
              :due DATE [TIME]|TIME|none  (2026-12-01, tom, +3d, fri 9:00, 17:30)  :tag +a -b
              :prio N   :title TEXT   :worknote TEXT   :user NAME   :sync   :q
              :require  :block  :unrequire  :unblock  (pick the other task)
              :moveto TAB  move the task, its subtasks and work log to another tab
              :tabnew NAME   :tabrename NAME   :tabclose!
              :tabset [NAME [VALUE]]   :tabunset NAME   tab settings (default_priority)
              :tabremote REMOTE|none   :tabjoin REMOTE [TAB-ID]   :rebuild
              REMOTE = a directory, gdrive:FOLDER-ID, or rsync:[user@]host:path
Sharing       :mykey  your public key   :tabinvite KEY [NAME]   :tabrevoke NAME
              :tabmembers   (remote data is encrypted to the members' age/SSH keys)
              :tabmove REMOTE  move it for everyone   :tabaccept  follow a move (⇅↪)
j/k ^d/^u Space scroll, q Esc ? close this help.";
