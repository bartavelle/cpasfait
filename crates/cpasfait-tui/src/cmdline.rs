//! Command line history (Up/Down) and Tab completion.

use std::path::PathBuf;

/// Commands offered by completion (aliases left out).
pub const COMMANDS: &[&str] = &[
    "block",
    "due",
    "edit",
    "help",
    "moveto",
    "mykey",
    "note",
    "prio",
    "q",
    "rebuild",
    "require",
    "stop",
    "switch",
    "sync",
    "tabaccept",
    "tabclose!",
    "tabinvite",
    "tabjoin",
    "tabmembers",
    "tabmove",
    "tabnew",
    "tabremote",
    "tabrename",
    "tabrevoke",
    "tabset",
    "tabunset",
    "tag",
    "title",
    "unblock",
    "unrequire",
    "user",
    "worknote",
];

const MAX_HISTORY: usize = 1000;

/// Commands run before, oldest first, kept in a file (one per line) when
/// there is one.
#[derive(Default)]
pub struct History {
    entries: Vec<String>,
    path: Option<PathBuf>,
    nav: Option<Nav>,
}

/// Browsing the history: the entry shown, and what was typed before.
struct Nav {
    idx: usize,
    draft: String,
}

impl History {
    pub fn load(path: PathBuf) -> History {
        let entries = std::fs::read_to_string(&path)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect())
            .unwrap_or_default();
        History {
            entries,
            path: Some(path),
            nav: None,
        }
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Remember `line` as the newest entry (an older copy is dropped).
    pub fn add(&mut self, line: &str) {
        self.nav = None;
        let line = line.trim();
        if line.is_empty() || line.contains('\n') {
            return;
        }
        self.entries.retain(|e| e != line);
        self.entries.push(line.to_owned());
        let excess = self.entries.len().saturating_sub(MAX_HISTORY);
        self.entries.drain(..excess);
        if let Some(path) = &self.path {
            // Not worth bothering the user if it can't be written.
            let tmp = path.with_extension("tmp");
            let mut text = self.entries.join("\n");
            text.push('\n');
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    /// Stop browsing (the line was edited, or left).
    pub fn reset(&mut self) {
        self.nav = None;
    }

    /// The previous entry starting with what was typed (`current`, when
    /// browsing starts).
    pub fn older(&mut self, current: &str) -> Option<String> {
        let len = self.entries.len();
        let nav = self.nav.get_or_insert_with(|| Nav {
            idx: len,
            draft: current.to_owned(),
        });
        let i = (0..nav.idx).rev().find(|&i| self.entries[i].starts_with(&nav.draft))?;
        nav.idx = i;
        Some(self.entries[i].clone())
    }

    /// The next entry, or back to what was typed after the newest one.
    pub fn newer(&mut self) -> Option<String> {
        let nav = self.nav.as_mut()?;
        match (nav.idx + 1..self.entries.len()).find(|&i| self.entries[i].starts_with(&nav.draft)) {
            Some(i) => {
                nav.idx = i;
                Some(self.entries[i].clone())
            }
            None => self.nav.take().map(|n| n.draft),
        }
    }
}

/// What can replace `line[start..cursor]`.
pub struct Candidates {
    pub start: usize,
    pub items: Vec<String>,
    /// Add a space after a unique match (more words may follow).
    pub space: bool,
}

/// Completion in progress: Tab again cycles through `items`.
pub struct Completion {
    pub start: usize,
    pub items: Vec<String>,
    /// The item inserted, none yet when only the common prefix was.
    pub selected: Option<usize>,
    /// The line as completion left it: anything else means it was edited.
    pub line: String,
}

/// `items` starting with `word` (ignoring case), sorted, without duplicates.
pub fn matching(word: &str, items: impl IntoIterator<Item = String>) -> Vec<String> {
    let lower = word.to_lowercase();
    let mut v: Vec<String> = items
        .into_iter()
        .filter(|i| i.to_lowercase().starts_with(&lower))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The longest prefix common to all `items`.
pub fn common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut end = first.len();
    for i in &items[1..] {
        end = first
            .char_indices()
            .zip(i.chars())
            .find(|((_, a), b)| a != b)
            .map_or(end.min(i.len()), |((idx, _), _)| idx.min(end));
    }
    first[..end].to_owned()
}

/// Where the word being completed starts: after the last space.
pub fn word_start(before: &str) -> usize {
    before.rfind(char::is_whitespace).map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_browses_by_prefix_and_restores_the_draft() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");
        let mut h = History::load(path.clone());
        for c in ["tabnew a", "sync", "tabnew b", "sync"] {
            h.add(c);
        }
        assert_eq!(h.entries(), ["tabnew a", "tabnew b", "sync"]);
        assert_eq!(h.older("tab").as_deref(), Some("tabnew b"));
        assert_eq!(h.older("ignored").as_deref(), Some("tabnew a"));
        assert_eq!(h.older("x"), None);
        assert_eq!(h.newer().as_deref(), Some("tabnew b"));
        assert_eq!(h.newer().as_deref(), Some("tab"));
        assert_eq!(h.newer(), None);
        // Saved.
        assert_eq!(History::load(path).entries(), ["tabnew a", "tabnew b", "sync"]);
    }

    #[test]
    fn prefixes() {
        let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(common_prefix(&v(&["tabmove", "tabmembers"])), "tabm");
        assert_eq!(common_prefix(&v(&["tabé1", "tabé2"])), "tabé");
        assert_eq!(common_prefix(&v(&["abc", "ab"])), "ab");
        assert_eq!(matching("TA", v(&["tag", "tab", "x", "tag"])), ["tab", "tag"]);
        assert_eq!(word_start("tag +foo ba"), 9);
    }
}
