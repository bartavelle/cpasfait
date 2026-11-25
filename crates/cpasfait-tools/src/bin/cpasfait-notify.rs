//! Desktop notifications for tasks that fall due, without the TUI.

use anyhow::{Context, Result};
use clap::Parser;
use cpasfait_core::Repo;
use cpasfait_core::notify::{notifications, send};
use std::path::PathBuf;
use std::time::Duration;

/// Announce, with a desktop notification that stays until dismissed
/// (notify-send), the tasks of all tabs that have fallen due. Each due date
/// is announced once, by this tool or by the TUI (they share the database).
///
/// Run it once (from cron), or keep it running with --watch, e.g. as a
/// systemd user service. It reads the local database: changes made on other
/// devices arrive when the TUI syncs.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Keep running, checking every --interval seconds.
    #[arg(short, long)]
    watch: bool,
    #[arg(short, long, default_value_t = 10)]
    interval: u64,
    /// Print the notifications instead of showing them.
    #[arg(long)]
    print: bool,
    #[arg(long)]
    db: Option<PathBuf>,
}

fn check(repo: &mut Repo, print: bool) -> Result<()> {
    let tz = jiff::tz::TimeZone::system();
    for n in notifications(&repo.take_due_notices(&tz)?) {
        if print {
            println!("{}\n  {}", n.summary, n.body.replace('\n', "\n  "));
        } else {
            send(&n)?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let path = args.db.unwrap_or_else(cpasfait_core::paths::default_db_path);
    let mut repo = Repo::open(&path).with_context(|| format!("opening {}", path.display()))?;
    loop {
        let res = check(&mut repo, args.print);
        if !args.watch {
            return res;
        }
        if let Err(e) = res {
            eprintln!("cpasfait-notify: {e:#}");
        }
        std::thread::sleep(Duration::from_secs(args.interval.max(1)));
    }
}
