use cpasfait_core::backend::RemoteBackend;
use cpasfait_core::backend::localdir::LocalDir;
use cpasfait_core::crypto::{self, Identity};
use cpasfait_core::sync::{invited_to, remote_tabs, sync_tab};
use cpasfait_core::{Error, Place, Repo, TabId};
use std::path::Path;

/// A device: its database and its user's identity.
struct Dev {
    repo: Repo,
    id: Identity,
}

impl Dev {
    fn new(user: &str) -> Dev {
        let mut repo = Repo::open_in_memory().unwrap();
        repo.set_user(user).unwrap();
        Dev {
            repo,
            id: Identity::generate(),
        }
    }
    /// Another device of the same user.
    fn sibling(&self, key_file: &Path) -> Dev {
        Dev {
            repo: Repo::open_in_memory().unwrap(),
            id: Identity::load(key_file).unwrap(),
        }
    }
    fn sync(&mut self, tab: TabId, b: &dyn RemoteBackend) -> cpasfait_core::Result<cpasfait_core::sync::SyncStats> {
        sync_tab(&mut self.repo, tab, b, &self.id)
    }
    fn invite(&mut self, tab: TabId, other: &Dev) {
        let name = other.repo.user().to_owned();
        self.repo.set_member(tab, other.id.recipient(), Some(&name)).unwrap();
    }
    fn title(&self, task: cpasfait_core::TaskId) -> Option<String> {
        self.repo.task(task).ok().map(|t| t.title)
    }
}

/// A new tab on `d`, shared through `b`.
fn shared_tab(d: &mut Dev, b: &dyn RemoteBackend) -> TabId {
    let tab = d.repo.create_tab("shared").unwrap();
    d.repo.share_tab(tab, d.id.recipient()).unwrap();
    d.sync(tab, b).unwrap();
    tab
}

fn all_files(dir: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(all_files(&p));
        } else {
            out.push((p.clone(), std::fs::read(&p).unwrap()));
        }
    }
    out
}

#[test]
fn two_devices_converge_through_a_directory() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut d1, mut d2) = (Dev::new("alice"), Dev::new("bob"));

    let tab = d1.repo.create_tab("shared").unwrap();
    let a = d1.repo.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    d1.repo.share_tab(tab, d1.id.recipient()).unwrap();
    d1.invite(tab, &d2);
    let s = d1.sync(tab, &backend).unwrap();
    assert_eq!((s.pulled, s.pushed, s.keys_sent), (0, 6, 1));

    // d2 joins
    assert_eq!(remote_tabs(&backend).unwrap(), vec![tab]);
    assert!(invited_to(&backend, tab, &d2.id).unwrap());
    assert!(!invited_to(&backend, tab, &Identity::generate()).unwrap());
    d2.repo.add_known_tab(tab).unwrap();
    let s = d2.sync(tab, &backend).unwrap();
    assert_eq!((s.pulled, s.pushed, s.keys_sent), (6, 0, 0));
    assert_eq!(d2.repo.tabs().unwrap()[0].name, "shared");
    let members = d2.repo.members(tab).unwrap();
    assert_eq!(
        members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        ["alice", "bob"]
    );

    // concurrent edits
    d1.repo.set_title(a, "from d1").unwrap();
    d2.repo.set_title(a, "from d2").unwrap();
    d2.repo.add_task(tab, Place::LastChildOf(Some(a)), "child").unwrap();
    d1.sync(tab, &backend).unwrap();
    d2.sync(tab, &backend).unwrap();
    d1.sync(tab, &backend).unwrap();
    assert_eq!(d1.repo.dump_model().unwrap(), d2.repo.dump_model().unwrap());
    assert_eq!(d1.title(a).unwrap(), "from d2"); // later HLC wins

    // nothing left to do
    let s = d1.sync(tab, &backend).unwrap();
    assert_eq!((s.pulled, s.pushed, s.keys_sent), (0, 0, 0));

    // nothing readable on the remote
    for (path, data) in all_files(remote.path()) {
        for secret in ["from d", "child", "shared", "alice", "bob", d2.id.recipient()] {
            assert!(
                !data.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{secret:?} in {path:?}"
            );
        }
    }
}

#[test]
fn uninvited_devices_cannot_read() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut d1, mut d2) = (Dev::new("alice"), Dev::new("bob"));
    let tab = shared_tab(&mut d1, &backend);
    let a = d1.repo.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    d1.sync(tab, &backend).unwrap();

    d2.repo.add_known_tab(tab).unwrap();
    match d2.sync(tab, &backend) {
        Err(Error::NotInvited(key)) => assert_eq!(key, d2.id.recipient()),
        other => panic!("{other:?}"),
    }
    assert_eq!(d2.title(a), None);
    // Ops made before being invited are pushed once invited.
    let b = d2.repo.add_task(tab, Place::LastChildOf(None), "b").unwrap();
    assert!(d2.sync(tab, &backend).is_err());

    d1.invite(tab, &d2);
    assert_eq!(d1.sync(tab, &backend).unwrap().keys_sent, 1);
    let s = d2.sync(tab, &backend).unwrap();
    assert_eq!((s.pushed, d2.title(a).as_deref()), (3, Some("a")));
    d1.sync(tab, &backend).unwrap();
    assert_eq!(d1.title(b).as_deref(), Some("b"));
}

#[test]
fn another_device_of_the_same_user() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let keys = tempfile::tempdir().unwrap();
    let key_file = keys.path().join("keys.txt");
    let mut laptop = Dev::new("alice");
    laptop.id = Identity::create(&key_file).unwrap();
    let tab = shared_tab(&mut laptop, &backend);
    let mut phone = laptop.sibling(&key_file);
    phone.repo.add_known_tab(tab).unwrap();
    assert_eq!(phone.sync(tab, &backend).unwrap().pulled, 2);
}

#[test]
fn removed_members_cannot_read_new_data() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut a, mut b, mut c) = (Dev::new("alice"), Dev::new("bob"), Dev::new("carol"));
    let tab = shared_tab(&mut a, &backend);
    a.invite(tab, &b);
    a.invite(tab, &c);
    let t = a.repo.add_task(tab, Place::LastChildOf(None), "old").unwrap();
    a.sync(tab, &backend).unwrap();
    for d in [&mut b, &mut c] {
        d.repo.add_known_tab(tab).unwrap();
        d.sync(tab, &backend).unwrap();
        assert_eq!(d.title(t).as_deref(), Some("old"));
    }

    // Bob removes Carol: the next sync starts a new key.
    b.repo.set_member(tab, c.id.recipient(), None).unwrap();
    b.repo.set_title(t, "new").unwrap();
    let s = b.sync(tab, &backend).unwrap();
    assert_eq!(s.keys_sent, 1);
    assert_eq!(b.repo.epochs(tab).unwrap().len(), 2);

    a.sync(tab, &backend).unwrap();
    assert_eq!(a.title(t).as_deref(), Some("new"));
    let s = c.sync(tab, &backend).unwrap();
    assert_eq!(s.waiting_for_key, 1);
    assert_eq!(c.title(t).as_deref(), Some("old"));
    // Carol's own new data is still accepted: the remote's access control
    // (Drive sharing) is what keeps former members out.
    assert_eq!(a.repo.epochs(tab).unwrap().len(), 2);
    assert_eq!(c.repo.epochs(tab).unwrap().len(), 1);

    // A newcomer gets every key, and the whole history.
    let mut d = Dev::new("dave");
    a.invite(tab, &d);
    assert_eq!(a.sync(tab, &backend).unwrap().keys_sent, 2);
    d.repo.add_known_tab(tab).unwrap();
    let s = d.sync(tab, &backend).unwrap();
    assert_eq!(s.waiting_for_key, 0);
    assert_eq!(d.title(t).as_deref(), Some("new"));
    assert_eq!(d.repo.epochs(tab).unwrap().len(), 2);

    // Carol re-invited: she gets the new key too.
    a.invite(tab, &c);
    a.sync(tab, &backend).unwrap();
    let s = c.sync(tab, &backend).unwrap();
    assert_eq!(s.waiting_for_key, 0);
    assert_eq!(c.title(t).as_deref(), Some("new"));
}

#[test]
fn stale_members_list_does_not_leak_new_keys() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut a, mut b, mut c) = (Dev::new("alice"), Dev::new("bob"), Dev::new("carol"));
    let tab = shared_tab(&mut a, &backend);
    a.invite(tab, &b);
    a.invite(tab, &c);
    a.sync(tab, &backend).unwrap();
    b.repo.add_known_tab(tab).unwrap();
    b.sync(tab, &backend).unwrap();
    c.repo.add_known_tab(tab).unwrap();
    c.sync(tab, &backend).unwrap();

    // Bob removes Carol; Alice sees the new key file but not yet the removal.
    b.repo.set_member(tab, c.id.recipient(), None).unwrap();
    b.sync(tab, &backend).unwrap();
    let bob_ops = remote
        .path()
        .join(tab.to_string())
        .join("ops")
        .join(b.repo.device().to_string());
    let hidden = remote.path().join("hidden");
    std::fs::rename(&bob_ops, &hidden).unwrap();
    a.sync(tab, &backend).unwrap();
    assert!(a.repo.members(tab).unwrap().iter().all(|m| m.active), "alice is stale");
    std::fs::rename(&hidden, &bob_ops).unwrap();
    let t = b.repo.add_task(tab, Place::LastChildOf(None), "secret").unwrap();
    b.sync(tab, &backend).unwrap();
    c.sync(tab, &backend).unwrap();
    assert_eq!(c.title(t), None);
    assert_eq!(c.repo.epochs(tab).unwrap().len(), 1);
}

#[test]
fn forged_keys_are_ignored() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut a, mut b) = (Dev::new("alice"), Dev::new("bob"));
    let tab = shared_tab(&mut a, &backend);
    a.invite(tab, &b);
    a.sync(tab, &backend).unwrap();
    b.repo.add_known_tab(tab).unwrap();
    b.sync(tab, &backend).unwrap();
    let real = a.repo.write_epoch(tab).unwrap().unwrap();

    // The storage provider knows the members' public keys and makes keys of
    // its own: a second root, and a "rotation" with a bad proof.
    let forge = |epoch: uuid::Uuid, prev: Option<(uuid::Uuid, Vec<u8>)>| {
        let key = crypto::random_key();
        let payload = serde_json::json!({
            "v": 1, "tab": tab, "epoch": epoch, "key": crypto::hex(&key),
            "prev": prev.map(|(e, mac)| serde_json::json!({"epoch": e, "mac": crypto::hex(&mac)})),
            "recipients": [a.id.recipient(), b.id.recipient()], "exclude": [],
        });
        let data = crypto::wrap(
            &[a.id.recipient().into(), b.id.recipient().into()],
            payload.to_string().as_bytes(),
        )
        .unwrap();
        backend
            .put_new(&format!("{tab}/keys/{epoch}/evil-{}.age", uuid::Uuid::new_v4()), &data)
            .unwrap();
    };
    forge(uuid::Uuid::now_v7(), None);
    forge(uuid::Uuid::now_v7(), Some((real.id, vec![0; 32])));
    // ... and a fake key for the real epoch.
    forge(real.id, None);
    for d in [&mut a, &mut b] {
        d.sync(tab, &backend).unwrap();
        assert_eq!(d.repo.epochs(tab).unwrap(), vec![real.clone()]);
    }
    // A newcomer would see two roots: refused rather than guessed.
    let mut c = Dev::new("carol");
    let other_root = uuid::Uuid::now_v7();
    let key = crypto::random_key();
    let payload = serde_json::json!({
        "v": 1, "tab": tab, "epoch": other_root, "key": crypto::hex(&key), "prev": null,
        "recipients": [c.id.recipient()], "exclude": [],
    });
    let data = crypto::wrap(&[c.id.recipient().into()], payload.to_string().as_bytes()).unwrap();
    backend
        .put_new(&format!("{tab}/keys/{other_root}/evil.age"), &data)
        .unwrap();
    a.invite(tab, &c);
    a.sync(tab, &backend).unwrap();
    c.repo.add_known_tab(tab).unwrap();
    assert!(matches!(c.sync(tab, &backend), Err(Error::Crypto(_))));
}

#[test]
fn tampered_segments_are_rejected() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut a, mut b) = (Dev::new("alice"), Dev::new("bob"));
    let tab = shared_tab(&mut a, &backend);
    a.invite(tab, &b);
    a.repo.add_task(tab, Place::LastChildOf(None), "x").unwrap();
    a.sync(tab, &backend).unwrap();
    let dir = remote
        .path()
        .join(tab.to_string())
        .join("ops")
        .join(a.repo.device().to_string());
    let seg = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .max()
        .unwrap();
    let mut data = std::fs::read(&seg).unwrap();
    let n = data.len();
    data[n - 20] ^= 1;
    std::fs::write(&seg, data).unwrap();
    b.repo.add_known_tab(tab).unwrap();
    assert!(matches!(b.sync(tab, &backend), Err(Error::Crypto(_))));
}

#[test]
fn missing_segment_blocks_cursor() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut d1, mut d2) = (Dev::new("alice"), Dev::new("bob"));
    let tab = shared_tab(&mut d1, &backend); // segment 1-2
    d1.invite(tab, &d2);
    d1.repo.add_task(tab, Place::LastChildOf(None), "x").unwrap();
    d1.sync(tab, &backend).unwrap(); // segment 3-5
    // simulate the first segment not having arrived yet
    let dir = remote
        .path()
        .join(tab.to_string())
        .join("ops")
        .join(d1.repo.device().to_string());
    let first = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .min()
        .unwrap();
    let hidden = dir.join(".hidden");
    std::fs::rename(&first, &hidden).unwrap();
    d2.repo.add_known_tab(tab).unwrap();
    assert_eq!(d2.sync(tab, &backend).unwrap().pulled, 0);
    std::fs::rename(&hidden, &first).unwrap();
    assert_eq!(d2.sync(tab, &backend).unwrap().pulled, 6);
    assert_eq!(d1.repo.dump_model().unwrap(), d2.repo.dump_model().unwrap());
}

#[test]
fn ops_in_other_tabs_do_not_create_gaps() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut d1, mut d2) = (Dev::new("alice"), Dev::new("bob"));
    let tab = shared_tab(&mut d1, &backend);
    d1.invite(tab, &d2);
    d1.sync(tab, &backend).unwrap();
    d2.repo.create_tab("private").unwrap(); // d2's first op is in another tab
    d2.repo.add_known_tab(tab).unwrap();
    d2.sync(tab, &backend).unwrap();
    d2.repo.add_task(tab, Place::LastChildOf(None), "from d2").unwrap();
    d2.sync(tab, &backend).unwrap();
    assert_eq!(d1.sync(tab, &backend).unwrap().pulled, 3);
    assert_eq!(d1.repo.tree(tab).unwrap().len(), 1);
}

fn local(p: &Path) -> cpasfait_core::RemoteConfig {
    cpasfait_core::RemoteConfig::LocalDir {
        path: p.to_str().unwrap().to_owned(),
    }
}

/// Alice and Bob share a tab through `old`; Bob has synced.
fn pair(old: &dyn RemoteBackend, old_path: &Path) -> (Dev, Dev, TabId) {
    let (mut a, mut b) = (Dev::new("alice"), Dev::new("bob"));
    let tab = a.repo.create_tab("shared").unwrap();
    a.repo.share_tab(tab, a.id.recipient()).unwrap();
    a.repo.set_remote(tab, Some(&local(old_path))).unwrap();
    a.invite(tab, &b);
    a.sync(tab, old).unwrap();
    b.repo.add_known_tab(tab).unwrap();
    b.repo.set_remote(tab, Some(&local(old_path))).unwrap();
    b.sync(tab, old).unwrap();
    (a, b, tab)
}

#[test]
fn moving_to_another_remote() {
    use cpasfait_core::sync::{accept_move, move_tab};
    let dir = tempfile::tempdir().unwrap();
    let (p1, p2) = (dir.path().join("one"), dir.path().join("two"));
    let (old, new) = (LocalDir::new(&p1), LocalDir::new(&p2));
    let (mut a, mut b, tab) = pair(&old, &p1);
    let mut c = Dev::new("carol");
    a.invite(tab, &c);
    let t = a.repo.add_task(tab, Place::LastChildOf(None), "before").unwrap();
    a.sync(tab, &old).unwrap();
    c.repo.add_known_tab(tab).unwrap();
    c.sync(tab, &old).unwrap();

    let copied = move_tab(&mut a.repo, tab, &old, &new, &local(&p2), &a.id, &mut |_| {}).unwrap();
    assert!(copied >= 4, "{copied}");
    let tabs = a.repo.tabs().unwrap();
    assert_eq!(tabs[0].remote, Some(local(&p2)));
    assert_eq!(tabs[0].pending_move, None, "the mover follows its own move");

    // Bob, not having seen it yet, writes to the old remote after the copy.
    let late = b.repo.add_task(tab, Place::LastChildOf(None), "late").unwrap();
    b.sync(tab, &old).unwrap();
    let m = b.repo.tabs().unwrap()[0].pending_move.clone().unwrap();
    assert_eq!((m.by.as_str(), m.remote.as_str()), ("alice", p2.to_str().unwrap()));
    // He follows it as is; his late change comes along.
    let r = accept_move(&mut b.repo, tab, Some(&old), &new, &local(&p2), &b.id, &mut |_| {}).unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(r.copied >= 1);
    assert_eq!(b.repo.tabs().unwrap()[0].remote, Some(local(&p2)));
    assert_eq!(b.repo.tabs().unwrap()[0].pending_move, None);

    // Carol sees the same place under another path.
    let link = dir.path().join("two-elsewhere");
    std::os::unix::fs::symlink(&p2, &link).unwrap();
    c.sync(tab, &old).unwrap();
    let elsewhere = LocalDir::new(&link);
    accept_move(&mut c.repo, tab, Some(&old), &elsewhere, &local(&link), &c.id, &mut |_| {}).unwrap();
    c.repo.set_title(t, "after").unwrap();
    c.sync(tab, &elsewhere).unwrap();

    a.sync(tab, &new).unwrap();
    b.sync(tab, &new).unwrap();
    assert_eq!(a.title(late).as_deref(), Some("late"));
    assert_eq!(b.title(t).as_deref(), Some("after"));
    assert_eq!(a.repo.dump_model().unwrap(), b.repo.dump_model().unwrap());
    assert_eq!(a.repo.dump_model().unwrap(), c.repo.dump_model().unwrap());
}

#[test]
fn following_a_move_to_a_wrong_path_is_refused() {
    use cpasfait_core::sync::{accept_move, move_tab};
    let dir = tempfile::tempdir().unwrap();
    let (p1, p2, typo) = (dir.path().join("one"), dir.path().join("two"), dir.path().join("tow"));
    let (old, new) = (LocalDir::new(&p1), LocalDir::new(&p2));
    let (mut a, mut b, tab) = pair(&old, &p1);
    move_tab(&mut a.repo, tab, &old, &new, &local(&p2), &a.id, &mut |_| {}).unwrap();
    b.sync(tab, &old).unwrap();
    let wrong = LocalDir::new(&typo);
    assert!(accept_move(&mut b.repo, tab, Some(&old), &wrong, &local(&typo), &b.id, &mut |_| {}).is_err());
    assert!(!typo.exists(), "nothing written there");
    let t = &b.repo.tabs().unwrap()[0];
    assert_eq!(t.remote, Some(local(&p1)));
    assert!(t.pending_move.is_some());
    // no move pending: nothing to accept
    assert!(accept_move(&mut a.repo, tab, None, &new, &local(&p2), &a.id, &mut |_| {}).is_err());
}

#[test]
fn old_remote_lost_after_a_move() {
    use cpasfait_core::sync::{accept_move, move_tab};
    let dir = tempfile::tempdir().unwrap();
    let (p1, p2) = (dir.path().join("one"), dir.path().join("two"));
    let (old, new) = (LocalDir::new(&p1), LocalDir::new(&p2));
    let (mut a, mut b, tab) = pair(&old, &p1);
    move_tab(&mut a.repo, tab, &old, &new, &local(&p2), &a.id, &mut |_| {}).unwrap();
    let late = b.repo.add_task(tab, Place::LastChildOf(None), "late").unwrap();
    b.sync(tab, &old).unwrap();
    std::fs::remove_dir_all(&p1).unwrap();
    // Bob's late ops never reached the new remote by copy: he uploads them again.
    let r = accept_move(&mut b.repo, tab, Some(&old), &new, &local(&p2), &b.id, &mut |_| {}).unwrap();
    assert_eq!(r.copied, 0);
    a.sync(tab, &new).unwrap();
    assert_eq!(a.title(late).as_deref(), Some("late"));
}

#[test]
fn lost_segments_are_uploaded_again() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let (mut a, mut b, tab) = pair(&backend, remote.path());
    let t = a.repo.add_task(tab, Place::LastChildOf(None), "x").unwrap();
    a.sync(tab, &backend).unwrap();
    // Alice's first segment disappears from the remote.
    let dir = remote
        .path()
        .join(tab.to_string())
        .join("ops")
        .join(a.repo.device().to_string());
    let first = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .min()
        .unwrap();
    std::fs::remove_file(first).unwrap();
    let mut c = Dev::new("carol");
    a.invite(tab, &c);
    assert!(a.sync(tab, &backend).unwrap().republished > 0);
    c.repo.add_known_tab(tab).unwrap();
    c.sync(tab, &backend).unwrap();
    assert_eq!(c.title(t).as_deref(), Some("x"));
    // overlapping segments are harmless for those who had them
    assert_eq!(b.sync(tab, &backend).unwrap().pulled, 4);
    assert_eq!(b.repo.dump_model().unwrap(), c.repo.dump_model().unwrap());
}
