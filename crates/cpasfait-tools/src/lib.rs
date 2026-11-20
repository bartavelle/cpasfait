//! Shared helpers for the companion tools.

use anyhow::{Context, Result, bail};
use cpasfait_core::{Repo, TabInfo};
use std::path::PathBuf;

/// Open the database read-only (fast, never migrates or writes).
pub fn open_db(db: Option<PathBuf>) -> Result<Repo> {
    let path = db.unwrap_or_else(cpasfait_core::paths::default_db_path);
    Repo::open_readonly(&path).with_context(|| format!("opening {}", path.display()))
}

/// Resolve `--tab`, or the only tab when there is just one.
pub fn pick_tab(repo: &Repo, query: Option<&str>) -> Result<TabInfo> {
    if let Some(q) = query {
        return Ok(repo.find_tab(q)?);
    }
    let tabs = repo.tabs()?;
    match tabs.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no tabs"),
        many => bail!(
            "several tabs, choose one with --tab: {}",
            many.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Write to stdout, silently ignoring a closed pipe (`| head`).
pub fn print_stdout(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(s.as_bytes()).and_then(|()| out.flush());
}
