//! The Ctrl-P task finder: fuzzy search on task titles across all tabs.

use crate::editor::LineEditor;
use crate::fuzzy::fuzzy_match;
use cpasfait_core::{Repo, TabId, TaskId};

pub const MAX_RESULTS: usize = 50;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: TaskId,
    pub tab: TabId,
    pub tab_name: String,
    pub title: String,
    /// Ancestors' titles, `A › B`.
    pub context: String,
    pub done: bool,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub candidate: usize,
    /// Matched char indices in the title.
    pub positions: Vec<usize>,
}

/// What choosing a task does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Jump to it (all tabs).
    Go,
    /// The task requires the chosen one.
    Require(TaskId),
    /// The task blocks the chosen one.
    Block(TaskId),
    /// Remove one of the task's requirements.
    Unrequire(TaskId),
    /// Remove one of the tasks the task blocks.
    Unblock(TaskId),
}

#[derive(Clone, Debug)]
pub struct Search {
    pub purpose: Purpose,
    /// Dialog title.
    pub title: String,
    pub editor: LineEditor,
    pub candidates: Vec<Candidate>,
    pub hits: Vec<Hit>,
    pub selected: usize,
    /// The tab the search was opened from (listed first on ties).
    pub current_tab: Option<TabId>,
}

impl Search {
    /// For [`Purpose::Go`]: all visible tasks of all tabs, current tab first,
    /// in tree order. Dependencies stay within the current tab.
    pub fn new(repo: &Repo, current_tab: Option<TabId>, purpose: Purpose, query: &str) -> anyhow::Result<Self> {
        let mut tabs = repo.tabs()?;
        tabs.sort_by_key(|t| Some(t.id) != current_tab);
        if purpose != Purpose::Go {
            tabs.retain(|t| Some(t.id) == current_tab);
        }
        let mut candidates = vec![];
        let mut title = " find task ".to_owned();
        for tab in tabs {
            let tree = repo.tree(tab.id)?;
            let name = |id| {
                let t: &str = tree.get(id).map(|t| t.title.as_str()).unwrap_or("?");
                let mut s: String = t.chars().take(30).collect();
                if t.chars().count() > 30 {
                    s.push('…');
                }
                s
            };
            let keep: Option<Vec<TaskId>> = match purpose {
                Purpose::Go => None,
                Purpose::Require(id) => {
                    title = format!(" “{}” requires… ", name(id));
                    Some(tree.iter().map(|t| t.id).filter(|&t| t != id).collect())
                }
                Purpose::Block(id) => {
                    title = format!(" “{}” blocks… ", name(id));
                    Some(tree.iter().map(|t| t.id).filter(|&t| t != id).collect())
                }
                Purpose::Unrequire(id) => {
                    title = format!(" “{}” no longer requires… ", name(id));
                    Some(tree.blockers(id))
                }
                Purpose::Unblock(id) => {
                    title = format!(" “{}” no longer blocks… ", name(id));
                    Some(tree.blocking(id))
                }
            };
            for t in tree.iter() {
                if keep.as_ref().is_some_and(|k| !k.contains(&t.id)) {
                    continue;
                }
                let mut path = tree.path(t.id);
                path.pop();
                candidates.push(Candidate {
                    id: t.id,
                    tab: tab.id,
                    tab_name: tab.name.clone(),
                    title: t.title.clone(),
                    context: path.join(" › "),
                    done: t.is_done(),
                });
            }
        }
        let mut s = Self {
            purpose,
            title,
            editor: LineEditor::new(query),
            candidates,
            hits: vec![],
            selected: 0,
            current_tab,
        };
        s.update();
        Ok(s)
    }

    pub fn query(&self) -> &str {
        self.editor.text()
    }

    /// Recompute hits for the current query: best score first; done tasks and
    /// other tabs lose ties; then shorter titles, then tree order.
    pub fn update(&mut self) {
        let q = self.editor.text().trim().to_owned();
        let mut scored: Vec<(i64, usize, usize, Vec<usize>)> = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let (mut score, positions) = fuzzy_match(&q, &c.title)?;
                if c.done {
                    score -= 20;
                }
                if Some(c.tab) != self.current_tab {
                    score -= 5;
                }
                Some((score, c.title.chars().count(), i, positions))
            })
            .collect();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        }
        self.hits = scored
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(_, _, candidate, positions)| Hit { candidate, positions })
            .collect();
        self.selected = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        let n = self.hits.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
    }

    pub fn chosen(&self) -> Option<&Candidate> {
        self.hits.get(self.selected).map(|h| &self.candidates[h.candidate])
    }
}

#[cfg(test)]
mod tests {
    /// `CPASFAIT_BENCH_DB=path cargo test --release -p cpasfait-tui search_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn search_speed() {
        let path = std::env::var("CPASFAIT_BENCH_DB").expect("CPASFAIT_BENCH_DB");
        let repo = cpasfait_core::Repo::open_readonly(std::path::Path::new(&path)).unwrap();
        let t = std::time::Instant::now();
        let mut s = super::Search::new(&repo, None, super::Purpose::Go, "").unwrap();
        println!("{} candidates loaded in {:?}", s.candidates.len(), t.elapsed());
        for q in ["a", "ta", "tas", "task 12", "zzzz"] {
            s.editor = crate::editor::LineEditor::new(q);
            let t = std::time::Instant::now();
            s.update();
            println!("{q:>8}: {} hits in {:?}", s.hits.len(), t.elapsed());
        }
    }
}
