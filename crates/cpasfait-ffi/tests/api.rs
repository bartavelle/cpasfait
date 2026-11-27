//! The app's view of the core. One test: the data dir is per process.

use cpasfait_core::backend::RemoteBackend;
use cpasfait_core::backend::localdir::LocalDir;
use cpasfait_core::crypto::Identity;
use cpasfait_core::sync::sync_tab;
use cpasfait_core::{Place, Repo};
use cpasfait_ffi::{Cpasfait, CpasfaitError, PlaceArg, Remote, RemoteError, RemoteProvider};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What the app would implement in Kotlin (SFTP...): here, a directory.
struct DirRemote(LocalDir);

impl Remote for DirRemote {
    fn list(&self, prefix: String) -> Result<Vec<String>, RemoteError> {
        self.0
            .list(&prefix)
            .map_err(|e| RemoteError::Failed { reason: e.to_string() })
    }
    fn get(&self, path: String) -> Result<Vec<u8>, RemoteError> {
        self.0
            .get(&path)
            .map_err(|e| RemoteError::Failed { reason: e.to_string() })
    }
    fn put_new(&self, path: String, data: Vec<u8>) -> Result<(), RemoteError> {
        match self.0.put_new(&path, &data) {
            Ok(()) => Ok(()),
            Err(cpasfait_core::Error::AlreadyExists(_)) => Err(RemoteError::AlreadyExists),
            Err(e) => Err(RemoteError::Failed { reason: e.to_string() }),
        }
    }
}

/// Serves `rsync:` specs from a directory, counting the opens.
struct Provider {
    dir: PathBuf,
    opened: AtomicUsize,
    specs: Mutex<Vec<String>>,
}

impl RemoteProvider for Provider {
    fn open(&self, spec: String) -> Result<Option<Arc<dyn Remote>>, RemoteError> {
        self.specs.lock().unwrap().push(spec.clone());
        if !spec.starts_with("rsync:") {
            return Ok(None);
        }
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::new(DirRemote(LocalDir::new(&self.dir)))))
    }
}

fn place_last(parent: Option<&str>) -> PlaceArg {
    PlaceArg::LastChildOf {
        parent: parent.map(str::to_owned),
    }
}

#[test]
fn app_api() {
    let tmp = tempfile::tempdir().unwrap();
    let app = Cpasfait::open(
        tmp.path().join("data").display().to_string(),
        Some("Europe/Paris".into()),
    )
    .unwrap();
    assert!(Cpasfait::open(tmp.path().join("other").display().to_string(), None).is_err());
    // Another connection to the same database (the sync one).
    let bg = Cpasfait::open(tmp.path().join("data").display().to_string(), None).unwrap();

    let tabs = app.tabs().unwrap();
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].name, "inbox");
    let tab = tabs[0].id.clone();
    assert!(tmp.path().join("data/cpasfait.db").exists());

    // Tasks.
    let paint = app.add_task(tab.clone(), place_last(None), "Paint".into()).unwrap();
    let walls = app
        .add_task(tab.clone(), place_last(Some(&paint)), "Walls".into())
        .unwrap();
    let report = app.add_task(tab.clone(), place_last(None), "Report".into()).unwrap();
    app.set_priority(report.clone(), 3).unwrap();
    assert_eq!(
        app.set_due(paint.clone(), Some("2030-01-02 14:30".into())).unwrap(),
        Some("2030-01-02 14:30".into())
    );
    assert!(matches!(
        app.set_due(paint.clone(), Some("someday".into())),
        Err(CpasfaitError::Failed { .. })
    ));
    app.set_tag(walls.clone(), "home".into(), true).unwrap();
    app.set_blocked_by(paint.clone(), report.clone(), true).unwrap();
    let rows = app.rows(tab.clone(), false).unwrap();
    let lines: Vec<_> = rows.iter().map(|r| (r.title.as_str(), r.depth)).collect();
    assert_eq!(lines, vec![("Report", 0), ("Paint", 0), ("Walls", 1)]);
    let p = &rows[1];
    assert!(p.blocked && p.has_children);
    assert_eq!(p.due.as_deref(), Some("2030-01-02 14:30"));
    assert_eq!(p.due_ms, Some(1_893_591_000_000)); // 13:30 UTC
    assert_eq!(rows[2].tags, vec!["home"]);
    assert!(rows[0].score > p.score);

    app.set_done(walls.clone(), true).unwrap();
    assert_eq!(app.rows(tab.clone(), true).unwrap().len(), 2);

    // Description and notes.
    app.set_description(paint.clone(), "two coats".into()).unwrap();
    let n1 = app.add_note(paint.clone(), "bought the paint".into()).unwrap();
    app.add_note(paint.clone(), "first coat done".into()).unwrap();
    let d = app.task(paint.clone()).unwrap();
    assert_eq!(d.description, "two coats");
    assert_eq!(d.notes.len(), 2);
    assert!(d.notes.iter().all(|n| n.mine && n.author == app.user()));
    let row = |app: &Cpasfait| {
        app.rows(tab.clone(), false)
            .unwrap()
            .into_iter()
            .find(|r| r.id == paint)
            .unwrap()
    };
    assert!(row(&app).has_description);
    assert_eq!(row(&app).note_count, 2);
    app.set_note_text(n1.clone(), "bought the paint (white)".into())
        .unwrap();
    app.delete_note(n1).unwrap();
    assert_eq!(app.task(paint.clone()).unwrap().notes[0].text, "first coat done");
    assert!(app.add_note(paint.clone(), " ".into()).is_err());

    // Work log.
    let version = bg.data_version().unwrap();
    let session = app.start_work(report.clone()).unwrap();
    assert_ne!(
        bg.data_version().unwrap(),
        version,
        "the other connection sees the change"
    );
    let active = app.active_session().unwrap().unwrap();
    assert_eq!(
        (active.id.as_str(), active.mine, active.end),
        (session.as_str(), true, None)
    );
    assert_eq!(app.rows(tab.clone(), false).unwrap()[0].working, vec![app.user()]);
    app.stop_work().unwrap();
    assert!(app.active_session().unwrap().is_none());
    let d = app.task(report.clone()).unwrap();
    assert_eq!(d.sessions.len(), 1);
    assert!(d.sessions[0].end.is_some());
    assert_eq!(d.blocks.len(), 1);
    assert_eq!(
        app.task(paint.clone()).unwrap().requires[0].title.as_deref(),
        Some("Report")
    );

    // Due dates.
    assert_eq!(app.upcoming_dues().unwrap().len(), 1);
    assert!(app.take_due_notices().unwrap().is_empty());

    // Search, next task.
    let hits = app.search("kitchen".into(), 10).unwrap();
    assert!(hits.is_empty());
    let hits = app.search("PAINT".into(), 10).unwrap();
    assert_eq!(
        hits.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(),
        vec!["Paint", "Walls"]
    );
    assert_eq!(hits[1].path, "Paint");
    assert!(hits[1].done);
    assert_eq!(app.search("walls paint".into(), 10).unwrap().len(), 1);
    assert_eq!(app.next_task(tab.clone()).unwrap(), Some(report.clone()));

    // Work log by hand, time report.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let hour = 3_600_000;
    let logged = app.add_session(walls.clone(), now - 3 * hour, now - 2 * hour).unwrap();
    assert!(app.add_session(walls.clone(), now, now + hour).is_err());
    app.set_session_span(logged.clone(), now - 4 * hour, Some(now - 2 * hour))
        .unwrap();
    app.set_session_note(logged.clone(), "first coat".into()).unwrap();
    let report_tabs = app.time_report(now - 24 * hour, now, None).unwrap();
    assert_eq!(report_tabs.len(), 1);
    let paint_line = report_tabs[0].lines.iter().find(|l| l.title == "Paint").unwrap();
    assert_eq!((paint_line.own_ms, paint_line.total_ms), (0, 2 * hour));
    assert!(app.time_report(now - 24 * hour, now, Some(tab.clone())).unwrap()[0].total_ms >= 2 * hour);
    app.delete_session(logged).unwrap();
    assert!(app.task(walls.clone()).unwrap().sessions.is_empty());

    // Tab settings.
    assert_eq!(app.default_priority(tab.clone()).unwrap(), 0);
    app.set_default_priority(tab.clone(), Some(1)).unwrap();
    assert_eq!(app.default_priority(tab.clone()).unwrap(), 1);
    app.set_default_priority(tab.clone(), None).unwrap();
    assert_eq!(app.default_priority(tab.clone()).unwrap(), 0);

    // Sharing through a remote the app provides.
    let remote_dir = tmp.path().join("frolic");
    let provider = Arc::new(Provider {
        dir: remote_dir.clone(),
        opened: AtomicUsize::new(0),
        specs: Mutex::new(vec![]),
    });
    let remotes: Arc<dyn RemoteProvider> = provider.clone();
    app.set_remote(tab.clone(), Some("rsync:me@frolic:perso".into()))
        .unwrap();
    let key = app.public_key().unwrap();
    assert!(key.starts_with("age1"));
    assert!(tmp.path().join("data/identity.txt").exists());
    let s = bg.sync(Some(remotes.clone())).unwrap();
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    assert!(s.pushed > 0);
    assert_eq!(provider.opened.load(Ordering::SeqCst), 1);
    assert!(remote_dir.join(&tab).join("keys").is_dir());

    // Another device (desktop, with the core) is invited, syncs and edits.
    let other_key = Identity::generate();
    app.invite(tab.clone(), other_key.recipient().into(), Some("laptop".into()))
        .unwrap();
    assert!(app.invite(tab.clone(), other_key.recipient().into(), None).is_err());
    assert_eq!(app.members(tab.clone()).unwrap().len(), 2);
    assert!(app.revoke(tab.clone(), "age1nobody".into()).is_err());
    assert!(app.revoke(tab.clone(), key.clone()).is_ok(), "not the last member");
    app.invite(tab.clone(), key.clone(), Some("me".into())).unwrap();
    bg.sync(Some(remotes.clone())).unwrap();
    let mut laptop = Repo::open_in_memory().unwrap();
    let tab_id = tab.parse().unwrap();
    laptop.add_known_tab(tab_id).unwrap();
    let dir = LocalDir::new(&remote_dir);
    sync_tab(&mut laptop, tab_id, &dir, &other_key).unwrap();
    assert_eq!(laptop.tree(tab_id).unwrap().len(), 3);
    laptop
        .add_task(tab_id, Place::LastChildOf(None), "From the laptop")
        .unwrap();
    // The laptop shares a second tab with the phone.
    let second = laptop.create_tab("work").unwrap();
    laptop.share_tab(second, other_key.recipient()).unwrap();
    laptop.set_member(second, &key, Some("phone")).unwrap();
    sync_tab(&mut laptop, tab_id, &dir, &other_key).unwrap();
    sync_tab(&mut laptop, second, &dir, &other_key).unwrap();

    let s = bg.sync(Some(remotes.clone())).unwrap();
    assert!(s.pulled > 0 && s.errors.is_empty(), "{s:?}");
    assert!(
        app.rows(tab.clone(), false)
            .unwrap()
            .iter()
            .any(|r| r.title == "From the laptop")
    );

    let joined = app
        .join_tab("rsync:me@frolic:perso".into(), None, Some(remotes.clone()))
        .unwrap();
    assert_eq!(joined.tab, second.to_string());
    assert_eq!(joined.waiting_for_invitation, None);
    assert_eq!(
        app.tabs().unwrap().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        vec!["inbox", "work"]
    );
    assert!(
        app.join_tab("rsync:me@frolic:perso".into(), None, Some(remotes.clone()))
            .is_err(),
        "nothing new"
    );

    // Moving the tab to another remote.
    let elsewhere = tmp.path().join("elsewhere");
    assert_eq!(
        bg.move_tab(tab.clone(), "rsync:me@frolic:perso".into(), Some(remotes.clone()))
            .map_err(|e| e.to_string()),
        Err("already there".into())
    );
    assert!(
        bg.move_tab(tab.clone(), elsewhere.display().to_string(), Some(remotes.clone()))
            .unwrap()
            > 0
    );
    assert_eq!(app.tabs().unwrap()[0].remote, Some(elsewhere.display().to_string()));

    // Without a provider: directories (here) work; rsync: would run rsync.
    app.set_remote(second.to_string(), None).unwrap();
    let plain = tmp.path().join("plain");
    app.set_remote(tab.clone(), Some(plain.display().to_string())).unwrap();
    assert!(bg.sync(None).unwrap().errors.is_empty());
    assert!(plain.join(&tab).is_dir());
}
