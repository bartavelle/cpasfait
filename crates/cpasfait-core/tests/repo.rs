use cpasfait_core::report::{self, Filter};
use cpasfait_core::{Place, Repo, Status};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

fn repo_with_clock() -> (Repo, Arc<AtomicI64>) {
    let time = Arc::new(AtomicI64::new(1_000_000));
    let mut r = Repo::open_in_memory().unwrap();
    let t = time.clone();
    r.set_now(move || t.load(Ordering::SeqCst));
    (r, time)
}

#[test]
fn ordering_and_moves() {
    let (mut r, _) = repo_with_clock();
    let tab = r.create_tab("home").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    let c = r.add_task(tab, Place::LastChildOf(None), "c").unwrap();
    let b = r.add_task(tab, Place::After(a), "b").unwrap();
    let z = r.add_task(tab, Place::FirstChildOf(None), "z").unwrap();
    let titles = |r: &Repo| {
        let tree = r.tree(tab).unwrap();
        tree.rows(&Default::default(), false)
            .iter()
            .map(|row| format!("{}{}", " ".repeat(row.depth), tree.get(row.id).unwrap().title))
            .collect::<Vec<_>>()
            .join(",")
    };
    assert_eq!(titles(&r), "z,a,b,c");
    assert!(r.indent(b).unwrap());
    assert_eq!(titles(&r), "z,a, b,c");
    assert!(!r.indent(z).unwrap());
    assert!(r.indent(c).unwrap());
    assert_eq!(titles(&r), "z,a, b, c");
    assert!(r.shift(c, true).unwrap());
    assert_eq!(titles(&r), "z,a, c, b");
    assert!(r.outdent(c).unwrap());
    assert_eq!(titles(&r), "z,a, b,c");
    assert!(r.move_task(a, Place::LastChildOf(Some(b))).is_err());
    r.delete_task(a).unwrap();
    assert_eq!(titles(&r), "z,c");
}

#[test]
fn work_log() {
    let (mut r, time) = repo_with_clock();
    let tab = r.create_tab("work").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    let b = r.add_task(tab, Place::LastChildOf(None), "b").unwrap();
    assert!(r.toggle_work(a).unwrap());
    time.fetch_add(60_000, Ordering::SeqCst);
    // starting b auto-stops a
    r.start_work(b).unwrap();
    let active = r.active_session().unwrap().unwrap();
    assert_eq!(active.task, Some(b));
    let sa = r.sessions_for_task(a).unwrap();
    assert_eq!(sa.len(), 1);
    assert_eq!(sa[0].duration_ms(0), 60_000);
    // starting b again is a no-op
    let id = r.start_work(b).unwrap();
    assert_eq!(id, active.id);
    time.fetch_add(30_000, Ordering::SeqCst);
    assert!(!r.toggle_work(b).unwrap());
    assert!(r.active_session().unwrap().is_none());
    assert_eq!(r.sessions_for_task(b).unwrap()[0].duration_ms(0), 30_000);
}

#[test]
fn concurrent_sessions_on_same_task_both_survive() {
    let (mut r1, _) = repo_with_clock();
    let (mut r2, _) = repo_with_clock();
    let tab = r1.create_tab("shared").unwrap();
    let a = r1.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    r2.ingest(&r1.all_ops().unwrap()).unwrap();
    r1.start_work(a).unwrap();
    r2.start_work(a).unwrap();
    r1.ingest(&r2.all_ops().unwrap()).unwrap();
    r2.ingest(&r1.all_ops().unwrap()).unwrap();
    assert_eq!(r1.open_sessions(tab).unwrap().len(), 2);
    assert_eq!(r1.dump_model().unwrap(), r2.dump_model().unwrap());
    // each device only stops its own
    r1.stop_work().unwrap();
    assert_eq!(r1.open_sessions(tab).unwrap().len(), 1);
}

#[test]
fn report_rollup_and_clipping() {
    let (mut r, time) = repo_with_clock();
    let tab = r.create_tab("work").unwrap();
    let p = r.add_task(tab, Place::LastChildOf(None), "parent").unwrap();
    let c = r.add_task(tab, Place::LastChildOf(Some(p)), "child").unwrap();
    let t0 = time.load(Ordering::SeqCst);
    r.start_work(p).unwrap();
    time.fetch_add(10_000, Ordering::SeqCst);
    r.start_work(c).unwrap();
    time.fetch_add(20_000, Ordering::SeqCst);
    r.stop_work().unwrap();
    // open session counted until now
    r.start_work(c).unwrap();
    time.fetch_add(5_000, Ordering::SeqCst);

    let all = report::slices(&r, 0, i64::MAX, &Filter::default()).unwrap();
    let rep = report::by_task(&r, &all).unwrap();
    assert_eq!(rep.len(), 1);
    assert_eq!(rep[0].total_ms, 35_000);
    let lines: Vec<_> = rep[0]
        .lines
        .iter()
        .map(|l| (l.title.as_str(), l.depth, l.own_ms, l.total_ms))
        .collect();
    assert_eq!(lines, vec![("parent", 0, 10_000, 35_000), ("child", 1, 25_000, 25_000)]);

    // clip: only [t0+5s, t0+15s)
    let clipped = report::slices(&r, t0 + 5_000, t0 + 15_000, &Filter::default()).unwrap();
    assert_eq!(clipped.iter().map(|s| s.end - s.start).sum::<i64>(), 10_000);

    // deleted tasks still count
    r.delete_task(p).unwrap();
    let rep = report::by_task(&r, &all).unwrap();
    assert_eq!(rep[0].lines.len(), 1);
    assert_eq!(rep[0].lines[0].total_ms, 35_000);
}

#[test]
fn by_day_splits_midnight() {
    use jiff::tz::TimeZone;
    let tz = TimeZone::UTC;
    let midnight = jiff::civil::date(2026, 11, 20)
        .to_zoned_datetime(tz.clone())
        .unwrap()
        .timestamp()
        .as_millisecond();
    let s = report::Slice {
        tab: cpasfait_core::TabId::new(),
        task: None,
        user: "u".into(),
        start: midnight - 3_600_000,
        end: midnight + 7_200_000,
    };
    let days = report::by_day(&[s], &tz);
    assert_eq!(
        days,
        vec![
            (jiff::civil::date(2026, 11, 19), 3_600_000),
            (jiff::civil::date(2026, 11, 20), 7_200_000)
        ]
    );
}

#[test]
fn next_task_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    let tab;
    {
        let mut r = Repo::open(&path).unwrap();
        tab = r.create_tab("work").unwrap();
        let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
        let b = r.add_task(tab, Place::LastChildOf(None), "b").unwrap();
        r.set_priority(b, 2).unwrap();
        r.set_tag(a, "x", true).unwrap();
        r.set_due(a, Some(jiff::civil::date(2026, 1, 1))).unwrap();
        r.set_status(b, Status::Done).unwrap();
    }
    let r = Repo::open_readonly(&path).unwrap();
    let next = r.next_task(r.find_tab("wo").unwrap().id).unwrap().unwrap();
    assert_eq!(next.title, "a");
    assert_eq!(next.tags, vec!["x"]);
    assert_eq!(next.due, Some(jiff::civil::date(2026, 1, 1)));
    let _ = tab;
}

#[test]
fn dependencies() {
    let mut r = Repo::open_in_memory().unwrap();
    let tab = r.create_tab("t").unwrap();
    let other = r.create_tab("o").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    let b = r.add_task(tab, Place::LastChildOf(None), "b").unwrap();
    let child = r.add_task(tab, Place::LastChildOf(Some(a)), "child").unwrap();
    let elsewhere = r.add_task(other, Place::LastChildOf(None), "x").unwrap();
    r.set_priority(a, 5).unwrap();

    r.set_blocked_by(a, b, true).unwrap();
    assert_eq!(r.task(a).unwrap().blocked_by, vec![b]);
    // a has an open child: still not actionable; the child is
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, child);
    r.toggle_done(child).unwrap();
    // a is blocked by b
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, b);
    r.toggle_done(b).unwrap();
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, a);

    assert!(r.set_blocked_by(a, a, true).is_err());
    assert!(r.set_blocked_by(b, a, true).is_err(), "cycle");
    assert!(r.set_blocked_by(child, a, true).is_err(), "requires its parent");
    assert!(r.set_blocked_by(a, elsewhere, true).is_err(), "other tab");
    r.set_blocked_by(a, b, false).unwrap();
    assert!(r.task(a).unwrap().blocked_by.is_empty());
    // removing is always allowed, even if never set
    r.set_blocked_by(b, a, false).unwrap();
    let before = r.dump_model().unwrap();
    r.rebuild().unwrap();
    assert_eq!(r.dump_model().unwrap(), before);
}

#[test]
fn upgrade_from_v1_replays_dependency_ops() {
    // A v1 client stored a "task.blocked_by" op it did not understand.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let (a, b) = {
        let mut r = Repo::open(&path).unwrap();
        let tab = r.create_tab("t").unwrap();
        let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
        let b = r.add_task(tab, Place::LastChildOf(None), "b").unwrap();
        r.set_blocked_by(a, b, true).unwrap();
        (a, b)
    };
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("DROP TABLE task_dep; PRAGMA user_version = 1;")
            .unwrap();
    }
    // The read-only open used by the companion tools migrates too.
    let r = Repo::open_readonly(&path).unwrap();
    assert_eq!(r.task(a).unwrap().blocked_by, vec![b]);
}
