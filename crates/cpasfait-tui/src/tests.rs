use crate::app::{App, Mode};
use cpasfait_core::Repo;
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
    (App::new(repo).unwrap(), time)
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
    press(&mut app, "oGroceries<enter>aMilk<enter>oEggs<esc>");
    assert_eq!(titles(&app), vec!["Groceries", "  Milk", "  Eggs"]);
    press(&mut app, "<<");
    assert_eq!(titles(&app), vec!["Groceries", "  Milk", "Eggs"]);
    press(&mut app, ">>kK");
    assert_eq!(titles(&app), vec!["Groceries", "  Milk", "  Eggs"]);
    // Esc on an empty new task discards it
    press(&mut app, "o<esc>");
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
    press(&mut app, "oA<enter>aB<enter>gg");
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
    press(&mut app, "oWrite report<enter>");
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
    assert_eq!(t.due, Some(jiff::civil::date(2030, 1, 2)));
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
    press(&mut app, "oA<enter>oB<enter>+gg");
    assert_eq!(app.repo.task(app.next.unwrap()).unwrap().title, "B");
    press(&mut app, "gn");
    assert_eq!(app.tree.get(app.selected().unwrap()).unwrap().title, "B");
}

#[test]
fn tab_join_through_directory() {
    let remote = tempfile::tempdir().unwrap();
    let dir = remote.path().to_str().unwrap().to_owned();
    let (mut a, _) = app();
    press(&mut a, "oShared task<enter>");
    press(&mut a, &format!(":tabremote {dir}<enter>"));
    // the outer loop would hand this to the sync worker; do it inline
    crate::sync_worker::run_once(&mut a.repo);
    let (mut b, _) = app();
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
        "oProject<enter>aDesign<enter>oBuild<enter>:due 2099-01-01<enter>",
    );
    press(&mut app, ":tag +dev<enter>xggs");
    assert!(matches!(app.mode, Mode::Normal));
    // no work-log lines in the snapshot (their timestamps depend on the local timezone)
    app.detail_sessions.clear();
    insta::assert_snapshot!(screen(&mut app, 90, 12));
    press(&mut app, "?");
    insta::assert_snapshot!("help", screen(&mut app, 100, 30));
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
    press(&mut app, "ofirst<esc>");
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
    press(&mut app, "oAlpha<enter>oBeta<enter>oGamma<enter>oDelta<enter>");
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
    press(&mut app, "S");
    set(at(13, 9, 0)); // Monday the 13th: this week
    press(&mut app, "ggjjs");
    set(at(13, 12, 0));
    press(&mut app, "S");
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
