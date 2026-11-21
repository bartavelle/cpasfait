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

/// An `ssh` that drops its options and the host name, then runs the command here.
fn fake_ssh(dir: &Path) -> String {
    let p = dir.join("fake-ssh");
    std::fs::write(
        &p,
        "#!/bin/sh\n\
         while [ $# -gt 0 ]; do case \"$1\" in -o|-p|-l) shift 2;; -*) shift;; *) break;; esac; done\n\
         shift\n\
         exec sh -c \"$*\"\n",
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
