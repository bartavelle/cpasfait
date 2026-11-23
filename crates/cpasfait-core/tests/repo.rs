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
    // siblings are sorted by score; equal scores keep creation order
    r.set_priority(c, 1).unwrap();
    assert_eq!(titles(&r), "z,a, c, b");
    r.set_priority(c, 0).unwrap();
    assert_eq!(titles(&r), "z,a, b, c");
    assert!(r.outdent(c).unwrap());
    assert_eq!(titles(&r), "z,a, b,c");
    r.set_priority(c, 2).unwrap();
    assert_eq!(titles(&r), "c,z,a, b");
    // indent goes under the task displayed above
    assert!(r.indent(z).unwrap());
    assert_eq!(titles(&r), "c, z,a, b");
    assert!(r.move_task(a, Place::LastChildOf(Some(b))).is_err());
    r.delete_task(a).unwrap();
    assert_eq!(titles(&r), "c, z");
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
    r.set_priority(a, 3).unwrap();

    r.set_blocked_by(a, b, true).unwrap();
    assert_eq!(r.task(a).unwrap().blocked_by, vec![b]);
    // a has an open child and is blocked by b: b inherits a's score (600)
    // and beats the child (300)
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, b);
    r.toggle_done(b).unwrap();
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, child);
    r.toggle_done(child).unwrap();
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, a);
    r.toggle_done(b).unwrap(); // reopen for the checks below

    assert!(r.set_blocked_by(a, a, true).is_err());
    assert!(r.set_blocked_by(b, a, true).is_err(), "cycle");
    assert!(r.set_blocked_by(child, a, true).is_err(), "requires its parent");
    // other tabs are fine
    r.set_blocked_by(a, elsewhere, true).unwrap();
    assert!(r.set_blocked_by(elsewhere, a, true).is_err(), "cycle across tabs");
    r.set_blocked_by(a, elsewhere, false).unwrap();
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
        c.execute_batch("DROP TABLE task_dep; DROP TABLE tab_member; DROP TABLE tab_epoch; DROP TABLE key_file; DROP TABLE tab_setting; ALTER TABLE tab DROP COLUMN move_id; ALTER TABLE tab DROP COLUMN move_remote; ALTER TABLE tab DROP COLUMN move_by; ALTER TABLE tab DROP COLUMN move_done; ALTER TABLE task DROP COLUMN author_name; ALTER TABLE task DROP COLUMN author_email; ALTER TABLE device DROP COLUMN email; PRAGMA user_version = 1;")
            .unwrap();
    }
    // The read-only open used by the companion tools migrates too.
    let r = Repo::open_readonly(&path).unwrap();
    assert_eq!(r.task(a).unwrap().blocked_by, vec![b]);
}

#[test]
fn dependencies_across_tabs() {
    use cpasfait_core::DepState;
    use cpasfait_core::backend::localdir::LocalDir;
    use cpasfait_core::crypto::Identity;
    use cpasfait_core::sync::sync_tab;
    let mut r1 = Repo::open_in_memory().unwrap();
    let work = r1.create_tab("work").unwrap();
    let home = r1.create_tab("home").unwrap();
    let report = r1.add_task(work, Place::LastChildOf(None), "report").unwrap();
    let other = r1.add_task(work, Place::LastChildOf(None), "other").unwrap();
    let car = r1.add_task(home, Place::LastChildOf(None), "fix car").unwrap();
    r1.set_priority(report, 3).unwrap();
    r1.set_blocked_by(report, car, true).unwrap();

    assert!(r1.tree(work).unwrap().is_blocked(report));
    // "fix car" inherits the score of the task it blocks in another tab
    let s = r1.tree(home).unwrap().scores()[&car];
    assert_eq!((s.priority, s.blocking), (300, 600));
    assert_eq!(r1.structure(home).unwrap().scores()[&car].blocking, 600);
    assert_eq!(r1.next_task(work).unwrap().unwrap().id, other);
    let d = r1.dependencies(report).unwrap();
    assert_eq!(d.requires.len(), 1);
    assert_eq!(d.requires[0].tab_name.as_deref(), Some("home"));
    assert_eq!(d.requires[0].title.as_deref(), Some("fix car"));
    assert_eq!(d.requires[0].state, DepState::Todo);
    assert_eq!(r1.dependencies(car).unwrap().blocks[0].task, report);

    r1.toggle_done(car).unwrap();
    assert_eq!(r1.next_task(work).unwrap().unwrap().id, report);
    assert_eq!(r1.dependencies(report).unwrap().requires[0].state, DepState::Done);
    r1.toggle_done(car).unwrap();

    // Another device that only shares "work" knows nothing about "home".
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (k1, k2) = (Identity::generate(), Identity::generate());
    r1.share_tab(work, k1.recipient()).unwrap();
    r1.set_member(work, k2.recipient(), Some("bob")).unwrap();
    sync_tab(&mut r1, work, &backend, &k1).unwrap();
    let mut r2 = Repo::open_in_memory().unwrap();
    r2.add_known_tab(work).unwrap();
    sync_tab(&mut r2, work, &backend, &k2).unwrap();
    let d = r2.dependencies(report).unwrap();
    assert_eq!(d.requires[0].state, DepState::UnknownTab);
    assert_eq!(d.requires[0].tab, Some(home));
    assert_eq!(d.requires[0].title, None);
    // unknown requirements do not block
    assert!(!r2.tree(work).unwrap().is_blocked(report));
    // Once the tab is known but not synced yet, the task is "missing".
    r2.add_known_tab(home).unwrap();
    assert_eq!(r2.dependencies(report).unwrap().requires[0].state, DepState::Missing);

    // Deleting the blocker unblocks.
    r1.delete_task(car).unwrap();
    assert_eq!(r1.dependencies(report).unwrap().requires[0].state, DepState::Deleted);
    assert!(!r1.tree(work).unwrap().is_blocked(report));
}

#[test]
fn tab_settings() {
    let mut r = Repo::open_in_memory().unwrap();
    let tab = r.create_tab("work").unwrap();
    let other = r.create_tab("home").unwrap();
    assert_eq!(r.tab_settings(tab).unwrap().default_priority, 0);
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();

    r.set_tab_setting(tab, "default_priority", Some(" 2 ")).unwrap();
    assert_eq!(r.tab_settings(tab).unwrap().default_priority, 2);
    assert_eq!(r.tab_settings(other).unwrap().default_priority, 0);
    let b = r.add_task(tab, Place::LastChildOf(Some(a)), "b").unwrap();
    let c = r.add_task(other, Place::LastChildOf(None), "c").unwrap();
    assert_eq!(r.task(b).unwrap().priority, 2);
    assert_eq!(r.task(c).unwrap().priority, 0);
    // existing tasks are not changed
    assert_eq!(r.task(a).unwrap().priority, 0);

    assert!(r.set_tab_setting(tab, "default_priority", Some("high")).is_err());
    assert!(r.set_tab_setting(tab, "colour", Some("1")).is_err());
    assert!(r.set_tab_setting(tab, "colour", None).is_err());

    // reset; the read model survives a rebuild
    r.set_tab_setting(other, "default_priority", Some("-1")).unwrap();
    r.set_tab_setting(tab, "default_priority", None).unwrap();
    let before = r.dump_model().unwrap();
    r.rebuild().unwrap();
    assert_eq!(r.dump_model().unwrap(), before);
    assert_eq!(r.tab_settings(tab).unwrap().default_priority, 0);
    assert_eq!(r.tab_settings(other).unwrap().default_priority, -1);
}

#[test]
fn tasks_record_their_author() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let (a, b) = {
        let mut r = Repo::open(&path).unwrap();
        r.set_user("alice").unwrap();
        let tab = r.create_tab("t").unwrap();
        let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
        r.set_email(Some(" alice@example.org ")).unwrap();
        let b = r.add_task(tab, Place::LastChildOf(None), "b").unwrap();
        (a, b)
    };
    let r = Repo::open_readonly(&path).unwrap();
    assert_eq!(r.email(), Some("alice@example.org"));
    let author = |t| r.task(t).unwrap().author.unwrap().to_string();
    assert_eq!(author(a), "alice");
    assert_eq!(author(b), "alice <alice@example.org>");
}

#[test]
fn work_times_entered_afterwards() {
    let (mut r, time) = repo_with_clock();
    let start = time.load(Ordering::SeqCst);
    let min = 60_000;
    let tab = r.create_tab("work").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    let b = r.add_task(tab, Place::LastChildOf(None), "b").unwrap();
    r.start_work(a).unwrap();
    time.fetch_add(60 * min, Ordering::SeqCst);
    // forgot to switch to b 20 minutes ago
    assert!(r.start_work_at(b, start - min).is_err());
    assert!(r.start_work_at(b, start + 61 * min).is_err());
    r.start_work_at(b, start + 40 * min).unwrap();
    assert_eq!(r.sessions_for_task(a).unwrap()[0].end, Some(start + 40 * min));
    let sb = r.active_session().unwrap().unwrap();
    assert_eq!((sb.task, sb.start), (Some(b), Some(start + 40 * min)));
    // and to stop 5 minutes ago
    assert!(r.stop_work_at(start + 39 * min).is_err());
    assert_eq!(r.active_session().unwrap().map(|s| s.id), Some(sb.id));
    assert_eq!(r.stop_work_at(start + 55 * min).unwrap(), vec![sb.id]);
    assert_eq!(r.sessions_for_task(b).unwrap()[0].duration_ms(0), 15 * min);
    // nothing running: a back-dated start
    r.start_work_at(a, start + 58 * min).unwrap();
    assert_eq!(r.active_session().unwrap().unwrap().start, Some(start + 58 * min));
}

#[test]
fn back_dated_start_cannot_overlap_the_last_session() {
    let (mut r, time) = repo_with_clock();
    let t0 = time.load(Ordering::SeqCst);
    let tab = r.create_tab("work").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    r.start_work(a).unwrap();
    time.fetch_add(60_000, Ordering::SeqCst);
    r.stop_work().unwrap();
    assert_eq!(r.last_work_end().unwrap(), Some(t0 + 60_000));
    time.fetch_add(60_000, Ordering::SeqCst);
    assert!(r.start_work_at(a, t0 + 30_000).is_err());
    r.start_work_at(a, t0 + 60_000).unwrap();
}

#[test]
fn report_by_tags() {
    let (mut r, time) = repo_with_clock();
    let work = r.create_tab("work").unwrap();
    let home = r.create_tab("home").unwrap();
    let proj = r.add_task(work, Place::LastChildOf(None), "project").unwrap();
    r.set_tag(proj, "acme", true).unwrap();
    let dev = r.add_task(work, Place::LastChildOf(Some(proj)), "dev").unwrap();
    r.set_tag(dev, "dev", true).unwrap();
    let meet = r.add_task(work, Place::LastChildOf(Some(proj)), "meeting").unwrap();
    let chores = r.add_task(home, Place::LastChildOf(None), "chores").unwrap();
    let min = 60_000;
    for (task, mins) in [(dev, 30), (meet, 20), (chores, 10), (dev, 15)] {
        r.start_work(task).unwrap();
        time.fetch_add(mins * min, Ordering::SeqCst);
    }
    r.stop_work().unwrap();
    let all = report::slices(&r, 0, i64::MAX, &Filter::default()).unwrap();
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        report::by_tag_set(&r, &all).unwrap(),
        vec![
            (s(&["acme", "dev"]), 45 * min),
            (s(&["acme"]), 20 * min),
            (vec![], 10 * min)
        ]
    );
    assert_eq!(
        report::by_tag(&r, &all).unwrap(),
        vec![
            (Some("acme".into()), 65 * min),
            (Some("dev".into()), 45 * min),
            (None, 10 * min)
        ]
    );
}

#[test]
fn low_priorities() {
    let mut r = Repo::open_in_memory().unwrap();
    let tab = r.create_tab("t").unwrap();
    let low = r.add_task(tab, Place::LastChildOf(None), "low").unwrap();
    let normal = r.add_task(tab, Place::LastChildOf(None), "normal").unwrap();
    let lowest = r.add_task(tab, Place::LastChildOf(None), "lowest").unwrap();
    r.set_priority(low, -1).unwrap();
    r.set_priority(lowest, -3).unwrap();
    assert!(r.set_priority(low, -4).is_err());
    assert!(r.set_priority(low, 4).is_err());
    assert_eq!(r.tree(tab).unwrap().children(None), &[normal, low, lowest]);
    // the next task is still the highest score, even when all are low
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, normal);
    r.toggle_done(normal).unwrap();
    assert_eq!(r.next_task(tab).unwrap().unwrap().id, low);
    // the tab setting has the same range
    assert!(r.set_tab_setting(tab, "default_priority", Some("-4")).is_err());
    r.set_tab_setting(tab, "default_priority", Some("-2")).unwrap();
    let t = r.add_task(tab, Place::LastChildOf(None), "x").unwrap();
    assert_eq!(r.task(t).unwrap().priority, -2);
}
