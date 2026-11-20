//! Prints the most prioritary task of a tab. Must stay fast (status bars, prompts).
//!
//! Exit status: 0 = a task was printed, 2 = nothing to do, 1 = error.

use anyhow::Result;
use clap::Parser;
use cpasfait_tools::{open_db, pick_tab};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

/// Print the most prioritary actionable task of a tab: not done, without open
/// subtasks, highest priority first, then earliest due date, then tree order.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Tab name (or unique prefix, or id). Optional when there is a single tab.
    #[arg(short, long)]
    tab: Option<String>,
    /// Database path (default: $CPASFAIT_DATA_DIR/cpasfait.db or the platform data dir).
    #[arg(long)]
    db: Option<PathBuf>,
    /// Print the full path (Parent › Child) instead of only the title.
    #[arg(short, long)]
    path: bool,
    /// Also print priority, due date and tags.
    #[arg(short, long)]
    long: bool,
    /// JSON output.
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct Out<'a> {
    id: String,
    tab: &'a str,
    title: &'a str,
    path: Vec<&'a str>,
    priority: i64,
    due: Option<String>,
    tags: &'a [String],
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("cpasfait-next: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    let args = Args::parse();
    let repo = open_db(args.db)?;
    let tab = pick_tab(&repo, args.tab.as_deref())?;
    // `structure` skips titles/notes/tags: only the winner is loaded fully.
    let tree = repo.structure(tab.id)?;
    let Some(id) = tree.next_task() else {
        return Ok(false);
    };
    let t = repo.task(id)?;
    let mut path = vec![];
    for a in tree.ancestors(id).into_iter().rev() {
        path.push(repo.task(a)?.title);
    }
    path.push(t.title.clone());
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    if args.json {
        let out = Out {
            id: t.id.to_string(),
            tab: &tab.name,
            title: &t.title,
            path,
            priority: t.priority,
            due: t.due.map(|d| d.to_string()),
            tags: &t.tags,
        };
        cpasfait_tools::print_stdout(&format!("{}\n", serde_json::to_string(&out)?));
        return Ok(true);
    }
    let mut line = if args.path { path.join(" › ") } else { t.title.clone() };
    if args.long {
        if t.priority > 0 {
            line.push_str(&format!(" !{}", t.priority));
        }
        if let Some(d) = t.due {
            line.push_str(&format!(" due:{d}"));
        }
        for tag in &t.tags {
            line.push_str(&format!(" #{tag}"));
        }
    }
    cpasfait_tools::print_stdout(&format!("{line}\n"));
    Ok(true)
}
