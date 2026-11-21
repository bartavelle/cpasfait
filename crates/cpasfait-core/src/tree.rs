//! In-memory task hierarchy of one tab, built from the read model.
//!
//! Handles the anomalies that concurrent edits can produce:
//! - a parent that is unknown locally (not synced yet): the task shows at the root;
//! - a parent that is deleted: the task is hidden with it;
//! - parent cycles (two concurrent moves): the cycle member with the smallest
//!   id is re-rooted. Deterministic, so all replicas show the same tree.

use crate::ids::TaskId;
use crate::model::Task;
use crate::score::{Score, own_score};
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
    /// Done-state of required tasks outside this tree (other tabs). Absent =
    /// unknown or deleted: does not block.
    external: HashMap<TaskId, bool>,
    /// Per task: summed scores of the open tasks outside this tree that
    /// require it.
    external_blocking: HashMap<TaskId, i64>,
    /// "Today" for due-date scores (default: the local date).
    today: Option<jiff::civil::Date>,
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
            external: HashMap::new(),
            external_blocking: HashMap::new(),
            today: None,
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

    pub fn set_external(&mut self, done: HashMap<TaskId, bool>) {
        self.external = done;
    }

    pub fn set_external_blocking(&mut self, scores: HashMap<TaskId, i64>) {
        self.external_blocking = scores;
    }

    pub fn set_today(&mut self, today: jiff::civil::Date) {
        self.today = Some(today);
    }

    fn today(&self) -> jiff::civil::Date {
        self.today.unwrap_or_else(crate::dates::today_local)
    }

    /// Not done and no done ancestor, per index.
    fn active(&self) -> Vec<bool> {
        let mut active = vec![false; self.tasks.len()];
        for i in 0..self.tasks.len() {
            active[i] = !self.tasks[i].is_done() && self.parent[i].is_none_or(|p| active[p]);
        }
        active
    }

    /// Scores of the open tasks (see [`crate::score`]), per index (`None` for
    /// done ones). Requirement cycles are cut where the walk re-enters them.
    fn score_vec(&self, active: &[bool]) -> Vec<Option<Score>> {
        let n = self.tasks.len();
        let today = self.today();
        // dependents[j]: open tasks of this tree requiring task j
        let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];
        for (i, t) in self.tasks.iter().enumerate() {
            if !active[i] {
                continue;
            }
            for b in &t.blocked_by {
                if let Some(&j) = self.index.get(b) {
                    dependents[j].push(i);
                }
            }
        }
        let mut out: Vec<Option<Score>> = vec![None; n];
        let mut state = vec![0u8; n]; // 0 new, 1 in progress, 2 done
        for root in 0..n {
            if !active[root] || state[root] != 0 {
                continue;
            }
            // Iterative post-order DFS over "is required by" edges.
            let mut stack = vec![(root, 0usize)];
            state[root] = 1;
            while let Some(&mut (i, ref mut k)) = stack.last_mut() {
                if let Some(&d) = dependents[i].get(*k) {
                    *k += 1;
                    if state[d] == 0 {
                        state[d] = 1;
                        stack.push((d, 0));
                    }
                    continue;
                }
                stack.pop();
                let t = &self.tasks[i];
                let mut s = own_score(t.priority, t.due, today);
                s.blocking = self.external_blocking.get(&t.id).copied().unwrap_or(0)
                    + dependents[i]
                        .iter()
                        .filter_map(|&d| out[d].map(|x| x.total()))
                        .sum::<i64>();
                out[i] = Some(s);
                state[i] = 2;
            }
        }
        out
    }

    /// Scores of the open tasks.
    pub fn scores(&self) -> HashMap<TaskId, Score> {
        let active = self.active();
        self.score_vec(&active)
            .into_iter()
            .enumerate()
            .filter_map(|(i, s)| Some((self.tasks[i].id, s?)))
            .collect()
    }

    /// Order siblings by decreasing score (then earliest due date, none
    /// last), done tasks after the open ones; ties keep the current order
    /// (positions, i.e. creation order). Call once the external information
    /// ([`Tree::set_external`], [`Tree::set_external_blocking`], today) is set.
    pub fn sort_by_score(&mut self) {
        let n = self.tasks.len();
        let active = self.active();
        let scores = self.score_vec(&active);
        let keys: Vec<_> = (0..n)
            .map(|i| {
                let t = &self.tasks[i];
                let s = scores[i].map(|s| s.total());
                (
                    s.is_none(),
                    std::cmp::Reverse(s.unwrap_or(0)),
                    t.due.is_none(),
                    t.due,
                    i,
                )
            })
            .collect();
        let index = &self.index;
        let by_key = |a: &TaskId, b: &TaskId| keys[index[a]].cmp(&keys[index[b]]);
        self.roots.sort_by(by_key);
        for c in self.children.iter_mut() {
            c.sort_by(by_key);
        }
        // Renumber in the new pre-order.
        let mut order: Vec<usize> = Vec::with_capacity(n);
        let mut stack: Vec<usize> = self.roots.iter().rev().map(|r| self.index[r]).collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            stack.extend(self.children[i].iter().rev().map(|c| self.index[c]));
        }
        let mut new_of = vec![NONE; n];
        for (new, &old) in order.iter().enumerate() {
            new_of[old] = new;
        }
        let mut tasks: Vec<Option<Task>> = std::mem::take(&mut self.tasks).into_iter().map(Some).collect();
        let mut children = std::mem::take(&mut self.children);
        self.tasks = order.iter().map(|&o| tasks[o].take().expect("each once")).collect();
        self.children = order.iter().map(|&o| std::mem::take(&mut children[o])).collect();
        self.parent = order.iter().map(|&o| self.parent[o].map(|p| new_of[p])).collect();
        self.index = self.tasks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    }

    /// Tasks of this tree that `id` requires (unknown and hidden ones left out).
    pub fn blockers(&self, id: TaskId) -> Vec<TaskId> {
        self.get(id)
            .map(|t| t.blocked_by.iter().copied().filter(|b| self.contains(*b)).collect())
            .unwrap_or_default()
    }

    /// Tasks of this tree that require `id`, in tree order.
    pub fn blocking(&self, id: TaskId) -> Vec<TaskId> {
        self.tasks
            .iter()
            .filter(|t| t.blocked_by.contains(&id))
            .map(|t| t.id)
            .collect()
    }

    /// Done, or under a done ancestor.
    pub fn is_effectively_done(&self, id: TaskId) -> bool {
        self.get(id).is_some_and(|t| t.is_done())
            || self
                .ancestors(id)
                .iter()
                .any(|a| self.get(*a).is_some_and(|t| t.is_done()))
    }

    /// Has a requirement (in this tab or another) that is not done yet.
    pub fn is_blocked(&self, id: TaskId) -> bool {
        self.get(id).is_some_and(|t| {
            t.blocked_by.iter().any(|&b| {
                if self.contains(b) {
                    !self.is_effectively_done(b)
                } else {
                    self.external.get(&b) == Some(&false)
                }
            })
        })
    }

    /// Does `from` require `to`, directly or through other requirements?
    pub fn requires_transitively(&self, from: TaskId, to: TaskId) -> bool {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![from];
        while let Some(t) = stack.pop() {
            if t == to {
                return true;
            }
            if seen.insert(t)
                && let Some(task) = self.get(t)
            {
                stack.extend(task.blocked_by.iter().copied());
            }
        }
        false
    }

    /// The most prioritary actionable task:
    /// - not done, with no done ancestor (done hides the subtree's remaining work);
    /// - a leaf with respect to open work (no child still to do);
    /// - not blocked by a task still to do;
    /// - ordered by score (highest first, see [`crate::score`]), then due
    ///   date (earliest first, none last), then tree order.
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
        let scores = self.score_vec(&active);
        let key = |i: usize| {
            let t = &self.tasks[i];
            let s = scores[i].map(|s| s.total()).unwrap_or(0);
            (std::cmp::Reverse(s), t.due.is_none(), t.due, i)
        };
        let blocked = |i: usize| {
            self.tasks[i].blocked_by.iter().any(|b| match self.index.get(b) {
                Some(&j) => active[j],
                None => self.external.get(b) == Some(&false),
            })
        };
        (0..n)
            .filter(|&i| active[i] && !open_child[i] && !blocked(i))
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
            blocked_by: vec![],
            author: None,
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

    #[test]
    fn blocked_tasks_are_not_next() {
        let (a, b, c) = (TaskId::new(), TaskId::new(), TaskId::new());
        let mut ta = task(a, None, "1");
        ta.priority = 9;
        ta.blocked_by = vec![b, TaskId::new()]; // unknown blockers are ignored
        let tb = task(b, None, "2");
        let mut tc = task(c, None, "3");
        tc.blocked_by = vec![a];
        let t = Tree::build(vec![ta.clone(), tb.clone(), tc.clone()]);
        assert!(t.is_blocked(a) && !t.is_blocked(b) && t.is_blocked(c));
        assert_eq!(t.blockers(a), vec![b]);
        assert_eq!(t.blocking(a), vec![c]);
        assert_eq!(t.next_task(), Some(b));
        assert!(t.requires_transitively(c, b) && !t.requires_transitively(b, c));
        // blocker done -> unblocked
        let mut tb_done = tb.clone();
        tb_done.status = Status::Done;
        let t = Tree::build(vec![ta.clone(), tb_done, tc.clone()]);
        assert!(!t.is_blocked(a));
        assert_eq!(t.next_task(), Some(a));
        // a deleted blocker no longer blocks
        let mut tb_del = tb;
        tb_del.deleted = true;
        let t = Tree::build(vec![ta, tb_del, tc]);
        assert_eq!(t.next_task(), Some(a));
    }

    #[test]
    fn siblings_sorted_by_score() {
        let p = TaskId::new();
        let mk = |title: &str, parent: Option<TaskId>, pos: &str| {
            let mut t = task(TaskId::new(), parent, pos);
            t.title = title.into();
            t
        };
        let mut a = mk("a", None, "a");
        let mut b = mk("b", None, "b");
        let c = mk("c", None, "c");
        let mut d = mk("d", None, "d");
        let mut parent = task(p, None, "e");
        parent.title = "p".into();
        let (k1, mut k2) = (mk("k1", Some(p), "a"), mk("k2", Some(p), "b"));
        a.status = Status::Done;
        a.priority = 9;
        b.priority = 1;
        d.priority = 1;
        d.due = Some(jiff::civil::date(2026, 12, 1));
        k2.priority = 2;
        let mut t = Tree::build(vec![a, b, c, d, parent, k1, k2]);
        t.set_today(jiff::civil::date(2026, 11, 20));
        t.sort_by_score();
        let titles: Vec<&str> = t.iter().map(|x| x.title.as_str()).collect();
        // d: 1 + due; b: 1; then c and p (score 0, creation order); a done last
        assert_eq!(titles, ["d", "b", "c", "p", "k2", "k1", "a"]);
        assert_eq!(t.children(Some(p)).len(), 2);
        assert_eq!(t.parent(t.children(Some(p))[0]), Some(p));
        let rows: Vec<usize> = t.rows(&Default::default(), false).iter().map(|r| r.depth).collect();
        assert_eq!(rows, [0, 0, 0, 0, 1, 1, 0]);
    }

    #[test]
    fn scores_and_ranking() {
        use jiff::civil::date;
        let today = date(2026, 11, 20);
        let ids: Vec<TaskId> = (0..5).map(|_| TaskId::new()).collect();
        let mk = |i: usize, prio: i64, due: Option<jiff::civil::Date>| {
            let mut t = task(ids[i], None, &i.to_string());
            t.priority = prio;
            t.due = due;
            t
        };
        let build = |tasks: Vec<Task>| {
            let mut t = Tree::build(tasks);
            t.set_today(today);
            t
        };
        // same priority: a deadline wins, the nearer the better
        let t = build(vec![
            mk(0, 1, None),
            mk(1, 1, Some(date(2027, 6, 1))),
            mk(2, 1, Some(date(2026, 11, 22))),
        ]);
        assert_eq!(t.next_task(), Some(ids[2]));
        let s = t.scores();
        assert_eq!(s[&ids[0]].total(), 100);
        assert!(s[&ids[1]].total() > 100 && s[&ids[1]].total() < s[&ids[2]].total());
        // but priority dominates
        let t = build(vec![mk(0, 2, None), mk(2, 1, Some(date(2026, 11, 1)))]);
        assert_eq!(t.next_task(), Some(ids[0]));

        // blocking adds the blocked tasks' scores, transitively
        let a = mk(0, 0, None); // blocks b
        let mut b = mk(1, 3, None); // blocks c
        b.blocked_by = vec![ids[0]];
        let mut c = mk(2, 5, None);
        c.blocked_by = vec![ids[1]];
        let d = mk(3, 6, None); // unrelated, higher than anything on its own
        let t = build(vec![a.clone(), b.clone(), c.clone(), d.clone()]);
        let s = t.scores();
        assert_eq!(s[&ids[2]].total(), 500);
        assert_eq!(s[&ids[1]].total(), 300 + 500);
        assert_eq!((s[&ids[0]].blocking, s[&ids[0]].total()), (800, 800));
        assert_eq!(t.next_task(), Some(ids[0]), "unblocks 800 worth of work");
        // a done blocked task no longer counts
        let mut c_done = c.clone();
        c_done.status = Status::Done;
        let t = build(vec![a.clone(), b.clone(), c_done, d.clone()]);
        assert_eq!(t.scores()[&ids[0]].total(), 300);
        assert_eq!(t.next_task(), Some(ids[3]));
        // cycles (concurrent edits) terminate
        let mut a2 = a;
        a2.blocked_by = vec![ids[2]];
        let t = build(vec![a2, b, c, d]);
        assert!(t.scores().len() == 4);
        assert_eq!(t.next_task(), Some(ids[3]));
    }
}
