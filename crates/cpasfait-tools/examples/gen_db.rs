//! Generate a database with many tasks and work sessions, for benchmarks and demos.
//!
//! cargo run --release -p cpasfait-tools --example gen_db -- PATH [TASKS]

use cpasfait_core::{Change, Place, Repo, TaskId};
use rand::RngExt;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: gen_db PATH [TASKS]");
    let n: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(10_000);
    let mut repo = Repo::open(&path)?;
    let mut rng = rand::rng();
    let now = repo.now_ms();
    for tab_name in ["work", "home"] {
        let tab = repo.create_tab(tab_name)?;
        let mut ids: Vec<TaskId> = vec![];
        for i in 0..n / 2 {
            let parent = if ids.is_empty() || rng.random_bool(0.3) {
                None
            } else {
                Some(ids[rng.random_range(0..ids.len())])
            };
            let id = repo.add_task(tab, Place::LastChildOf(parent), &format!("{tab_name} task {i}"))?;
            let mut edits = vec![];
            if rng.random_bool(0.3) {
                edits.push((tab, id.0, Change::TaskPriority(rng.random_range(1..4))));
            }
            if rng.random_bool(0.5) {
                edits.push((tab, id.0, Change::TaskStatus(cpasfait_core::Status::Done)));
            }
            if rng.random_bool(0.2) {
                let d = jiff::civil::date(2026, rng.random_range(1..13), rng.random_range(1..29));
                edits.push((tab, id.0, Change::TaskDue(Some(d))));
            }
            if rng.random_bool(0.2) {
                edits.push((
                    tab,
                    id.0,
                    Change::TaskTag {
                        tag: "tag".into(),
                        on: true,
                    },
                ));
            }
            if rng.random_bool(0.1) {
                // a past work session
                let sid = cpasfait_core::SessionId::new();
                let start = now - rng.random_range(0..14 * 86_400_000i64);
                let end = start + rng.random_range(60_000..3 * 3_600_000i64);
                edits.extend([
                    (tab, sid.0, Change::SessionTask(id)),
                    (tab, sid.0, Change::SessionUser("bench".into())),
                    (tab, sid.0, Change::SessionStart(start)),
                    (tab, sid.0, Change::SessionEnd(Some(end))),
                ]);
            }
            repo.commit(edits)?;
            ids.push(id);
        }
    }
    println!("{} ops", repo.all_ops()?.len());
    Ok(())
}
