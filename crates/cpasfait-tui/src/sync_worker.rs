//! Background sync: its own database connection, so the UI never blocks on I/O.

use cpasfait_core::Repo;
use cpasfait_core::sync::sync_all;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

pub struct SyncReport {
    pub pulled: usize,
    pub pushed: usize,
    /// Changes that need a key this device does not have yet.
    pub waiting_for_key: usize,
    pub errors: Vec<String>,
}

pub struct SyncWorker {
    trigger: Sender<()>,
    pub results: Receiver<SyncReport>,
}

impl SyncWorker {
    pub fn spawn(db_path: PathBuf, interval: Duration) -> SyncWorker {
        let (trigger, trigger_rx) = mpsc::channel::<()>();
        let (result_tx, results) = mpsc::channel();
        thread::spawn(move || {
            let mut repo = match Repo::open(&db_path) {
                Ok(r) => r,
                Err(e) => {
                    let _ = result_tx.send(SyncReport {
                        pulled: 0,
                        pushed: 0,
                        waiting_for_key: 0,
                        errors: vec![e.to_string()],
                    });
                    return;
                }
            };
            loop {
                if result_tx.send(run_once(&mut repo)).is_err() {
                    return;
                }
                match trigger_rx.recv_timeout(interval) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {
                        // Coalesce queued triggers.
                        while trigger_rx.try_recv().is_ok() {}
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        SyncWorker { trigger, results }
    }

    pub fn request(&self) {
        let _ = self.trigger.send(());
    }
}

pub fn run_once(repo: &mut Repo) -> SyncReport {
    let mut report = SyncReport {
        pulled: 0,
        pushed: 0,
        waiting_for_key: 0,
        errors: vec![],
    };
    let names: std::collections::HashMap<_, _> = repo
        .tabs()
        .unwrap_or_default()
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    match sync_all(repo) {
        Ok(results) => {
            for (tab, r) in results {
                match r {
                    Ok(s) => {
                        report.pulled += s.pulled;
                        report.pushed += s.pushed;
                        report.waiting_for_key += s.waiting_for_key;
                    }
                    Err(e) => {
                        let name = names.get(&tab).map_or_else(|| tab.to_string(), |n| format!("{n:?}"));
                        report.errors.push(format!("tab {name}: {e:#}"));
                    }
                }
            }
        }
        Err(e) => report.errors.push(e.to_string()),
    }
    report
}
