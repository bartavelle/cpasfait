//! In-memory task hierarchy of one tab, built from the read model.
//!
//! Handles the anomalies that concurrent edits can produce:
//! - a parent that is unknown locally (not synced yet): the task shows at the root;
//! - a parent that is deleted: the task is hidden with it;
//! - parent cycles (two concurrent moves): the cycle member with the smallest
//!   id is re-rooted. Deterministic, so all replicas show the same tree.

use crate::ids::TaskId;
use crate::model::Task;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: TaskId,
    pub depth: usize,
    pub has_children: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// Visible tasks, in pre-order (display order).
    tasks: Vec<Task>,
    index: HashMap<TaskId, usize>,
    parent: Vec<Option<usize>>,
    children: Vec<Vec<TaskId>>,
    roots: Vec<TaskId>,
}

const NONE: usize = usize::MAX;

impl Tree {
    pub fn build(all: Vec<Task>) -> Tree {
        let n = all.len();
        let index: HashMap<TaskId, usize> = all.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        // Effective parents: unknown parents become root.
        let mut parent: Vec<usize> = all
            .iter()
            .map(|t| t.parent.and_then(|p| index.get(&p).copied()).unwrap_or(NONE))
            .collect();
        break_cycles(&all, &mut parent);

        // Visibility: not deleted and parent visible. 0 = unknown, 1 = visible, 2 = hidden.
        let mut vis = vec![0u8; n];
        let mut stack = vec![];
        for i in 0..n {
            let mut j = i;
            while vis[j] == 0 {
                stack.push(j);
                if all[j].deleted {
                    vis[j] = 2;
                    break;
                }
                match parent[j] {
                    NONE => {
                        vis[j] = 1;
                        break;
                    }
                    p => j = p,
                }
            }
            let v = vis[j];
            for k in stack.drain(..) {
                if vis[k] == 0 {
                    vis[k] = v;
                }
            }
        }

        // Children lists over the visible tasks, sorted by (position, id).
        let mut kids: Vec<Vec<usize>> = vec![vec![]; n];
        let mut roots: Vec<usize> = vec![];
        for i in (0..n).filter(|&i| vis[i] == 1) {
            match parent[i] {
                NONE => roots.push(i),
                p => kids[p].push(i),
            }
        }
        let cmp = |a: &usize, b: &usize| (&all[*a].position, all[*a].id).cmp(&(&all[*b].position, all[*b].id));
        roots.sort_by(cmp);
        for k in kids.iter_mut() {
            k.sort_by(cmp);
        }

        // Pre-order renumbering.
        let mut order: Vec<usize> = Vec::with_capacity(n);
        let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            stack.extend(kids[i].iter().rev());
        }
        let mut new_of = vec![NONE; n];
        for (new, &old) in order.iter().enumerate() {
            new_of[old] = new;
        }
        let mut slots: Vec<Option<Task>> = all.into_iter().map(Some).collect();
        let tasks: Vec<Task> = order.iter().map(|&old| slots[old].take().unwrap()).collect();
        let index = tasks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        let parent = order
            .iter()
            .map(|&old| Some(parent[old]).filter(|&p| p != NONE).map(|p| new_of[p]))
            .collect();
        let id_of = |old: usize| tasks[new_of[old]].id;
        let children = order
            .iter()
            .map(|&old| kids[old].iter().map(|&c| id_of(c)).collect())
            .collect();
        let roots = roots.iter().map(|&r| id_of(r)).collect();
        Tree {
            tasks,
            index,
            parent,
            children,
            roots,
        }
    }

    fn idx(&self, id: TaskId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// Pre-order walk; `f(id, depth)` returns whether to descend into children.
    fn walk(&self, root: Option<TaskId>, f: &mut impl FnMut(TaskId, usize) -> bool) {
        fn go(t: &Tree, p: Option<TaskId>, depth: usize, f: &mut impl FnMut(TaskId, usize) -> bool) {
            for &c in t.children(p) {
                if f(c, depth) {
                    go(t, Some(c), depth + 1, f);
                }
            }
        }
        go(self, root, 0, f)
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn get(&self, id: TaskId) -> Option<&Task> {
        self.idx(id).map(|i| &self.tasks[i])
    }

    pub fn contains(&self, id: TaskId) -> bool {
        self.index.contains_key(&id)
    }

    /// Visible tasks in display (pre-)order.
    pub fn iter(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter()
    }

    /// Visible children of `parent` (`None` = roots), in display order.
    pub fn children(&self, parent: Option<TaskId>) -> &[TaskId] {
        match parent {
            None => &self.roots,
            Some(p) => self.idx(p).map(|i| self.children[i].as_slice()).unwrap_or(&[]),
        }
    }

    pub fn parent(&self, id: TaskId) -> Option<TaskId> {
        self.idx(id).and_then(|i| self.parent[i]).map(|p| self.tasks[p].id)
    }

    /// Ancestors from the parent up to the root.
    pub fn ancestors(&self, id: TaskId) -> Vec<TaskId> {
        let mut out = vec![];
        let mut cur = self.idx(id).and_then(|i| self.parent[i]);
        while let Some(p) = cur {
            out.push(self.tasks[p].id);
            cur = self.parent[p];
        }
        out
    }

    /// Titles from the root down to `id`.
    pub fn path(&self, id: TaskId) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .ancestors(id)
            .iter()
            .rev()
            .map(|a| self.get(*a).unwrap().title.as_str())
            .collect();
        if let Some(t) = self.get(id) {
            v.push(&t.title);
        }
        v
    }

    /// All visible descendants of `id` (excluding itself), in display order.
    pub fn descendants(&self, id: TaskId) -> Vec<TaskId> {
        let mut out = vec![];
        self.walk(Some(id), &mut |c, _| {
            out.push(c);
            true
        });
        out
    }

    /// Display rows: skips children of `collapsed` tasks, and done tasks (with
    /// their subtree) when `hide_done`.
    pub fn rows(&self, collapsed: &HashSet<TaskId>, hide_done: bool) -> Vec<Row> {
        let mut out = vec![];
        self.walk(None, &mut |id, depth| {
            let t = self.get(id).unwrap();
            if hide_done && t.is_done() {
                return false;
            }
            let has_children = self
                .children(Some(id))
                .iter()
                .any(|c| !(hide_done && self.get(*c).unwrap().is_done()));
            out.push(Row {
                id,
                depth,
                has_children,
            });
            !collapsed.contains(&id)
        });
        out
    }

    /// The most prioritary actionable task:
    /// - not done, with no done ancestor (done hides the subtree's remaining work);
    /// - a leaf with respect to open work (no child still to do);
    /// - ordered by priority (highest first), then due date (earliest first,
    ///   none last), then tree order.
    pub fn next_task(&self) -> Option<TaskId> {
        let n = self.tasks.len();
        let mut open_child = vec![false; n];
        let mut active = vec![false; n]; // not done and no done ancestor
        for i in 0..n {
            let t = &self.tasks[i];
            active[i] = !t.is_done() && self.parent[i].is_none_or(|p| active[p]);
            if active[i]
                && let Some(p) = self.parent[i]
            {
                open_child[p] = true;
            }
        }
        let key = |i: usize| {
            let t = &self.tasks[i];
            (std::cmp::Reverse(t.priority), t.due.is_none(), t.due, i)
        };
        (0..n)
            .filter(|&i| active[i] && !open_child[i])
            .min_by_key(|&i| key(i))
            .map(|i| self.tasks[i].id)
    }
}

fn break_cycles(all: &[Task], parent: &mut [usize]) {
    // 0 = unvisited, 1 = on current path, 2 = known to reach a root.
    let n = parent.len();
    let mut state = vec![0u8; n];
    let mut path = vec![];
    for start in 0..n {
        let mut cur = start;
        while cur != NONE && state[cur] == 0 {
            state[cur] = 1;
            path.push(cur);
            cur = parent[cur];
        }
        if cur != NONE && state[cur] == 1 {
            // Cycle: the members are path[pos..]. Re-root the smallest id.
            let pos = path.iter().position(|&p| p == cur).unwrap();
            let victim = *path[pos..].iter().min_by_key(|&&i| all[i].id).unwrap();
            parent[victim] = NONE;
        }
        for p in path.drain(..) {
            state[p] = 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TabId;
    use crate::op::Status;

    fn task(id: TaskId, parent: Option<TaskId>, pos: &str) -> Task {
        Task {
            id,
            tab: TabId(uuid::Uuid::nil()),
            title: pos.into(),
            notes: String::new(),
            status: Status::Todo,
            priority: 0,
            due: None,
            parent,
            position: pos.into(),
            deleted: false,
            tags: vec![],
        }
    }

    #[test]
    fn cycle_is_broken_deterministically() {
        let mut ids = [TaskId::new(), TaskId::new(), TaskId::new()];
        ids.sort();
        let [a, b, c] = ids;
        // a -> b -> c -> a
        let t = Tree::build(vec![
            task(a, Some(b), "1"),
            task(b, Some(c), "2"),
            task(c, Some(a), "3"),
        ]);
        assert_eq!(t.children(None), &[a]);
        assert_eq!(t.parent(c), Some(a));
        assert_eq!(t.parent(b), Some(c));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn deleted_parent_hides_subtree_unknown_parent_is_root() {
        let (a, b, c, ghost) = (TaskId::new(), TaskId::new(), TaskId::new(), TaskId::new());
        let mut ta = task(a, None, "1");
        ta.deleted = true;
        let t = Tree::build(vec![ta, task(b, Some(a), "2"), task(c, Some(ghost), "3")]);
        assert!(!t.contains(a) && !t.contains(b));
        assert_eq!(t.children(None), &[c]);
    }

    #[test]
    fn next_task_rules() {
        let (p, c1, c2, other) = (TaskId::new(), TaskId::new(), TaskId::new(), TaskId::new());
        let mut tp = task(p, None, "1");
        tp.priority = 5; // parent with open children: not a candidate
        let tc1 = task(c1, Some(p), "1");
        let mut tc2 = task(c2, Some(p), "2");
        tc2.due = Some(jiff::civil::date(2026, 1, 1));
        let mut to = task(other, None, "2");
        to.due = Some(jiff::civil::date(2027, 1, 1));
        let t = Tree::build(vec![tp.clone(), tc1.clone(), tc2.clone(), to.clone()]);
        // same priority: earliest due wins over no due
        assert_eq!(t.next_task(), Some(c2));
        let mut tc1b = tc1.clone();
        tc1b.priority = 1;
        let t = Tree::build(vec![tp.clone(), tc1b, tc2.clone(), to.clone()]);
        assert_eq!(t.next_task(), Some(c1));
        // children done -> parent becomes actionable
        let mut d1 = tc1.clone();
        d1.status = Status::Done;
        let mut d2 = tc2.clone();
        d2.status = Status::Done;
        let t = Tree::build(vec![tp.clone(), d1, d2, to.clone()]);
        assert_eq!(t.next_task(), Some(p));
        // done parent hides its children
        let mut dp = tp.clone();
        dp.status = Status::Done;
        let t = Tree::build(vec![dp, tc1, tc2, to]);
        assert_eq!(t.next_task(), Some(other));
    }
}
