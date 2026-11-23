//! Time reports: where time was spent between two instants.

use crate::error::Result;
use crate::ids::{TabId, TaskId};
use crate::repo::Repo;
use crate::tree::Tree;
use jiff::tz::TimeZone;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// A piece of a work session clipped to the report range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Slice {
    pub tab: TabId,
    pub task: Option<TaskId>,
    pub user: String,
    pub start: i64,
    pub end: i64,
}

impl Slice {
    pub fn ms(&self) -> i64 {
        self.end - self.start
    }
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub tab: Option<TabId>,
    pub user: Option<String>,
}

/// Session slices in `[from, to)` (Unix ms). Open sessions run until `now`.
pub fn slices(repo: &Repo, from: i64, to: i64, filter: &Filter) -> Result<Vec<Slice>> {
    let now = repo.now_ms();
    let mut out = vec![];
    for s in repo.sessions_overlapping(from, to, filter.tab)? {
        if filter.user.as_ref().is_some_and(|u| *u != s.user) {
            continue;
        }
        let start = s.start.unwrap_or(from).max(from);
        let end = s.end.unwrap_or(now).min(to);
        if end > start {
            out.push(Slice {
                tab: s.tab,
                task: s.task,
                user: s.user,
                start,
                end,
            });
        }
    }
    Ok(out)
}

/// One line of a per-task report, in tree order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaskLine {
    pub task: Option<TaskId>,
    pub title: String,
    pub depth: usize,
    /// Time logged on this task itself.
    pub own_ms: i64,
    /// Own time plus all descendants'.
    pub total_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TabReport {
    pub tab: TabId,
    pub name: String,
    pub total_ms: i64,
    pub lines: Vec<TaskLine>,
}

/// Per-tab, per-task breakdown with totals rolled up through the hierarchy.
/// Only tasks with time (or descendants with time) appear. Time on tasks that
/// are no longer visible (deleted) is grouped under a "(deleted tasks)" line.
pub fn by_task(repo: &Repo, slices: &[Slice]) -> Result<Vec<TabReport>> {
    let mut per_tab: BTreeMap<TabId, HashMap<Option<TaskId>, i64>> = BTreeMap::new();
    for s in slices {
        *per_tab.entry(s.tab).or_default().entry(s.task).or_default() += s.ms();
    }
    let names: HashMap<TabId, String> = repo.tabs()?.into_iter().map(|t| (t.id, t.name)).collect();
    let mut out = vec![];
    for (tab, own) in per_tab {
        let tree = repo.tree(tab)?;
        let mut lines = vec![];
        let total_of = |id: TaskId| -> i64 {
            own.get(&Some(id)).copied().unwrap_or(0)
                + tree
                    .descendants(id)
                    .iter()
                    .map(|d| own.get(&Some(*d)).copied().unwrap_or(0))
                    .sum::<i64>()
        };
        fn go(
            tree: &Tree,
            parent: Option<TaskId>,
            depth: usize,
            own: &HashMap<Option<TaskId>, i64>,
            total_of: &dyn Fn(TaskId) -> i64,
            lines: &mut Vec<TaskLine>,
        ) {
            for &c in tree.children(parent) {
                let total = total_of(c);
                if total == 0 {
                    continue;
                }
                lines.push(TaskLine {
                    task: Some(c),
                    title: tree.get(c).map(|t| t.title.clone()).unwrap_or_default(),
                    depth,
                    own_ms: own.get(&Some(c)).copied().unwrap_or(0),
                    total_ms: total,
                });
                go(tree, Some(c), depth + 1, own, total_of, lines);
            }
        }
        go(&tree, None, 0, &own, &total_of, &mut lines);
        let orphan: i64 = own
            .iter()
            .filter(|(k, _)| k.is_none_or(|id| !tree.contains(id)))
            .map(|(_, v)| v)
            .sum();
        if orphan > 0 {
            lines.push(TaskLine {
                task: None,
                title: "(deleted tasks)".into(),
                depth: 0,
                own_ms: orphan,
                total_ms: orphan,
            });
        }
        let total_ms = own.values().sum();
        out.push(TabReport {
            tab,
            name: names.get(&tab).cloned().unwrap_or_default(),
            total_ms,
            lines,
        });
    }
    Ok(out)
}

/// The tags of each task with time: its own and its ancestors' (tagging a
/// project's parent task tags all its subtasks), sorted.
fn effective_tags(repo: &Repo, slices: &[Slice]) -> Result<HashMap<Option<TaskId>, Vec<String>>> {
    let mut trees: HashMap<TabId, Tree> = HashMap::new();
    let mut out: HashMap<Option<TaskId>, Vec<String>> = HashMap::new();
    for s in slices {
        let Some(task) = s.task else {
            out.entry(None).or_default();
            continue;
        };
        if out.contains_key(&Some(task)) {
            continue;
        }
        let tree = match trees.entry(s.tab) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(repo.tree(s.tab)?),
        };
        let mut tags: Vec<String> = if tree.contains(task) {
            std::iter::once(task)
                .chain(tree.ancestors(task))
                .filter_map(|id| tree.get(id))
                .flat_map(|t| t.tags.iter().cloned())
                .collect()
        } else {
            // deleted (or unknown): its own tags only
            repo.task(task).map(|t| t.tags).unwrap_or_default()
        };
        tags.sort();
        tags.dedup();
        out.insert(Some(task), tags);
    }
    Ok(out)
}

/// Totals per combination of tags (see `effective_tags`; empty: no tags),
/// sorted by decreasing time.
pub fn by_tag_set(repo: &Repo, slices: &[Slice]) -> Result<Vec<(Vec<String>, i64)>> {
    let tags = effective_tags(repo, slices)?;
    let mut m: HashMap<&[String], i64> = HashMap::new();
    for s in slices {
        *m.entry(tags[&s.task].as_slice()).or_default() += s.ms();
    }
    Ok(sorted(m.into_iter().map(|(k, v)| (k.to_vec(), v))))
}

/// Totals per tag (`None`: tasks without tags). A task with several tags
/// counts for each, so the totals add up to more than the time spent.
pub fn by_tag(repo: &Repo, slices: &[Slice]) -> Result<Vec<(Option<String>, i64)>> {
    let tags = effective_tags(repo, slices)?;
    let mut m: HashMap<Option<&str>, i64> = HashMap::new();
    for s in slices {
        let t = &tags[&s.task];
        if t.is_empty() {
            *m.entry(None).or_default() += s.ms();
        }
        for tag in t {
            *m.entry(Some(tag.as_str())).or_default() += s.ms();
        }
    }
    Ok(sorted(m.into_iter().map(|(k, v)| (k.map(str::to_owned), v))))
}

/// By decreasing time, then key; the "none" key (empty, `None`) last.
fn sorted<K: Ord + Default>(it: impl Iterator<Item = (K, i64)>) -> Vec<(K, i64)> {
    let mut v: Vec<_> = it.collect();
    v.sort_by(|a, b| {
        (a.0 == K::default())
            .cmp(&(b.0 == K::default()))
            .then(b.1.cmp(&a.1))
            .then(a.0.cmp(&b.0))
    });
    v
}

/// Totals per user, sorted by decreasing time.
pub fn by_user(slices: &[Slice]) -> Vec<(String, i64)> {
    let mut m: HashMap<&str, i64> = HashMap::new();
    for s in slices {
        *m.entry(&s.user).or_default() += s.ms();
    }
    let mut v: Vec<_> = m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// Totals per calendar day in `tz`, splitting slices across midnight.
pub fn by_day(slices: &[Slice], tz: &TimeZone) -> Vec<(jiff::civil::Date, i64)> {
    let mut m: BTreeMap<jiff::civil::Date, i64> = BTreeMap::new();
    for s in slices {
        let mut start = s.start;
        while start < s.end {
            let Ok(ts) = jiff::Timestamp::from_millisecond(start) else {
                break;
            };
            let zdt = ts.to_zoned_datetime(tz.clone());
            let day = zdt.date();
            let next_midnight = day
                .tomorrow()
                .ok()
                .and_then(|d| d.to_zoned_datetime(tz.clone()).ok())
                .map(|z| z.timestamp().as_millisecond())
                .unwrap_or(s.end);
            let end = s.end.min(next_midnight.max(start + 1));
            *m.entry(day).or_default() += end - start;
            start = end;
        }
    }
    m.into_iter().collect()
}

/// Format milliseconds as `HhMM` (e.g. `3h05`), or `MMm` under an hour.
pub fn fmt_duration(ms: i64) -> String {
    let mins = ms / 60_000;
    if mins < 60 {
        format!("{mins}m")
    } else {
        format!("{}h{:02}", mins / 60, mins % 60)
    }
}
