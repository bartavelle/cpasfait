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
    AddBelow,
    AddAbove,
    AddChild,
    EditTitle,
    ReplaceTitle,
    EditNotes,
    ToggleDone,
    Delete,
    UndoDelete,
    Indent,
    Outdent,
    MoveUp,
    MoveDown,
    PriorityUp,
    PriorityDown,
    ToggleWork,
    StopWork,
    JumpNext,
    JumpActive,
    ToggleHideDone,
    ToggleDetail,
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
    ("<c-pagedown>", Action::NextTab),
    ("<c-pageup>", Action::PrevTab),
    ("o", Action::AddBelow),
    ("O", Action::AddAbove),
    ("a", Action::AddChild),
    ("i", Action::EditTitle),
    ("<enter>", Action::EditTitle),
    ("cc", Action::ReplaceTitle),
    ("e", Action::EditNotes),
    ("x", Action::ToggleDone),
    (" ", Action::ToggleDone),
    ("dd", Action::Delete),
    ("u", Action::UndoDelete),
    (">>", Action::Indent),
    ("<<", Action::Outdent),
    ("K", Action::MoveUp),
    ("J", Action::MoveDown),
    ("+", Action::PriorityUp),
    ("-", Action::PriorityDown),
    ("s", Action::ToggleWork),
    ("S", Action::StopWork),
    ("gn", Action::JumpNext),
    ("gs", Action::JumpActive),
    ("H", Action::ToggleHideDone),
    ("D", Action::ToggleDetail),
    (":", Action::Command),
    ("?", Action::Help),
    ("q", Action::Quit),
];

/// Encode a key event into the notation used in [`BINDINGS`].
pub fn key_name(key: KeyEvent) -> Option<String> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char(c) if ctrl => format!("<c-{}>", c.to_ascii_lowercase()),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Down => "<down>".into(),
        KeyCode::Up => "<up>".into(),
        KeyCode::Left => "<left>".into(),
        KeyCode::Right => "<right>".into(),
        KeyCode::Tab => "<tab>".into(),
        KeyCode::Enter => "<enter>".into(),
        KeyCode::PageDown if ctrl => "<c-pagedown>".into(),
        KeyCode::PageUp if ctrl => "<c-pageup>".into(),
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
              gt/gT    next/prev tab (also ^PgDn/^PgUp) 1-9    go to tab
              gn       jump to next (most prioritary) task
              gs       jump to the task being worked on
Editing       o/O      new task below/above             a      new subtask
              i Enter  edit title    cc  replace title   e      notes in $EDITOR
              x Space  toggle done   dd  delete          u      undo delete
              >>/<<    indent/outdent                    J/K    move down/up
              +/-      priority up/down
Work log      s        start/stop working on the task   S      stop any running work
View          H        hide/show done tasks             D      toggle detail pane
Commands      :due DATE|none  (2026-12-01, today, tom, +3d, fri)   :tag +a -b
              :prio N   :title TEXT   :worknote TEXT   :user NAME
              :tabnew NAME   :tabrename NAME   :tabclose!   :sync
              :tabremote REMOTE|none   :tabjoin REMOTE [TAB-ID]   :rebuild   :q
              REMOTE = a directory, or gdrive:FOLDER-ID (see cpasfait-gdrive)
Press any key to close this help.";
