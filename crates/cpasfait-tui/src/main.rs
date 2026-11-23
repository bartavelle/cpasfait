mod app;
mod config;
mod editor;
mod fuzzy;
mod keymap;
mod search;
mod sync_worker;
mod timebox;
mod ui;

use anyhow::{Context, Result, bail};
use app::{App, Request};
use cpasfait_core::Repo;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sync_worker::SyncWorker;

const USAGE: &str = "usage: cpasfait [--db PATH] [--no-sync]

Configuration: ~/.config/cpasfait/config.toml (created on first run: asks for
your email, recorded as the author of your tasks).

Environment: CPASFAIT_CONFIG (configuration file), CPASFAIT_DATA_DIR (database
directory; its config.toml is used instead of the default one), CPASFAIT_USER
(display name for new databases), EDITOR (editor for notes and E).";

const SYNC_INTERVAL: Duration = Duration::from_secs(60);

fn main() -> Result<()> {
    let mut db = cpasfait_core::paths::default_db_path();
    let mut sync = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--db" => db = PathBuf::from(args.next().context("--db needs a path")?),
            "--no-sync" => sync = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown argument {other:?}\n{USAGE}"),
        }
    }
    let config = config::load_or_create()?;
    let mut repo = Repo::open(&db).with_context(|| format!("opening {}", db.display()))?;
    if let Some(name) = config.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
        && name != repo.user()
    {
        repo.set_user(name)?;
    }
    if config.email.is_some() && config.email.as_deref() != repo.email() {
        repo.set_email(config.email.as_deref())?;
    }
    let mut app = App::new(repo)?;
    let worker = sync.then(|| SyncWorker::spawn(db.clone(), SYNC_INTERVAL));

    let mut terminal = ratatui::init();
    let res = run(&mut terminal, &mut app, worker.as_ref());
    ratatui::restore();
    res?;

    // Last push so other devices see our latest changes.
    if sync {
        let report = sync_worker::run_once(&mut app.repo);
        for e in report.errors {
            eprintln!("sync error: {e}");
        }
    }
    Ok(())
}

fn run(terminal: &mut DefaultTerminal, app: &mut App, worker: Option<&SyncWorker>) -> Result<()> {
    let mut last_edit_sync: Option<Instant> = None;
    // Number of explicit sync requests whose result should be reported.
    let mut explicit_syncs = 0usize;
    loop {
        terminal.draw(|f| ui::render(f, app))?;
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(500))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    let before = app.repo.local_changes();
                    app.on_key(k);
                    // Local edits: push them soon (debounced).
                    if worker.is_some() && app.repo.local_changes() != before {
                        last_edit_sync.get_or_insert_with(Instant::now);
                    }
                }
                _ => {}
            }
        }
        for req in std::mem::take(&mut app.requests) {
            match req {
                Request::Sync => match worker {
                    Some(w) => {
                        w.request();
                        explicit_syncs += 1;
                    }
                    None => app.error("sync disabled (--no-sync)"),
                },
                Request::EditNotes(task) => {
                    if let Err(e) = edit_notes(terminal, app, task) {
                        app.error(format!("{e:#}"));
                    }
                }
                Request::EditTask(task) => match app.task_file(task) {
                    Ok(text) => {
                        let edited = run_editor(terminal, &text, "txt").map_err(|e| format!("{e:#}"));
                        app.finish_task_edit(task, edited);
                    }
                    Err(e) => app.error(format!("{e:#}")),
                },
            }
        }
        if let (Some(w), Some(t)) = (worker, last_edit_sync)
            && t.elapsed() > Duration::from_secs(3)
        {
            w.request();
            last_edit_sync = None;
        }
        if let Some(w) = worker {
            while let Ok(r) = w.results.try_recv() {
                app.sync_status = Some(if r.errors.is_empty() {
                    let now = jiff::ZonedDateTime::now().strftime("%H:%M").to_string();
                    format!("synced {now}")
                } else {
                    "sync failed".into()
                });
                if let Some(e) = r.errors.first() {
                    app.error(format!("sync: {e}"));
                } else if r.waiting_for_key > 0 {
                    app.info("some changes use a key you don't have yet (not invited back, or keys not synced yet)");
                } else if explicit_syncs > 0 {
                    app.info(format!("synced: {} received, {} sent", r.pulled, r.pushed));
                } else if r.pulled > 0 {
                    app.info(format!("received {} changes", r.pulled));
                }
                explicit_syncs = explicit_syncs.saturating_sub(1);
            }
        }
        if let Err(e) = app.refresh_if_changed() {
            app.error(format!("{e:#}"));
        }
    }
}

fn edit_notes(terminal: &mut DefaultTerminal, app: &mut App, task: cpasfait_core::TaskId) -> Result<()> {
    let t = app.repo.task(task)?;
    let notes = run_editor(terminal, &t.notes, "md")?.trim_end().to_owned();
    if notes != t.notes {
        app.repo.set_notes(task, &notes)?;
        app.refresh()?;
        app.info("notes saved");
    }
    Ok(())
}

/// Edit `text` in $VISUAL / $EDITOR (default vi), in a temporary file with
/// extension `ext`. Errors when the editor fails (e.g. vim's `:cq`).
fn run_editor(terminal: &mut DefaultTerminal, text: &str, ext: &str) -> Result<String> {
    let path = std::env::temp_dir().join(format!("cpasfait-{}.{ext}", uuid_like()));
    std::fs::write(&path, text)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    ratatui::restore();
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&path)
        .status();
    *terminal = ratatui::init();
    terminal.clear()?;
    let status = status.context("running editor");
    let edited = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let status = status?;
    if !status.success() {
        bail!("editor exited with {status}");
    }
    Ok(edited?)
}

fn uuid_like() -> String {
    format!("{}-{}", std::process::id(), jiff::Timestamp::now().as_nanosecond())
}

#[cfg(test)]
mod tests;
