//! rsync backend, in remote-shell mode: a fake `ssh` runs the remote side
//! locally, so the real rsync client/server protocol is exercised.
#![cfg(unix)]

use cpasfait_core::backend::RemoteBackend;
use cpasfait_core::backend::rsync::Rsync;
use cpasfait_core::crypto::Identity;
use cpasfait_core::sync::{remote_tabs, sync_tab};
use cpasfait_core::{Place, Repo};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn have_rsync() -> bool {
    std::process::Command::new("rsync").arg("--version").output().is_ok()
}

/// An `ssh` that drops its options and the host name, then runs the command
/// here (through `rrsync ROOT` when given, as a restricted key would).
fn fake_ssh(dir: &Path) -> String {
    fake_ssh_with(dir, None)
}

fn fake_ssh_with(dir: &Path, rrsync_root: Option<&Path>) -> String {
    let p = dir.join("fake-ssh");
    let run = match rrsync_root {
        Some(root) => format!("SSH_ORIGINAL_COMMAND=\"$*\" exec rrsync '{}'", root.display()),
        None => "exec sh -c \"$*\"".to_owned(),
    };
    std::fs::write(
        &p,
        format!(
            "#!/bin/sh\n\
             while [ $# -gt 0 ]; do case \"$1\" in -o|-p|-l) shift 2;; -*) shift;; *) break;; esac; done\n\
             shift\n\
             {run}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{} -o BatchMode=yes", p.display())
}

#[test]
fn two_devices_converge_over_rsync() {
    if !have_rsync() {
        eprintln!("rsync not installed: skipped");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(tmp.path());
    let target = format!("me@fakehost:{}", tmp.path().join("remote/shared").display());
    let b1 = Rsync::new(&target, tmp.path().join("m1")).with_ssh(&ssh);
    let b2 = Rsync::new(&target, tmp.path().join("m2")).with_ssh(&ssh);

    let (mut r1, mut r2) = (Repo::open_in_memory().unwrap(), Repo::open_in_memory().unwrap());
    let (k1, k2) = (Identity::generate(), Identity::generate());
    let tab = r1.create_tab("shared").unwrap();
    let a = r1.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    r1.share_tab(tab, k1.recipient()).unwrap();
    r1.set_member(tab, k2.recipient(), Some("bob")).unwrap();
    let s = sync_tab(&mut r1, tab, &b1, &k1).unwrap();
    assert_eq!((s.pushed, s.keys_sent), (6, 1));
    // on the "server"
    assert!(
        tmp.path()
            .join("remote/shared")
            .join(tab.to_string())
            .join("keys")
            .is_dir()
    );

    assert_eq!(remote_tabs(&b2).unwrap(), vec![tab]);
    r2.add_known_tab(tab).unwrap();
    assert_eq!(sync_tab(&mut r2, tab, &b2, &k2).unwrap().pulled, 6);

    r1.set_title(a, "one").unwrap();
    r2.add_task(tab, Place::LastChildOf(Some(a)), "child").unwrap();
    for _ in 0..2 {
        // past the pull-reuse window, as between two real syncs
        std::thread::sleep(std::time::Duration::from_millis(2100));
        sync_tab(&mut r1, tab, &b1, &k1).unwrap();
        sync_tab(&mut r2, tab, &b2, &k2).unwrap();
    }
    assert_eq!(r1.dump_model().unwrap(), r2.dump_model().unwrap());
    assert_eq!(r2.task(a).unwrap().title, "one");
}

#[test]
fn unreachable_host_is_an_error() {
    if !have_rsync() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let b = Rsync::new("host:path", tmp.path().join("m")).with_ssh("false");
    match b.list("t/ops") {
        Err(cpasfait_core::Error::Backend(m)) => assert!(m.contains("rsync failed"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(b.put_new("t/ops/d/1.seg", b"x").is_err());
    // the failed upload is not left in the mirror, so a retry uploads it
    assert!(!tmp.path().join("m/t/ops/d/1.seg").exists());
}

/// A key restricted with `command="rrsync /srv/cpasfait",restrict`: only
/// rsync runs, paths are relative to the rrsync root.
#[test]
fn rrsync_restricted_key() {
    if !have_rsync() || std::process::Command::new("rrsync").arg("-help").output().is_err() {
        eprintln!("rsync or rrsync not installed: skipped");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("srv");
    std::fs::create_dir(&root).unwrap();
    let ssh = fake_ssh_with(tmp.path(), Some(&root));
    for (spec, dir) in [("me@h:", root.clone()), ("me@h:team", root.join("team"))] {
        let m = tempfile::tempdir().unwrap();
        let b = Rsync::new(spec, m.path().join("m1")).with_ssh(&ssh);
        assert_eq!(b.list("t/ops").unwrap(), Vec::<String>::new(), "{spec}");
        b.put_new("t/ops/d/1.seg", b"x").unwrap();
        assert_eq!(std::fs::read(dir.join("t/ops/d/1.seg")).unwrap(), b"x");
        let b2 = Rsync::new(spec, m.path().join("m2")).with_ssh(&ssh);
        assert_eq!(b2.list("t/ops").unwrap(), vec!["t/ops/d/1.seg"]);
        assert_eq!(b2.list("").unwrap(), vec!["t/ops/d/1.seg"]);
    }
    // Missing parents cannot be created through rrsync: a clear error.
    let m = tempfile::tempdir().unwrap();
    let b = Rsync::new("me@h:a/b", m.path()).with_ssh(&ssh);
    match b.put_new("t/x", b"x") {
        Err(cpasfait_core::Error::Backend(e)) => assert!(e.contains("No such file or directory"), "{e}"),
        other => panic!("{other:?}"),
    }
}

/// Many files at once: one rsync run (ssh connection) per batch of 100.
#[test]
fn upload_many_files_in_batches() {
    if !have_rsync() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(tmp.path());
    // Count the connections.
    let counting = tmp.path().join("counting-ssh");
    let log = tmp.path().join("calls");
    std::fs::write(
        &counting,
        format!("#!/bin/sh\necho >> '{}'\nexec {ssh} \"$@\"\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&counting, std::fs::Permissions::from_mode(0o755)).unwrap();
    let calls = || std::fs::read_to_string(&log).unwrap_or_default().lines().count();
    // The remote directory's parents do not exist yet.
    let remote = tmp.path().join("remote/deep/shared");
    let b = Rsync::new(&format!("me@h:{}", remote.display()), tmp.path().join("m"))
        .with_ssh(&counting.display().to_string());

    let files: Vec<(String, Vec<u8>)> = (0..150)
        .map(|i| (format!("t/ops/d{}/{i}.seg", i % 3), format!("data {i}").into_bytes()))
        .collect();
    let mut seen = vec![];
    assert_eq!(b.put_new_many(&files, &mut |n| seen.push(n)).unwrap(), 150);
    assert_eq!(seen, vec![100, 150]);
    // Two batches; the first one also created the missing parents.
    assert_eq!(calls(), 3);
    for (p, data) in &files {
        assert_eq!(&std::fs::read(remote.join(p)).unwrap(), data);
    }
    // Again: nothing new, nothing overwritten.
    let changed: Vec<_> = files.iter().map(|(p, _)| (p.clone(), b"other".to_vec())).collect();
    assert_eq!(b.put_new_many(&changed, &mut |_| {}).unwrap(), 0);
    assert_eq!(std::fs::read(remote.join("t/ops/d0/0.seg")).unwrap(), b"data 0");
}
