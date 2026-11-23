use cpasfait_core::taskfile::{self, Applied};
use cpasfait_core::{Place, Repo};
use jiff::tz::TimeZone;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

fn err<T>(res: cpasfait_core::Result<T>) -> String {
    res.err().map(|e| e.to_string()).unwrap_or_default()
}

fn ms(s: &str) -> i64 {
    s.parse::<jiff::civil::DateTime>()
        .unwrap()
        .to_zoned_datetime(TimeZone::UTC)
        .unwrap()
        .timestamp()
        .as_millisecond()
}

fn setup() -> (Repo, Arc<AtomicI64>) {
    let time = Arc::new(AtomicI64::new(ms("2026-11-23T09:00:00.250")));
    let mut r = Repo::open_in_memory().unwrap();
    r.set_user("me").unwrap();
    let t = time.clone();
    r.set_now(move || t.load(Ordering::SeqCst));
    (r, time)
}

fn edit(r: &mut Repo, task: cpasfait_core::TaskId, f: impl Fn(String) -> String) -> cpasfait_core::Result<Applied> {
    let text = f(taskfile::render(r, task, &TimeZone::UTC)?);
    let file = taskfile::parse(&text, &TimeZone::UTC)?.expect("not empty");
    taskfile::apply(r, task, &file)
}

fn spans(r: &Repo, task: cpasfait_core::TaskId) -> Vec<(i64, Option<i64>)> {
    let mut v: Vec<_> = r
        .sessions_for_task(task)
        .unwrap()
        .into_iter()
        .map(|s| (s.start.unwrap(), s.end))
        .collect();
    v.sort();
    v
}

#[test]
fn render_and_edit() {
    let (mut r, time) = setup();
    let tab = r.create_tab("work").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "Write report").unwrap();
    r.start_work(a).unwrap();
    time.store(ms("2026-11-23T10:30:00"), Ordering::SeqCst);
    r.stop_work().unwrap();
    let s = r.start_work(a).unwrap();
    r.set_session_note(s, "second part").unwrap();
    time.store(ms("2026-11-23T11:00:00"), Ordering::SeqCst);

    let text = taskfile::render(&r, a, &TimeZone::UTC).unwrap();
    assert!(text.starts_with("title: Write report\n"), "{text}");
    let log: Vec<_> = text.lines().skip_while(|l| *l != "work log:").skip(1).collect();
    assert_eq!(
        log,
        [
            "2026-11-23 09:00:00 - 2026-11-23 10:30:00",
            "2026-11-23 10:30:00 -                      # running, second part",
        ]
    );

    // unchanged: nothing recorded (sub-second times kept)
    let before = r.local_changes();
    assert_eq!(edit(&mut r, a, |t| t).unwrap(), Applied::default());
    assert_eq!(r.local_changes(), before);
    assert_eq!(spans(&r, a)[0].0, ms("2026-11-23T09:00:00.250"));

    // title, first session shortened, one added (seconds optional)
    let applied = edit(&mut r, a, |t| {
        t.replace("title: Write report", "title: Write the report")
            .replace("- 2026-11-23 10:30:00", "- 2026-11-23 10:00:00")
            + "2026-11-22 14:00 - 2026-11-22 15:15  # added\n"
    })
    .unwrap();
    assert_eq!(
        applied,
        Applied {
            title: true,
            added: 1,
            changed: 1,
            deleted: 0
        }
    );
    assert_eq!(r.task(a).unwrap().title, "Write the report");
    assert_eq!(
        spans(&r, a),
        [
            (ms("2026-11-22T14:00"), Some(ms("2026-11-22T15:15"))),
            (ms("2026-11-23T09:00"), Some(ms("2026-11-23T10:00"))),
            (ms("2026-11-23T10:30"), None),
        ]
    );
    // the running session keeps its note and stays this device's active one
    assert_eq!(r.active_session().unwrap().unwrap().id, s);

    // close the running session, delete the first one
    let applied = edit(&mut r, a, |t| {
        t.replace("2026-11-23 10:30:00 -  ", "2026-11-23 10:30:00 - 2026-11-23 10:45:00")
            .replace("2026-11-22 14:00:00 - 2026-11-22 15:15:00\n", "")
    })
    .unwrap();
    assert_eq!((applied.changed, applied.deleted), (1, 1));
    assert!(r.active_session().unwrap().is_none());
    assert_eq!(r.session(s).unwrap().note, "second part");
    assert_eq!(spans(&r, a).len(), 2);
}

#[test]
fn bad_edits_change_nothing() {
    let (mut r, time) = setup();
    let tab = r.create_tab("work").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    r.start_work(a).unwrap();
    time.store(ms("2026-11-23T10:00:00"), Ordering::SeqCst);
    r.stop_work().unwrap();
    let before = r.local_changes();
    let tz = TimeZone::UTC;
    let parse = |t: &str| taskfile::parse(t, &tz);

    assert_eq!(parse("# only comments\n\n").unwrap(), None);
    assert!(err(parse("work log:\n")).contains("no \"title"));
    assert!(err(parse("title: \n")).contains("line 1: empty title"));
    assert!(err(parse("title: a\nwork log:\nyesterday - today\n")).contains("line 3: expected"));
    assert!(err(parse("title: a\nwork log:\n2026-11-23 10:00 - 2026-11-23 09:00\n")).contains("ends before"));
    assert!(err(parse("title: a\n2026-11-23 10:00 - 2026-11-23 11:00\n")).contains("line 2"));

    // in the future
    let e = edit(&mut r, a, |t| t + "2026-11-23 10:30 - 2026-11-23 12:00\n");
    assert!(err(e).contains("future"));
    // a new running session
    let e = edit(&mut r, a, |t| t.replace("title: a", "title: b") + "2026-11-23 08:00 -\n");
    assert!(err(e).contains("only the running session"));
    assert_eq!(r.local_changes(), before);
    assert_eq!(r.task(a).unwrap().title, "a");
}

#[test]
fn other_users_sessions_are_labelled() {
    let (mut r, time) = setup();
    let tab = r.create_tab("work").unwrap();
    let a = r.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    r.set_user("bob").unwrap();
    r.start_work(a).unwrap();
    time.store(ms("2026-11-23T10:00:00"), Ordering::SeqCst);
    r.stop_work().unwrap();
    r.set_user("me").unwrap();
    let text = taskfile::render(&r, a, &TimeZone::UTC).unwrap();
    assert!(text.contains("2026-11-23 09:00:00 - 2026-11-23 10:00:00  # bob"), "{text}");
    assert_eq!(edit(&mut r, a, |t| t).unwrap(), Applied::default());
}
