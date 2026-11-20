use cpasfait_core::backend::localdir::LocalDir;
use cpasfait_core::sync::{remote_tabs, sync_tab};
use cpasfait_core::{Place, Repo};

#[test]
fn two_devices_converge_through_a_directory() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let mut r1 = Repo::open_in_memory().unwrap();
    let mut r2 = Repo::open_in_memory().unwrap();

    let tab = r1.create_tab("shared").unwrap();
    let a = r1.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    let s = sync_tab(&mut r1, tab, &backend).unwrap();
    assert_eq!((s.pulled, s.pushed), (0, 3));

    // r2 joins
    assert_eq!(remote_tabs(&backend).unwrap(), vec![tab]);
    r2.add_known_tab(tab).unwrap();
    let s = sync_tab(&mut r2, tab, &backend).unwrap();
    assert_eq!((s.pulled, s.pushed), (3, 0));
    assert_eq!(r2.tabs().unwrap()[0].name, "shared");

    // concurrent edits
    r1.set_title(a, "from r1").unwrap();
    r2.set_title(a, "from r2").unwrap();
    r2.add_task(tab, Place::LastChildOf(Some(a)), "child").unwrap();
    sync_tab(&mut r1, tab, &backend).unwrap();
    sync_tab(&mut r2, tab, &backend).unwrap();
    sync_tab(&mut r1, tab, &backend).unwrap();
    assert_eq!(r1.dump_model().unwrap(), r2.dump_model().unwrap());
    assert_eq!(r1.task(a).unwrap().title, "from r2"); // later HLC wins

    // nothing left to do
    let s = sync_tab(&mut r1, tab, &backend).unwrap();
    assert_eq!((s.pulled, s.pushed), (0, 0));
}

#[test]
fn missing_segment_blocks_cursor() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let mut r1 = Repo::open_in_memory().unwrap();
    let mut r2 = Repo::open_in_memory().unwrap();
    let tab = r1.create_tab("t").unwrap();
    sync_tab(&mut r1, tab, &backend).unwrap(); // segment 1-1
    r1.add_task(tab, Place::LastChildOf(None), "x").unwrap();
    sync_tab(&mut r1, tab, &backend).unwrap(); // segment 2-3
    // simulate the first segment not having arrived yet
    let dir = remote
        .path()
        .join(tab.to_string())
        .join("ops")
        .join(r1.device().to_string());
    let first = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .min()
        .unwrap();
    let hidden = dir.join(".hidden");
    std::fs::rename(&first, &hidden).unwrap();
    r2.add_known_tab(tab).unwrap();
    assert_eq!(sync_tab(&mut r2, tab, &backend).unwrap().pulled, 0);
    std::fs::rename(&hidden, &first).unwrap();
    assert_eq!(sync_tab(&mut r2, tab, &backend).unwrap().pulled, 3);
    assert_eq!(r1.dump_model().unwrap(), r2.dump_model().unwrap());
}

#[test]
fn ops_in_other_tabs_do_not_create_gaps() {
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let mut r1 = Repo::open_in_memory().unwrap();
    let mut r2 = Repo::open_in_memory().unwrap();
    let tab = r1.create_tab("shared").unwrap();
    sync_tab(&mut r1, tab, &backend).unwrap();
    r2.create_tab("private").unwrap(); // r2's first op is in another tab
    r2.add_known_tab(tab).unwrap();
    sync_tab(&mut r2, tab, &backend).unwrap();
    r2.add_task(tab, Place::LastChildOf(None), "from r2").unwrap();
    sync_tab(&mut r2, tab, &backend).unwrap();
    assert_eq!(sync_tab(&mut r1, tab, &backend).unwrap().pulled, 2);
    assert_eq!(r1.tree(tab).unwrap().len(), 1);
}
