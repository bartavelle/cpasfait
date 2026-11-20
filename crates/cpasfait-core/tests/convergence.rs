//! Property tests: replicas exchanging ops in arbitrary orders converge, and
//! the incremental read model equals a full rebuild.

use cpasfait_core::{Place, Repo, TabId};
use proptest::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Clone, Debug)]
enum Action {
    Add {
        dev: usize,
        parent: Option<usize>,
        title: u8,
    },
    Title {
        dev: usize,
        task: usize,
        title: u8,
    },
    Done {
        dev: usize,
        task: usize,
    },
    Move {
        dev: usize,
        task: usize,
        target: usize,
        child: bool,
    },
    Delete {
        dev: usize,
        task: usize,
    },
    Tag {
        dev: usize,
        task: usize,
        on: bool,
    },
    Work {
        dev: usize,
        task: usize,
    },
    Stop {
        dev: usize,
    },
    Exchange {
        from: usize,
        to: usize,
    },
    Tick(i64),
}

const DEVS: usize = 3;

fn action() -> impl Strategy<Value = Action> {
    let dev = 0..DEVS;
    let idx = 0usize..20;
    prop_oneof![
        3 => (dev.clone(), proptest::option::of(idx.clone()), any::<u8>()).prop_map(|(dev, parent, title)| Action::Add { dev, parent, title }),
        1 => (dev.clone(), idx.clone(), any::<u8>()).prop_map(|(dev, task, title)| Action::Title { dev, task, title }),
        1 => (dev.clone(), idx.clone()).prop_map(|(dev, task)| Action::Done { dev, task }),
        2 => (dev.clone(), idx.clone(), idx.clone(), any::<bool>()).prop_map(|(dev, task, target, child)| Action::Move { dev, task, target, child }),
        1 => (dev.clone(), idx.clone()).prop_map(|(dev, task)| Action::Delete { dev, task }),
        1 => (dev.clone(), idx.clone(), any::<bool>()).prop_map(|(dev, task, on)| Action::Tag { dev, task, on }),
        1 => (dev.clone(), idx.clone()).prop_map(|(dev, task)| Action::Work { dev, task }),
        1 => dev.clone().prop_map(|dev| Action::Stop { dev }),
        2 => (dev.clone(), dev.clone()).prop_map(|(from, to)| Action::Exchange { from, to }),
        1 => (-5i64..50).prop_map(Action::Tick),
    ]
}

fn pick(repo: &Repo, tab: TabId, i: usize) -> Option<cpasfait_core::TaskId> {
    let mut ids: Vec<_> = repo
        .tasks(tab)
        .unwrap()
        .into_iter()
        .filter(|t| !t.deleted)
        .map(|t| t.id)
        .collect();
    ids.sort();
    (!ids.is_empty()).then(|| ids[i % ids.len()])
}

fn run(actions: Vec<Action>) -> Result<(), TestCaseError> {
    let time = Arc::new(AtomicI64::new(1_000_000));
    let mut repos: Vec<Repo> = (0..DEVS)
        .map(|_| {
            let mut r = Repo::open_in_memory().unwrap();
            let t = time.clone();
            r.set_now(move || t.load(Ordering::SeqCst));
            r
        })
        .collect();
    let tab = repos[0].create_tab("shared").unwrap();
    let ops = repos[0].all_ops().unwrap();
    for r in &mut repos[1..] {
        r.ingest(&ops).unwrap();
    }
    for a in actions {
        match a {
            Action::Add { dev, parent, title } => {
                let p = parent.and_then(|i| pick(&repos[dev], tab, i));
                repos[dev]
                    .add_task(tab, Place::LastChildOf(p), &title.to_string())
                    .unwrap();
            }
            Action::Title { dev, task, title } => {
                if let Some(t) = pick(&repos[dev], tab, task) {
                    repos[dev].set_title(t, &title.to_string()).unwrap();
                }
            }
            Action::Done { dev, task } => {
                if let Some(t) = pick(&repos[dev], tab, task) {
                    repos[dev].toggle_done(t).unwrap();
                }
            }
            Action::Move {
                dev,
                task,
                target,
                child,
            } => {
                let r = &mut repos[dev];
                if let (Some(t), Some(target)) = (pick(r, tab, task), pick(r, tab, target)) {
                    let tree = r.tree(tab).unwrap();
                    if t != target && tree.contains(t) && tree.contains(target) {
                        let place = if child {
                            Place::LastChildOf(Some(target))
                        } else {
                            Place::After(target)
                        };
                        // Moving under a descendant is refused locally; fine.
                        let _ = r.move_task(t, place);
                    }
                }
            }
            Action::Delete { dev, task } => {
                if let Some(t) = pick(&repos[dev], tab, task) {
                    repos[dev].delete_task(t).unwrap();
                }
            }
            Action::Tag { dev, task, on } => {
                if let Some(t) = pick(&repos[dev], tab, task) {
                    repos[dev].set_tag(t, "x", on).unwrap();
                }
            }
            Action::Work { dev, task } => {
                if let Some(t) = pick(&repos[dev], tab, task) {
                    repos[dev].toggle_work(t).unwrap();
                }
            }
            Action::Stop { dev } => {
                repos[dev].stop_work().unwrap();
            }
            Action::Exchange { from, to } => {
                if from != to {
                    let ops = repos[from].all_ops().unwrap();
                    repos[to].ingest(&ops).unwrap();
                }
            }
            Action::Tick(d) => {
                time.fetch_add(d, Ordering::SeqCst);
            }
        }
        // Local invariant: at most one open session per device.
        for r in &repos {
            let open = r
                .open_sessions(tab)
                .unwrap()
                .into_iter()
                .filter(|s| s.device == Some(r.device()))
                .count();
            prop_assert!(open <= 1);
        }
    }
    // Full exchange: everyone gets everything, in reversed order for some.
    let mut all = vec![];
    for r in &repos {
        all.extend(r.all_ops().unwrap());
    }
    for (i, r) in repos.iter_mut().enumerate() {
        if i % 2 == 1 {
            all.reverse();
        }
        r.ingest(&all).unwrap();
    }
    let dumps: Vec<String> = repos.iter().map(|r| r.dump_model().unwrap()).collect();
    for d in &dumps[1..] {
        prop_assert_eq!(&dumps[0], d);
    }
    // Incremental == rebuild.
    repos[0].rebuild().unwrap();
    prop_assert_eq!(&dumps[0], &repos[0].dump_model().unwrap());
    // The tree of the converged state is well-formed (no panic, every task placed once).
    let tree = repos[0].tree(tab).unwrap();
    let rows = tree.rows(&Default::default(), false);
    prop_assert_eq!(rows.len(), tree.len());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]
    #[test]
    fn replicas_converge(actions in proptest::collection::vec(action(), 1..60)) {
        run(actions)?;
    }
}
