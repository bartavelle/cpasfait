use crate::app::{App, Mode};
use cpasfait_core::Repo;
use cpasfait_core::crypto::Identity;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

fn app() -> (App, Arc<AtomicI64>) {
    let time = Arc::new(AtomicI64::new(1_700_000_000_000));
    let mut repo = Repo::open_in_memory().unwrap();
    let t = time.clone();
    repo.set_now(move || t.load(Ordering::SeqCst));
    repo.set_user("alice").unwrap();
    let mut app = App::new(repo).unwrap();
    // Never the user's real key file.
    app.identity = Some(Identity::generate());
    (app, time)
}

fn message(app: &App) -> String {
    app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

/// Feed keys: plain chars, plus `<enter>`, `<esc>`, `<tab>`, `<bs>`.
fn press(app: &mut App, keys: &str) {
    let mut rest = keys;
    while !rest.is_empty() {
        let (code, len) = if let Some(end) = rest.strip_prefix('<').and_then(|r| r.find('>')) {
            let name = &rest[1..=end];
            let code = match name {
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "bs" => KeyCode::Backspace,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "stab" => KeyCode::BackTab,
                _ => KeyCode::Char('<'),
            };
            if matches!(code, KeyCode::Char('<')) {
                (code, 1)
            } else {
                (code, end + 2)
            }
        } else {
            let c = rest.chars().next().unwrap();
            (KeyCode::Char(c), c.len_utf8())
        };
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        rest = &rest[len..];
    }
}

/// Run the tab moves `app` asked for (main.rs does it with a progress popup),
/// returning the progress steps.
fn run_moves(app: &mut App) -> Vec<cpasfait_core::sync::MoveStep> {
    let mut steps = vec![];
    for req in std::mem::take(&mut app.requests) {
        match req {
            crate::app::Request::MoveTab(config) => app.move_tab(&config, &mut |s| steps.push(s)),
            crate::app::Request::AcceptMove(config) => app.accept_move(&config, &mut |s| steps.push(s)),
            other => app.requests.push(other),
        }
    }
    steps
}

fn titles(app: &App) -> Vec<String> {
    app.rows
        .iter()
        .map(|r| format!("{}{}", "  ".repeat(r.depth), app.tree.get(r.id).unwrap().title))
        .collect()
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[test]
fn create_edit_indent() {
    let (mut app, _) = app();
    assert_eq!(app.tabs[0].name, "inbox");
    press(&mut app, "nGroceries<enter>aMilk<enter>nEggs<esc>");
    assert_eq!(titles(&app), vec!["Groceries", "  Milk", "  Eggs"]);
    press(&mut app, "<<");
    assert_eq!(titles(&app), vec!["Groceries", "  Milk", "Eggs"]);
    press(&mut app, ">>");
    assert_eq!(titles(&app), vec!["Groceries", "  Milk", "  Eggs"]);
    // Esc on an empty new task discards it
    press(&mut app, "n<esc>");
    assert_eq!(app.rows.len(), 3);
    // edit title
    press(&mut app, "gg");
    press(&mut app, "i<bs><bs><bs><bs><bs><bs>ies!<enter>");
    assert_eq!(titles(&app)[0], "Groies!");
    press(&mut app, "ccShopping<enter>");
    assert_eq!(titles(&app)[0], "Shopping");
}

#[test]
fn fold_done_delete_undo() {
    let (mut app, _) = app();
    press(&mut app, "nA<enter>aB<enter>gg");
    press(&mut app, "h");
    assert_eq!(titles(&app), vec!["A"]);
    press(&mut app, "l");
    assert_eq!(titles(&app), vec!["A", "  B"]);
    press(&mut app, "jxH");
    assert_eq!(titles(&app), vec!["A"]);
    press(&mut app, "Hjdd");
    assert_eq!(titles(&app), vec!["A"]);
    press(&mut app, "u");
    assert_eq!(titles(&app), vec!["A", "  B"]);
}

#[test]
fn work_toggle_and_commands() {
    let (mut app, time) = app();
    press(&mut app, "nWrite report<enter>");
    press(&mut app, "s");
    assert!(app.active.is_some());
    time.fetch_add(90_000, Ordering::SeqCst);
    press(&mut app, ":worknote drafting<enter>");
    press(&mut app, "s");
    assert!(app.active.is_none());
    assert_eq!(app.detail_sessions.len(), 1);
    assert_eq!(app.detail_sessions[0].note, "drafting");
    assert_eq!(app.detail_sessions[0].duration_ms(0), 90_000);

    press(&mut app, ":due 2030-01-02<enter>:tag +work urgent<enter>:prio 2<enter>");
    let t = app.repo.task(app.selected().unwrap()).unwrap();
    assert_eq!(t.due, Some(cpasfait_core::Due::day(jiff::civil::date(2030, 1, 2))));
    assert_eq!(t.tags, vec!["urgent", "work"]);
    assert_eq!(t.priority, 2);
    press(&mut app, ":tag -urgent<enter>");
    assert_eq!(app.repo.task(t.id).unwrap().tags, vec!["work"]);

    press(&mut app, ":bogus<enter>");
    assert!(app.message.as_ref().unwrap().error);

    press(&mut app, ":tabnew Work<enter>");
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tab().unwrap().name, "Work");
    assert!(app.rows.is_empty());
    press(&mut app, "gt");
    assert_eq!(app.tab().unwrap().name, "inbox");
    press(&mut app, "2");
    assert_eq!(app.tab().unwrap().name, "Work");
    press(&mut app, "q");
    assert!(app.quit);
}

#[test]
fn next_task_jump() {
    let (mut app, _) = app();
    press(&mut app, "nA<enter>nB<enter>");
    assert_eq!(titles(&app), vec!["A", "B"]);
    // a higher score moves it up, and the cursor follows it
    press(&mut app, "+");
    assert_eq!(titles(&app), vec!["B", "A"]);
    assert_eq!(app.tree.get(app.selected().unwrap()).unwrap().title, "B");
    press(&mut app, "G");
    assert_eq!(app.repo.task(app.next.unwrap()).unwrap().title, "B");
    press(&mut app, "gn");
    assert_eq!(app.tree.get(app.selected().unwrap()).unwrap().title, "B");
}

#[test]
fn tab_join_through_directory() {
    let remote = tempfile::tempdir().unwrap();
    let dir = remote.path().to_str().unwrap().to_owned();
    let (mut a, _) = app();
    press(&mut a, "nShared task<enter>");
    press(&mut a, &format!(":tabremote {dir}<enter>"));
    let (mut b, _) = app();
    let key = b.identity.as_ref().unwrap().recipient().to_owned();
    press(&mut a, &format!(":tabinvite {key}<enter>"));
    // the outer loop would hand this to the sync worker; do it inline
    let tab = a.tabs[a.tab_idx].id;
    let backend = cpasfait_core::backend::localdir::LocalDir::new(&dir);
    cpasfait_core::sync::sync_tab(&mut a.repo, tab, &backend, a.identity.as_ref().unwrap()).unwrap();
    press(&mut b, &format!(":tabjoin {dir}<enter>"));
    assert!(
        b.message.as_ref().is_some_and(|m| !m.error),
        "{:?}",
        b.message.as_ref().map(|m| &m.text)
    );
    assert_eq!(b.tab().unwrap().name, "inbox");
    assert_eq!(titles(&b), vec!["Shared task"]);
}

#[test]
fn render_snapshot() {
    let (mut app, _) = app();
    press(
        &mut app,
        "nProject<enter>aDesign<enter>nBuild<enter>:due 2099-01-01<enter>",
    );
    press(&mut app, ":tag +dev<enter>xggs");
    assert!(matches!(app.mode, Mode::Normal));
    // no work-log lines in the snapshot (their timestamps depend on the local timezone)
    app.detail_sessions.clear();
    insta::assert_snapshot!(screen(&mut app, 90, 12));
    press(&mut app, "?");
    insta::assert_snapshot!("help", screen(&mut app, 100, 32));
}

#[test]
fn alt_arrows_switch_tabs() {
    let (mut app, _) = app();
    press(&mut app, ":tabnew Work<enter>"); // tabs: inbox, Work; on Work
    let current = |app: &App| app.tabs[app.tab_idx].name.clone();
    assert_eq!(current(&app), "Work");
    let alt = |app: &mut App, code| app.on_key(KeyEvent::new(code, KeyModifiers::ALT));
    alt(&mut app, KeyCode::Right);
    assert_eq!(current(&app), "inbox"); // wraps around
    alt(&mut app, KeyCode::Left);
    assert_eq!(current(&app), "Work");
    alt(&mut app, KeyCode::Left);
    assert_eq!(current(&app), "inbox");
}

#[test]
fn tag_editor() {
    let (mut app, _) = app();
    press(&mut app, "nfirst<esc>");
    let id = app.selected().unwrap();
    let tags = |app: &App| app.repo.task(id).unwrap().tags;
    press(&mut app, "twork #urgent,home<enter>");
    assert_eq!(tags(&app), vec!["home", "urgent", "work"]);
    // Prefilled with the current tags; remove one by editing the list.
    let Mode::Tags { editor, .. } = ({
        press(&mut app, "t");
        &app.mode
    }) else {
        panic!("not in tag mode")
    };
    assert_eq!(editor.text(), "home urgent work ");
    press(&mut app, "<esc>"); // cancel: unchanged
    assert_eq!(tags(&app), vec!["home", "urgent", "work"]);
    press(&mut app, ":tag<enter>"); // same editor from the command line
    for _ in 0.."work ".len() {
        press(&mut app, "<bs>");
    }
    press(&mut app, "<enter>");
    assert_eq!(tags(&app), vec!["home", "urgent"]);
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn time_box_cycles_periods() {
    let (mut app, time) = app();
    app.tz = jiff::tz::TimeZone::UTC;
    press(&mut app, "nAlpha<enter>nBeta<enter>nGamma<enter>nDelta<enter>");
    let at = |d: i8, h: i8, m: i8| {
        jiff::civil::date(2023, 11, d)
            .at(h, m, 0, 0)
            .to_zoned_datetime(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_millisecond()
    };
    let set = |t: i64| time.store(t, Ordering::SeqCst);
    set(at(1, 9, 0)); // Wednesday the 1st: this month only
    press(&mut app, "Gs");
    set(at(1, 9, 30));
    press(&mut app, "S<enter>");
    set(at(13, 9, 0)); // Monday the 13th: this week
    press(&mut app, "ggjjs");
    set(at(13, 12, 0));
    press(&mut app, "S<enter>");
    set(at(14, 8, 0)); // Tuesday the 14th: today
    press(&mut app, "ggs");
    set(at(14, 8, 40));
    press(&mut app, "js"); // switches to Beta, still running
    set(at(14, 10, 45));
    app.detail_sessions.clear();

    assert!(!screen(&mut app, 90, 14).contains("time ·"));
    press(&mut app, "T");
    insta::assert_snapshot!("time_today", screen(&mut app, 90, 14));
    press(&mut app, "T");
    let week = screen(&mut app, 90, 14);
    assert!(week.contains("time · this week"), "{week}");
    assert!(week.contains("total 5h 45m"), "{week}");
    let boxed = &week[week.find("time ·").unwrap()..];
    assert!(!boxed.contains("Delta"), "{week}");
    let in_box = |name: &str| boxed.find(&format!("│{name} ")).unwrap();
    assert!(
        in_box("Gamma") < in_box("Beta") && in_box("Beta") < in_box("Alpha"),
        "{week}"
    );
    press(&mut app, "T");
    let month = screen(&mut app, 90, 14);
    let boxed = &month[month.find("time ·").unwrap()..];
    assert!(boxed.contains("│Delta ") && boxed.contains("total 6h 15m"), "{month}");
    press(&mut app, "T");
    assert!(!screen(&mut app, 90, 14).contains("time ·"));
}

#[test]
fn ctrl_p_finds_tasks_across_tabs() {
    let (mut app, _) = app();
    press(
        &mut app,
        "nWrite report<enter>aDraft outline<enter>nReview figures<enter>",
    );
    press(&mut app, "ggl<tab>"); // fold "Write report"
    press(&mut app, "ggnPrepare taxes<enter>x"); // done
    press(&mut app, "H"); // hide done tasks
    press(&mut app, ":tabnew Home<enter>nRepaint fence<enter>");
    let ctrl = |app: &mut App, c| app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    let hits = |app: &App| match &app.mode {
        Mode::Search(s) => s
            .hits
            .iter()
            .map(|h| s.candidates[h.candidate].title.clone())
            .collect::<Vec<_>>(),
        _ => panic!("not searching"),
    };

    ctrl(&mut app, 'p');
    assert_eq!(hits(&app).len(), 5);
    press(&mut app, "rep");
    // word-start matches first; other tab and done tasks lose ties
    assert_eq!(hits(&app), ["Repaint fence", "Write report", "Prepare taxes"]);
    insta::assert_snapshot!("search", screen(&mut app, 90, 16));
    press(&mut app, "<esc>");
    assert!(matches!(app.mode, Mode::Normal));

    // Go to a folded task in another tab ("/" opens the same finder).
    press(&mut app, "/draft");
    press(&mut app, "<enter>");
    assert_eq!(app.tabs[app.tab_idx].name, "inbox");
    assert_eq!(app.tree.get(app.selected().unwrap()).unwrap().title, "Draft outline");

    // Down arrow picks the second hit; a hidden done task gets shown.
    ctrl(&mut app, 'p');
    press(&mut app, "re");
    let second = hits(&app)[1].clone();
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    press(&mut app, "<enter>");
    assert_eq!(app.tree.get(app.selected().unwrap()).unwrap().title, second);
    ctrl(&mut app, 'p');
    press(&mut app, "taxes<enter>");
    assert_eq!(app.tree.get(app.selected().unwrap()).unwrap().title, "Prepare taxes");
    assert!(!app.hide_done);
}

#[test]
fn require_and_block_commands() {
    let (mut app, _) = app();
    press(&mut app, "nBuy paint<enter>nPaint fence<enter>nInvite friends<enter>");
    let id = |app: &App, title: &str| app.tree.iter().find(|t| t.title == title).unwrap().id;
    let (paint, buy, invite) = (
        id(&app, "Paint fence"),
        id(&app, "Buy paint"),
        id(&app, "Invite friends"),
    );
    let title = |app: &App| match &app.mode {
        Mode::Search(s) => s.title.clone(),
        _ => panic!("not searching"),
    };

    // Paint fence requires Buy paint (query given as argument).
    press(&mut app, "ggj:require buy<enter>");
    assert_eq!(title(&app), " “Paint fence” requires… ");
    press(&mut app, "<enter>");
    assert_eq!(app.repo.task(paint).unwrap().blocked_by, vec![buy]);
    // Paint fence blocks Invite friends (picked in the finder).
    press(&mut app, ":block<enter>inv<enter>");
    assert_eq!(app.repo.task(invite).unwrap().blocked_by, vec![paint]);
    assert_eq!(app.next, Some(buy));
    app.detail_sessions.clear();
    insta::assert_snapshot!("dependencies", screen(&mut app, 100, 14));

    // Refused: cycle.
    press(&mut app, "gg:require inv<enter><enter>");
    assert!(app.message.as_ref().unwrap().error);
    assert!(app.repo.task(buy).unwrap().blocked_by.is_empty());

    // Unrequire only offers the current requirements.
    press(&mut app, "j:unrequire<enter>");
    match &app.mode {
        Mode::Search(s) => assert_eq!(s.candidates.len(), 1),
        _ => panic!(),
    }
    press(&mut app, "<enter>");
    assert!(app.repo.task(paint).unwrap().blocked_by.is_empty());
    press(&mut app, ":unrequire<enter>");
    assert!(matches!(app.mode, Mode::Normal));
    assert!(app.message.as_ref().unwrap().error);
    press(&mut app, ":unblock<enter><enter>");
    assert!(app.repo.task(invite).unwrap().blocked_by.is_empty());
}

#[test]
fn dependencies_across_tabs_in_tui() {
    use cpasfait_core::backend::localdir::LocalDir;
    use cpasfait_core::sync::sync_tab;
    let (mut app, _) = app();
    press(&mut app, "nWrite report<enter>");
    let report = app.selected().unwrap();
    let work = app.tabs[app.tab_idx].id;
    press(&mut app, ":tabnew Home<enter>nFix car<enter>");
    let car = app.selected().unwrap();
    // From Home: "Fix car" blocks a task of another tab.
    press(&mut app, ":block report<enter>");
    let Mode::Search(s) = &app.mode else { panic!() };
    assert_eq!(s.hits.len(), 1);
    press(&mut app, "<enter>");
    assert_eq!(app.repo.task(report).unwrap().blocked_by, vec![car]);
    press(&mut app, "gT");
    assert!(app.tree.is_blocked(report));
    app.detail_sessions.clear();
    let s = screen(&mut app, 130, 13);
    assert!(s.contains("requires ○ Fix car [Home]   ⊘ 1 to do first"), "{s}");
    assert!(s.contains("⊘blocked"), "{s}");

    // A device that only has the "inbox" tab sees an error instead.
    let remote = tempfile::tempdir().unwrap();
    let backend = LocalDir::new(remote.path());
    let bob = Identity::generate();
    app.repo
        .share_tab(work, app.identity.as_ref().unwrap().recipient())
        .unwrap();
    app.repo.set_member(work, bob.recipient(), Some("bob")).unwrap();
    sync_tab(&mut app.repo, work, &backend, app.identity.as_ref().unwrap()).unwrap();
    let (mut other, _) = app_with_tab(work, &backend, bob);
    other.detail_sessions.clear();
    let tab = app.tabs.iter().find(|t| t.name == "Home").unwrap().id;
    let s = screen(&mut other, 160, 14);
    let short = |s: String| s[s.len() - 8..].to_owned();
    assert!(
        s.contains(&format!(
            "requires ⚠ unknown task …{} [tab …{}]",
            short(car.to_string()),
            short(tab.to_string())
        )),
        "{s}"
    );
    assert!(s.contains("           in a tab you don't have"), "{s}");
    assert!(!s.contains("⊘blocked"), "unknown requirements do not block: {s}");
    // ... and can still remove the link.
    press(&mut other, ":unrequire<enter><enter>");
    assert!(other.repo.task(report).unwrap().blocked_by.is_empty());
}

/// A second device that joined `tab` through `backend`.
fn app_with_tab(
    tab: cpasfait_core::TabId,
    backend: &dyn cpasfait_core::backend::RemoteBackend,
    me: Identity,
) -> (App, Arc<AtomicI64>) {
    let time = Arc::new(AtomicI64::new(1_700_000_000_000));
    let mut repo = Repo::open_in_memory().unwrap();
    let t = time.clone();
    repo.set_now(move || t.load(Ordering::SeqCst));
    repo.set_user("bob").unwrap();
    repo.add_known_tab(tab).unwrap();
    cpasfait_core::sync::sync_tab(&mut repo, tab, backend, &me).unwrap();
    let mut app = App::new(repo).unwrap();
    app.identity = Some(me);
    let i = app.tabs.iter().position(|t| t.id == tab).unwrap();
    app.tab_idx = i;
    app.refresh().unwrap();
    app.list.select(Some(0));
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    (app, time)
}

#[test]
fn sharing_a_tab() {
    use cpasfait_core::backend::localdir::LocalDir;
    use cpasfait_core::sync::sync_tab;
    let remote = tempfile::tempdir().unwrap();
    let dir = remote.path().display().to_string();
    let backend = LocalDir::new(remote.path());
    let (mut alice, _) = app();
    press(&mut alice, "nPaint<enter>");
    press(&mut alice, ":tabinvite age1x<enter>");
    assert!(message(&alice).contains("not shared"), "{}", message(&alice));
    press(&mut alice, &format!(":tabremote {dir}<enter>"));
    assert!(message(&alice).contains("encrypted"), "{}", message(&alice));
    let tab = alice.tabs[alice.tab_idx].id;
    sync_tab(&mut alice.repo, tab, &backend, alice.identity.as_ref().unwrap()).unwrap();

    // Bob joins before being invited: told what to send.
    let (mut bob, _) = app();
    bob.repo.set_user("bob").unwrap();
    press(&mut bob, ":mykey<enter>");
    let bob_key = bob.identity.as_ref().unwrap().recipient().to_owned();
    assert!(message(&bob).contains(&bob_key));
    press(&mut bob, &format!(":tabjoin {dir}<enter>"));
    assert!(
        message(&bob).contains(&format!(":tabinvite {bob_key}")),
        "{}",
        message(&bob)
    );

    press(&mut alice, &format!(":tabinvite {bob_key} Bob B.<enter>"));
    assert!(
        message(&alice).contains(&format!("invited Bob B.: they can now run :tabjoin {dir} {tab}")),
        "{}",
        message(&alice)
    );
    press(&mut alice, &format!(":tabinvite {bob_key}<enter>"));
    assert!(message(&alice).contains("already a member"));
    press(&mut alice, ":tabmembers<enter>");
    let m = message(&alice);
    assert!(
        m.starts_with("members: alice (age1") && m.contains(", you), Bob B. (age1"),
        "{m}"
    );
    sync_tab(&mut alice.repo, tab, &backend, alice.identity.as_ref().unwrap()).unwrap();

    sync_tab(&mut bob.repo, tab, &backend, bob.identity.as_ref().unwrap()).unwrap();
    bob.refresh().unwrap();
    assert_eq!(bob.tree.iter().next().unwrap().title, "Paint");

    press(&mut alice, ":tabrevoke nobody<enter>");
    assert!(message(&alice).contains("no member"));
    press(&mut alice, ":tabrevoke Bob B.<enter>");
    assert!(message(&alice).starts_with("removed Bob B."), "{}", message(&alice));
    press(&mut alice, ":tabrevoke alice<enter>");
    assert!(message(&alice).contains("last member"), "{}", message(&alice));
}

#[test]
fn key_arguments() {
    use crate::app::short_key;
    assert_eq!(
        short_key("age1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqs3290gq"),
        "age1qyqs…90gq"
    );
    assert_eq!(
        short_key("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsK"),
        "ssh-ed25519 …AAAAIHsK"
    );
}

#[test]
fn moving_a_tab_between_remotes() {
    use cpasfait_core::backend::localdir::LocalDir;
    use cpasfait_core::sync::sync_tab;
    let tmp = tempfile::tempdir().unwrap();
    let (one, two) = (tmp.path().join("one"), tmp.path().join("two"));
    let (mut alice, _) = app();
    let (mut bob, _) = app();
    press(&mut alice, "nPaint<enter>");
    press(&mut alice, &format!(":tabremote {}<enter>", one.display()));
    let bob_key = bob.identity.as_ref().unwrap().recipient().to_owned();
    press(&mut alice, &format!(":tabinvite {bob_key} bob<enter>"));
    let tab = alice.tabs[alice.tab_idx].id;
    sync_tab(
        &mut alice.repo,
        tab,
        &LocalDir::new(&one),
        alice.identity.as_ref().unwrap(),
    )
    .unwrap();
    press(&mut bob, &format!(":tabjoin {}<enter>", one.display()));
    assert_eq!(message(&bob), "joined");

    press(&mut bob, ":tabaccept<enter>");
    assert!(message(&bob).contains("no remote move"), "{}", message(&bob));
    press(&mut alice, &format!(":tabmove {}<enter>", one.display()));
    assert!(message(&alice).contains("already there"));
    press(&mut alice, &format!(":tabmove {}<enter>", two.display()));
    let steps = run_moves(&mut alice);
    assert!(message(&alice).starts_with("moved to "), "{}", message(&alice));
    {
        use cpasfait_core::sync::MoveStep::*;
        let Some(&Downloading { total, .. }) = steps.get(2) else {
            panic!("{steps:?}")
        };
        assert!(total > 0);
        let mut expected = vec![SyncOld, Listing];
        expected.extend((0..=total).map(|done| Downloading { done, total }));
        expected.extend((0..=total).map(|done| Uploading { done, total }));
        expected.push(SyncNew);
        assert_eq!(steps, expected);
        // What the popup looks like.
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(50, 6)).unwrap();
        t.draw(|f| {
            crate::ui::render_progress(
                f,
                "Moving",
                Uploading { done: 1, total: 4 },
                std::time::Duration::from_secs(3),
            )
        })
        .unwrap();
        let text: String = t.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("uploading files: 1/4…  3s"), "{text}");
        assert!(text.contains("25%"), "{text}");
    }
    assert!(two.join(tab.to_string()).join("keys").is_dir());

    // Bob hears about it from the old remote.
    sync_tab(&mut bob.repo, tab, &LocalDir::new(&one), bob.identity.as_ref().unwrap()).unwrap();
    bob.refresh().unwrap();
    assert_eq!(
        message(&bob),
        format!(
            "alice moved this tab to {}: :tabaccept to follow (you can edit the remote first)",
            two.display()
        )
    );
    assert!(screen(&mut bob, 80, 5).contains("inbox ⇅↪"));
    press(&mut bob, ":tabaccept<enter>");
    let Mode::Command(e) = &bob.mode else { panic!() };
    assert_eq!(e.text(), format!("tabaccept {}", two.display()));
    press(&mut bob, "<enter>");
    assert_eq!(
        run_moves(&mut bob).last(),
        Some(&cpasfait_core::sync::MoveStep::SyncNew)
    );
    assert_eq!(message(&bob), format!("now using {}", two.display()));
    assert!(!screen(&mut bob, 80, 5).contains('↪'));
    assert_eq!(
        bob.tab().unwrap().remote,
        Some(cpasfait_core::RemoteConfig::LocalDir {
            path: two.display().to_string()
        })
    );
    assert_eq!(titles(&bob), vec!["Paint"]);
}

#[test]
fn tab_settings() {
    let (mut app, _) = app();
    press(&mut app, ":tabset<enter>");
    assert_eq!(message(&app), "default_priority=0 (priority of new tasks)");
    press(&mut app, ":tabset default_priority 3<enter>");
    assert_eq!(message(&app), "default_priority=3 (priority of new tasks)");
    press(&mut app, "nnew<enter>");
    let t = app.selected().unwrap();
    assert_eq!(app.repo.task(t).unwrap().priority, 3);
    press(&mut app, ":tabset default_priority=x<enter>");
    assert!(message(&app).contains("expected an integer"), "{}", message(&app));
    press(&mut app, ":tabset nope<enter>");
    assert!(message(&app).contains("unknown setting"), "{}", message(&app));
    press(&mut app, ":tabunset default_priority<enter>");
    assert_eq!(message(&app), "default_priority=0 (priority of new tasks)");
}

#[test]
fn help_scrolls() {
    let (mut app, _) = app();
    press(&mut app, "?");
    let top = screen(&mut app, 100, 12);
    assert!(top.contains("Navigation"), "{top}");
    assert!(top.contains("↓ j/k"), "{top}");
    press(&mut app, "x"); // other keys do nothing
    assert!(matches!(app.mode, Mode::Help { .. }));
    press(&mut app, "G");
    let bottom = screen(&mut app, 100, 12);
    assert!(bottom.contains("close this help"), "{bottom}");
    assert!(!bottom.contains("Navigation"), "{bottom}");
    assert!(bottom.contains("↑ j/k"), "{bottom}");
    // clamped: one k is one line up from the end
    press(&mut app, "k");
    let up = screen(&mut app, 100, 12);
    assert!(!up.contains("close this help"), "{up}");
    press(&mut app, "gj");
    assert!(!screen(&mut app, 100, 12).contains("Navigation"));
    press(&mut app, "q");
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn stop_and_switch_at_a_given_time() {
    let (mut app, time) = app();
    app.tz = jiff::tz::TimeZone::UTC;
    let at = |h: i8, m: i8| {
        jiff::civil::date(2023, 11, 14)
            .at(h, m, 0, 0)
            .to_zoned_datetime(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_millisecond()
    };
    let set = |t: i64| time.store(t, Ordering::SeqCst);
    press(&mut app, "nAlpha<enter>nBeta<enter>gg");
    let (alpha, beta) = (app.rows[0].id, app.rows[1].id);
    press(&mut app, "S");
    assert_eq!(message(&app), "nothing running");

    set(at(9, 0));
    press(&mut app, "s"); // Alpha
    set(at(11, 0));
    // W: the command line, prefilled with the time, to edit
    press(&mut app, "jW");
    let Mode::Command(editor) = &app.mode else { panic!() };
    assert_eq!(editor.text(), "switch 11:00");
    press(&mut app, "<bs><bs><bs><bs><bs>08:00<enter>");
    assert_eq!(message(&app), "08:00 is before \"Alpha\" started (09:00)");
    press(&mut app, ":switch 10:30<enter>");
    assert_eq!(
        message(&app),
        "working on \"Beta\" since 10:30, \"Alpha\" stopped (1h30)"
    );
    assert_eq!(app.repo.sessions_for_task(alpha).unwrap()[0].end, Some(at(10, 30)));
    press(&mut app, ":switch<enter>");
    assert_eq!(message(&app), "already working on \"Beta\"");

    set(at(12, 0));
    press(&mut app, "S");
    let Mode::Command(editor) = &app.mode else { panic!() };
    assert_eq!(editor.text(), "stop 12:00");
    press(&mut app, "<esc>:stop -1h<enter>");
    assert_eq!(message(&app), "stopped \"Beta\" at 11:00 (30m)");
    assert!(app.repo.active_session().unwrap().is_none());
    assert_eq!(app.repo.sessions_for_task(beta).unwrap()[0].end, Some(at(11, 0)));
    press(&mut app, ":stop<enter>");
    assert_eq!(message(&app), "nothing running");
    press(&mut app, ":switch noon<enter>");
    assert!(message(&app).starts_with("can't parse time"), "{}", message(&app));
    // nothing running: a back-dated start, not overlapping the last session
    // (a time not yet reached today is yesterday)
    press(&mut app, ":switch 23:00<enter>");
    assert_eq!(
        message(&app),
        "2023-11-13 23:00 is before your last session ended (11:00)"
    );
    press(&mut app, "k:switch 11:15<enter>");
    assert_eq!(message(&app), "working on \"Alpha\" since 11:15");
}

#[test]
fn time_box_counts_all_tabs() {
    let (mut app, time) = app();
    app.tz = jiff::tz::TimeZone::UTC;
    let add = |m: i64| {
        time.fetch_add(m * 60_000, Ordering::SeqCst);
    };
    press(&mut app, "nHere<enter>s");
    add(30);
    press(&mut app, ":tabnew Home<enter>nChores<enter>s");
    add(45);
    press(&mut app, "nDishes<enter>s");
    add(10);
    press(&mut app, "aRinse<enter>s"); // a subtask of Dishes
    add(5);
    press(&mut app, "gTT"); // back to the first tab
    let s = screen(&mut app, 90, 12);
    let boxed = &s[s.find("time ·").unwrap()..];
    assert!(boxed.contains("total 1h 30m"), "{s}");
    for line in ["Chores [Home]", "Here ", "Dishes [Home]", "Rinse [Home]"] {
        assert!(boxed.contains(line), "{line}\n{s}");
    }
    assert!(boxed.find("Chores").unwrap() < boxed.find("Here").unwrap(), "{s}");

    // deleted tasks (and their subtasks) do not count, in any tab
    press(&mut app, "dd"); // Here
    press(&mut app, "gtggdd"); // Chores
    press(&mut app, "ggdd"); // Dishes, with Rinse
    press(&mut app, "gT");
    let s = screen(&mut app, 90, 12);
    let boxed = &s[s.find("time ·").unwrap()..s.find(" NORMAL").unwrap()];
    assert!(boxed.contains("total 0s"), "{s}");
    assert!(!boxed.contains("Dishes") && !boxed.contains("Rinse"), "{s}");
}

#[test]
fn priority_range_and_display() {
    let (mut app, _) = app();
    press(&mut app, "nnormal<enter>nlater<enter>");
    let later = app.selected().unwrap();
    press(&mut app, "----"); // stops at -3
    assert_eq!(app.repo.task(later).unwrap().priority, -3);
    let s = screen(&mut app, 60, 6);
    assert!(s.contains("↓↓↓ later"), "{s}");
    press(&mut app, "+++++++"); // stops at 3
    assert_eq!(app.repo.task(later).unwrap().priority, 3);
    press(&mut app, ":prio -2<enter>");
    assert_eq!(app.repo.task(later).unwrap().priority, -2);
    press(&mut app, ":prio 7<enter>");
    assert!(message(&app).contains("expected -3 to 3"), "{}", message(&app));
    let s = screen(&mut app, 60, 6);
    // low priority: listed after the default ones
    assert!(s.find("normal").unwrap() < s.find("↓↓ later").unwrap(), "{s}");
}

#[test]
fn edit_task_in_editor() {
    use crate::app::Request;
    let (mut app, time) = app();
    app.tz = jiff::tz::TimeZone::UTC;
    // 2023-11-14 22:13:20 UTC
    press(&mut app, "nAlpha<enter>s");
    time.fetch_add(3_600_000, Ordering::SeqCst);
    press(&mut app, "s");
    let task = app.rows[0].id;
    let ms = |s: &str| {
        s.parse::<jiff::civil::DateTime>()
            .unwrap()
            .to_zoned_datetime(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_millisecond()
    };
    let month_ago = Some(ms("2023-10-15T00:00"));
    press(&mut app, "E");
    assert_eq!(
        std::mem::take(&mut app.requests),
        vec![Request::EditTask(task, month_ago)]
    );
    press(&mut app, ":edit<enter>");
    assert_eq!(
        std::mem::take(&mut app.requests),
        vec![Request::EditTask(task, month_ago)]
    );
    press(&mut app, ":edit all<enter>");
    assert_eq!(std::mem::take(&mut app.requests), vec![Request::EditTask(task, None)]);
    press(&mut app, ":edit 2023-11-01<enter>");
    let since = Some(ms("2023-11-01T00:00"));
    assert_eq!(std::mem::take(&mut app.requests), vec![Request::EditTask(task, since)]);
    press(&mut app, ":edit whenever<enter>");
    assert!(message(&app).starts_with("usage: :edit"), "{}", message(&app));

    let (text, since) = app.task_file(task, since).unwrap();
    assert!(text.contains("\n2023-11-14 22:13:20 - 2023-11-14 23:13:20\n"), "{text}");
    let edited = text.replace("title: Alpha", "title: Alpha 2") + "2023-11-14 20:00 - 2023-11-14 2l:00\n";
    app.finish_task_edit(task, since, Ok(edited));
    assert!(message(&app).starts_with("line 28: expected"), "{}", message(&app));
    assert_eq!(app.repo.task(task).unwrap().title, "Alpha");
    // the next edit shows the failed text, with the error
    // (with the window it was made with)
    let (again, again_since) = app.task_file(task, None).unwrap();
    assert_eq!(again_since, since);
    assert!(again.starts_with("# error: line 28: expected"), "{again}");
    let fixed = again.replace("2l:00", "21:00");
    app.finish_task_edit(task, since, Ok(fixed));
    assert_eq!(message(&app), "title changed, 1 session added");
    assert_eq!(app.repo.task(task).unwrap().title, "Alpha 2");
    assert_eq!(app.repo.sessions_for_task(task).unwrap().len(), 2);
    // back to the task's own text
    assert!(!app.task_file(task, since).unwrap().0.contains("# error"));
    app.finish_task_edit(task, since, Ok(String::new()));
    assert_eq!(message(&app), "empty file: nothing changed");
    app.finish_task_edit(task, since, Err("editor exited with 1".into()));
    assert_eq!(message(&app), "editor exited with 1: nothing changed");
}

#[test]
fn detail_pane_shows_latest_sessions() {
    let (mut app, time) = app();
    app.tz = jiff::tz::TimeZone::UTC;
    // 2023-11-14 22:13:20 UTC: 12 sessions of 10 minutes, one per day back
    press(&mut app, "nAdmin<enter>");
    let task = app.rows[0].id;
    let day = 86_400_000;
    let t0 = time.load(Ordering::SeqCst);
    for i in (0..12).rev() {
        time.store(t0 - i * day, Ordering::SeqCst);
        app.repo.start_work(task).unwrap();
        time.store(t0 - i * day + 600_000, Ordering::SeqCst);
        app.repo.stop_work().unwrap();
    }
    app.refresh().unwrap();
    let s = screen(&mut app, 100, 30);
    // Tuesday: this week is today and yesterday
    assert!(s.contains("work log  total 2h00\n"), "{s}");
    assert!(s.contains("today 10m · this week 20m"), "{s}");
    assert_eq!(s.matches("10m alice").count(), 8, "{s}");
    assert!(s.contains("… 4 older sessions, E to edit"), "{s}");
}

#[test]
fn move_task_to_another_tab() {
    let (mut app, time) = app();
    press(&mut app, ":tabnew Home<enter>gT");
    press(&mut app, "nPaint<enter>aBuy paint<enter>ks");
    time.fetch_add(600_000, Ordering::SeqCst);
    press(&mut app, ":moveto nowhere<enter>");
    assert!(message(&app).contains("nowhere"), "{}", message(&app));
    press(&mut app, ":moveto ho<enter>");
    assert_eq!(
        message(&app),
        "moved \"Paint\" with 1 subtask and 1 work session to Home"
    );
    assert!(app.rows.is_empty());
    press(&mut app, "gt");
    assert_eq!(titles(&app), ["Paint", "  Buy paint"]);
    // still working on it
    let active = app.active.clone().unwrap();
    assert_eq!(active.task, Some(app.rows[0].id));
}

#[test]
fn due_times_and_notifications() {
    let (mut app, time) = app();
    app.tz = jiff::tz::TimeZone::system(); // repo.today() uses the system zone
    let at = |h: i8, m: i8| {
        jiff::civil::date(2026, 11, 25)
            .at(h, m, 0, 0)
            .to_zoned_datetime(jiff::tz::TimeZone::system())
            .unwrap()
            .timestamp()
            .as_millisecond()
    };
    time.store(at(9, 0), Ordering::SeqCst);
    press(&mut app, "nCall Bob<enter>:due 14:30<enter>");
    let t = app.selected().unwrap();
    let due = app.repo.task(t).unwrap().due.unwrap();
    assert_eq!(due.to_string(), "2026-11-25 14:30", "a time alone is today");
    press(&mut app, ":due tom 9:15:30<enter>");
    assert_eq!(
        app.repo.task(t).unwrap().due.unwrap().to_string(),
        "2026-11-26 09:15:30"
    );
    press(&mut app, ":due 25:00<enter>");
    assert!(message(&app).starts_with("can't parse due date"), "{}", message(&app));
    press(&mut app, ":due 14:30<enter>");
    let s = screen(&mut app, 90, 8);
    assert!(s.contains("Call Bob  ⌛ 2026-11-25 14:30"), "{s}");

    // notifications: off by default (tests), once per due date
    time.store(at(14, 30), Ordering::SeqCst);
    assert!(app.due_notifications().unwrap().is_empty());
    app.notify = true;
    let n = app.due_notifications().unwrap();
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].summary, "Due: Call Bob");
    assert_eq!(n[0].body, "2026-11-25 14:30 · inbox");
    assert_eq!(message(&app), "due: \"Call Bob\" (2026-11-25 14:30)");
    assert!(app.due_notifications().unwrap().is_empty());
}

#[test]
fn alert_popup_shows_everything_and_any_key_closes_it() {
    let (mut app, _) = app();
    press(&mut app, "none<esc>");
    let long = "tab \"work\": rsync: ssh: connect to host example.invalid port 22: Name or service not known; \
                rsync error: unexplained error (code 255) at io.c(232) [Receiver=3.2.7] THE-END";
    app.alert("Sync failed", long);
    let s = screen(&mut app, 60, 20);
    assert!(s.contains("Sync failed"), "{s}");
    assert!(s.contains("THE-END"), "{s}");
    assert!(s.contains("any key to close"), "{s}");
    // The key closes the popup and does nothing else.
    press(&mut app, "dd");
    assert!(app.alert.is_none());
    assert_eq!(titles(&app), vec!["one"]);
    assert!(!screen(&mut app, 60, 20).contains("Sync failed"));
}

#[test]
fn wrap_cuts_long_words() {
    let w = crate::ui::wrap("aaa bbbbbbbbbb c\nd", 4);
    assert_eq!(w, vec!["aaa", "bbbb", "bbbb", "bb c", "d"]);
}

fn cmdline(app: &App) -> String {
    match &app.mode {
        Mode::Command(e) => e.text().to_owned(),
        Mode::Tags { editor, .. } => editor.text().to_owned(),
        _ => panic!("not editing"),
    }
}

#[test]
fn command_history() {
    let (mut app, _) = app();
    press(&mut app, ":tabnew a<enter>:tabnew b<enter>:user bob<enter>");
    press(&mut app, ":<up>");
    assert_eq!(cmdline(&app), "user bob");
    press(&mut app, "<up><up>");
    assert_eq!(cmdline(&app), "tabnew a");
    press(&mut app, "<up>");
    assert_eq!(cmdline(&app), "tabnew a");
    press(&mut app, "<down><down><down>");
    assert_eq!(cmdline(&app), "");
    // Only those starting with what was typed.
    press(&mut app, "tab<up>");
    assert_eq!(cmdline(&app), "tabnew b");
    press(&mut app, "<down>");
    assert_eq!(cmdline(&app), "tab");
    press(&mut app, "<esc>");
}

#[test]
fn tab_completion() {
    let (mut app, _) = app();
    press(
        &mut app,
        ":tabnew Work stuff<enter>:tabnew Home<enter>nx<esc>:tag +urgent +later<enter>",
    );
    // A unique command.
    press(&mut app, ":tabrem<tab>");
    assert_eq!(cmdline(&app), "tabremote ");
    press(&mut app, "<esc>");
    // Several: the common part, then cycle (S-Tab goes back).
    press(&mut app, ":tabm<tab>");
    assert_eq!(cmdline(&app), "tabm");
    assert_eq!(app.completion.as_ref().unwrap().items, ["tabmembers", "tabmove"]);
    let s = screen(&mut app, 80, 10);
    assert!(s.contains("tabmembers") && s.contains("tabmove"), "{s}");
    press(&mut app, "<tab>");
    assert_eq!(cmdline(&app), "tabmembers");
    press(&mut app, "<tab>");
    assert_eq!(cmdline(&app), "tabmove");
    press(&mut app, "<stab>");
    assert_eq!(cmdline(&app), "tabmembers");
    // Typing ends it.
    press(&mut app, "x");
    assert!(app.completion.is_none());
    press(&mut app, "<esc>");
    // Tags, after the sign.
    press(&mut app, ":tag -u<tab>");
    assert_eq!(cmdline(&app), "tag -urgent ");
    press(&mut app, "<esc>");
    // Tab names, spaces included, case ignored.
    press(&mut app, ":moveto wo<tab>");
    assert_eq!(cmdline(&app), "moveto Work stuff");
    press(&mut app, "<esc>");
    // In the tag editor too.
    press(&mut app, "ny<esc>tl<tab>");
    assert_eq!(cmdline(&app), "later ");
    press(&mut app, "<tab>");
    assert_eq!(cmdline(&app), "later urgent ");
    press(&mut app, "<tab>");
    assert_eq!(cmdline(&app), "later urgent ", "nothing left");
    press(&mut app, "<esc>");
    // Remotes typed before.
    app.history.add("tabjoin rsync:me@host:dir 0190");
    press(&mut app, ":tabremote rs<tab>");
    assert_eq!(cmdline(&app), "tabremote rsync:me@host:dir");
    press(&mut app, "<esc>");
}

#[test]
fn notes_on_tasks() {
    let (mut app, _) = app();
    app.tz = jiff::tz::TimeZone::UTC;
    press(&mut app, "nreport<esc>");
    press(&mut app, "N");
    assert_eq!(cmdline(&app), "note ");
    press(&mut app, "sent the draft to Bob<enter>");
    assert_eq!(app.detail_notes.len(), 1);
    let s = screen(&mut app, 100, 20);
    assert!(s.contains("notes") && s.contains("  sent the draft to Bob"), "{s}");
    // :note alone: the task's file, where "@@" adds one.
    press(&mut app, ":note<enter>");
    let task = app.selected().unwrap();
    assert!(matches!(app.requests[..], [crate::app::Request::EditTask(t, _)] if t == task));
    app.requests.clear();
}

#[test]
fn edit_the_whole_task() {
    let (mut app, _) = app();
    app.tz = jiff::tz::TimeZone::UTC;
    press(&mut app, "nreport<esc>nother<esc>");
    let task = app.rows[0].id;
    let other = app.rows[1].id;
    app.repo.add_note(task, "first").unwrap();
    let (text, since) = app.task_file(task, None).unwrap();
    // Unset optional fields are commented out, the due date with now.
    assert!(
        text.starts_with(
            "title: report\nstatus: todo\n# priority: 0\n# due: 2023-11-14 22:13:20\n# tags: TAG OTHER-TAG\n# requires: TASK-ID\n"
        ),
        "{text}"
    );
    assert!(text.contains("\nnotes:\n@@ 1 · 2023-11-14 22:13 · "), "{text}");
    let edited = text
        .replace("# priority: 0", "priority: 2")
        .replace("# due: 2023-11-14 22:13:20", "due: 2023-12-01")
        .replace("# tags: TAG OTHER-TAG", "tags: work +urgent")
        .replace("# requires: TASK-ID", &format!("requires: {other}"))
        .replace("description:\n", "description:\n# Plan\n@@ not a note\n")
        .replace("\nfirst\n", "\nfirst, fixed\n@@\nsecond\n");
    app.finish_task_edit(task, since, Ok(edited));
    assert_eq!(
        message(&app),
        "priority, due, tags, description, requires changed, 1 note added, 1 note changed"
    );
    let t = app.repo.task(task).unwrap();
    assert_eq!(t.priority, 2);
    assert_eq!(t.due.unwrap().to_string(), "2023-12-01");
    assert_eq!(t.tags, ["urgent", "work"]);
    assert_eq!(t.blocked_by, [other]);
    assert_eq!(t.notes, "# Plan\n@@ not a note");
    let notes: Vec<_> = app
        .repo
        .notes_for_task(task)
        .unwrap()
        .into_iter()
        .map(|n| n.text)
        .collect();
    assert_eq!(notes.len(), 2);
    assert!(notes.contains(&"first, fixed".to_owned()) && notes.contains(&"second".to_owned()));

    // Set fields are shown as is; commenting them out unsets them.
    let (text, since) = app.task_file(task, None).unwrap();
    assert!(
        text.contains("\npriority: 2\ndue: 2023-12-01\ntags: urgent work\nrequires: "),
        "{text}"
    );
    assert!(text.contains("\ndescription:\n# Plan\n\\@@ not a note\n"), "{text}");
    let edited = text
        .replace("due: 2023-12-01", "# due: 2023-12-01")
        .replace("\ntags: urgent work", "\n#tags: urgent work")
        .replace("status: todo", "status: done");
    app.finish_task_edit(task, since, Ok(edited));
    assert_eq!(message(&app), "status, due, tags changed");
    let t = app.repo.task(task).unwrap();
    assert!(t.due.is_none() && t.tags.is_empty() && t.is_done());

    // Errors: nothing changed.
    let (text, since) = app.task_file(task, None).unwrap();
    app.finish_task_edit(task, since, Ok(text.replace("priority: 2", "priority: 9")));
    assert!(
        message(&app).contains("priority is a number from -3 to 3"),
        "{}",
        message(&app)
    );
    let (text, since) = app.task_file(task, None).unwrap();
    let text = text
        .replace("priority: 9", "priority: 1")
        .replace(&format!("requires: {other}"), &format!("requires: {task}"));
    app.finish_task_edit(task, since, Ok(text));
    assert!(message(&app).contains("cannot require itself"), "{}", message(&app));
    assert_eq!(app.repo.task(task).unwrap().priority, 2);
}
