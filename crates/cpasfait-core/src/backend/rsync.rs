//! rsync over SSH: the remote is a directory on any machine reachable with
//! `ssh` and having `rsync` installed.
//!
//! Files on a remote are immutable and uniquely named, so a "copy what the
//! other side is missing, never delete" rsync in each direction is a correct
//! merge. This backend keeps a local mirror of the remote directory:
//!
//! - `list(prefix)` pulls the tab's directory into the mirror, then lists it;
//! - `get` reads the mirror;
//! - `put_new` writes into the mirror, then pushes that one file
//!   (`put_new_many`: many files, with one rsync run per batch).
//!
//! The system's `rsync` and `ssh` are used, so `~/.ssh/config`, ssh-agent,
//! hardware keys and `known_hosts` apply. ssh runs in batch mode (sync runs
//! in the background, it cannot prompt): use ssh-agent or a key without a
//! passphrase.
//!
//! Only `rsync --server` commands are run on the remote side, except when
//! the remote directory's parent is missing (then `mkdir -p` first), so a key
//! restricted with rrsync (`command="rrsync /srv/cpasfait",restrict` in
//! `authorized_keys`) works; the remote path is then relative to that root
//! (`rsync:host:` for the root itself). rrsync allows one transfer at a
//! time per directory and refuses the others at once: they are retried a
//! few times, and `rrsync -no-lock` avoids the problem altogether (safe,
//! as remote files are never changed).

use super::RemoteBackend;
use super::localdir::LocalDir;
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default `-e` command for rsync.
pub const SSH: &str = "ssh -o BatchMode=yes -o ConnectTimeout=20";

/// A directory listed twice within this delay is not pulled again (a sync
/// lists `<tab>/keys` then `<tab>/ops`: one pull of `<tab>` serves both).
const PULL_REUSE: Duration = Duration::from_secs(2);

/// What rrsync says when another transfer holds its lock on the directory
/// (it does not wait: it gives up at once).
const RRSYNC_BUSY: &str = "Another instance of rrsync is already accessing this directory";

/// Tries after the first one when rrsync is busy, waiting twice as long
/// each time from [`BUSY_FIRST_DELAY`] (about 8 s in all).
const BUSY_RETRIES: u32 = 5;
const BUSY_FIRST_DELAY: Duration = Duration::from_millis(if cfg!(test) { 5 } else { 250 });

/// Files pushed per rsync run by `put_new_many` (each run is an ssh
/// connection; batches let progress be shown).
const PUSH_BATCH: usize = 100;

pub struct Rsync {
    /// rsync location of the remote root: `host:path`, `user@host:path`, or a local path.
    remote: String,
    mirror: LocalDir,
    mirror_root: PathBuf,
    ssh: String,
    /// Top-level directories recently pulled.
    pulled: Mutex<Vec<(String, Instant)>>,
}

impl Rsync {
    pub fn new(remote: &str, mirror: impl Into<PathBuf>) -> Rsync {
        let mirror_root = mirror.into();
        // `host:~/x` is `host:x` (relative to the home directory), and the
        // `~` would not survive quoting.
        let remote = match remote.split_once(":~/") {
            Some((host, path)) if !host.contains('/') => format!("{host}:{path}"),
            _ => remote.to_owned(),
        };
        let mut remote = remote.trim_end_matches('/').to_owned();
        // `host:` (the login or rrsync root directory): `host:/x` would be absolute.
        if remote.ends_with(':') {
            remote.push('.');
        }
        Rsync {
            remote,
            mirror: LocalDir::new(&mirror_root),
            mirror_root,
            ssh: SSH.to_owned(),
            pulled: Mutex::new(vec![]),
        }
    }

    /// Use another remote shell than [`SSH`] (rsync's `-e`).
    pub fn with_ssh(mut self, ssh: &str) -> Rsync {
        self.ssh = ssh.to_owned();
        self
    }

    /// The default mirror directory for a remote: `<data dir>/mirrors/<hash>`.
    pub fn default_mirror(remote: &str) -> PathBuf {
        use sha2::Digest;
        let h = sha2::Sha256::digest(remote.as_bytes());
        crate::paths::data_dir()
            .join("mirrors")
            .join(crate::crypto::hex(&h[..8]))
    }

    fn remote_path(&self, rel: &str) -> String {
        if rel.is_empty() {
            format!("{}/", self.remote)
        } else {
            format!("{}/{rel}", self.remote)
        }
    }

    /// Run rsync; when rrsync refuses because another transfer holds its
    /// lock, try again a few times (see [`BUSY_RETRIES`]).
    fn rsync(&self, args: &[&str], cwd: Option<&Path>) -> Result<()> {
        let mut delay = BUSY_FIRST_DELAY;
        let mut attempt = 0;
        loop {
            match self.rsync_once(args, cwd) {
                Err(Error::Backend(m)) if m.contains(RRSYNC_BUSY) => {
                    if attempt == BUSY_RETRIES {
                        return Err(Error::Backend(format!(
                            "{m} (still busy after {} tries; on the server, rrsync's -no-lock option avoids this: \
                             cpasfait never changes or deletes remote files, so concurrent transfers are safe)",
                            attempt + 1
                        )));
                    }
                    attempt += 1;
                    // Some jitter, so that two clients waiting for each other don't retry in step.
                    let jitter = BUSY_FIRST_DELAY * u32::from(uuid::Uuid::new_v4().as_bytes()[0]) / 128;
                    std::thread::sleep(delay + jitter);
                    delay *= 2;
                }
                other => return other,
            }
        }
    }

    fn rsync_once(&self, args: &[&str], cwd: Option<&Path>) -> Result<()> {
        let mut cmd = Command::new("rsync");
        // Dot files: rsync's temporary files of a concurrent upload.
        cmd.args([
            "-r",
            "--ignore-existing",
            "--exclude=.*",
            "--timeout=60",
            "-e",
            &self.ssh,
        ])
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null());
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        let out = cmd.output().map_err(|e| {
            Error::Backend(if e.kind() == std::io::ErrorKind::NotFound {
                "rsync is not installed".into()
            } else {
                format!("running rsync: {e}")
            })
        })?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg: Vec<&str> = stderr
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with("rsync error:"))
            .take(3)
            .collect();
        let msg = if msg.is_empty() {
            stderr.trim().to_owned()
        } else {
            msg.join(" / ")
        };
        Err(Error::Backend(format!("rsync failed ({}): {msg}", out.status)))
    }

    /// Bring the remote directory `dir` (top-level, or "" for everything)
    /// into the mirror, unless that was just done.
    fn pull(&self, dir: &str) -> Result<()> {
        {
            let pulled = self.pulled.lock().expect("lock");
            if pulled
                .iter()
                .any(|(d, t)| (d.is_empty() || d == dir) && t.elapsed() < PULL_REUSE)
            {
                return Ok(());
            }
        }
        let local = self.mirror_root.join(dir);
        std::fs::create_dir_all(&local)?;
        let src = format!("{}/", self.remote_path(dir).trim_end_matches('/'));
        let dst = format!("{}/", local.display());
        match self.rsync(&[&src, &dst], None) {
            Ok(()) => {}
            // Nothing there yet.
            Err(Error::Backend(m)) if m.contains("No such file or directory") => {}
            Err(e) => return Err(e),
        }
        self.pulled.lock().expect("lock").push((dir.to_owned(), Instant::now()));
        Ok(())
    }

    /// `(host, path)` when the remote is on another machine (rsync's rule: a
    /// colon before any slash).
    fn split_remote(&self) -> Option<(&str, &str)> {
        let (host, path) = self.remote.split_once(':')?;
        (!host.contains('/')).then_some((host, path))
    }

    /// Upload mirror files (creating their directories on the remote), in
    /// one rsync run.
    fn push(&self, rels: &[&str]) -> Result<()> {
        // The list of files, relative to the mirror (`--files-from` implies
        // `-R`: their path is recreated under the destination). rsync
        // creates the destination itself, but not its missing parents.
        let list = std::env::temp_dir().join(format!("cpasfait-push-{}", uuid::Uuid::now_v7()));
        std::fs::write(&list, rels.join("\n") + "\n")?;
        let files_from = format!("--files-from={}", list.display());
        let res = self.push_list(&files_from);
        let _ = std::fs::remove_file(&list);
        res
    }

    fn push_list(&self, files_from: &str) -> Result<()> {
        let dst = self.remote_path("");
        let Some((_, path)) = self.split_remote() else {
            std::fs::create_dir_all(&self.remote)?;
            return self.rsync(&[files_from, ".", &dst], Some(&self.mirror_root));
        };
        match self.rsync(&[files_from, ".", &dst], Some(&self.mirror_root)) {
            Err(Error::Backend(m)) if m.contains("No such file or directory") => {
                // Not possible through rrsync, which only runs rsync.
                let mkdir = format!("--rsync-path=mkdir -p '{}' && rsync", path.replace('\'', "'\\''"));
                self.rsync(&[&mkdir, files_from, ".", &dst], Some(&self.mirror_root))
                    .map_err(|e| match e {
                        Error::Backend(m2) => Error::Backend(format!("{m} (then, creating the directory: {m2})")),
                        e => e,
                    })
            }
            other => other,
        }
    }
}

fn top_dir(path: &str) -> &str {
    path.split('/').find(|s| !s.is_empty()).unwrap_or("")
}

impl RemoteBackend for Rsync {
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.pull(top_dir(prefix))?;
        self.mirror.list(prefix)
    }

    fn get(&self, path: &str) -> Result<Vec<u8>> {
        match self.mirror.get(path) {
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                self.pull(top_dir(path))?;
                self.mirror.get(path)
            }
            other => other,
        }
    }

    fn put_new(&self, path: &str, data: &[u8]) -> Result<()> {
        match self.mirror.put_new(path, data) {
            Ok(()) => {
                if let Err(e) = self.push(&[path]) {
                    // Not on the remote: don't let the mirror pretend it is.
                    let _ = std::fs::remove_file(self.mirror_root.join(path));
                    return Err(e);
                }
                Ok(())
            }
            Err(Error::AlreadyExists(p)) => {
                // Maybe written by an earlier, interrupted run: make sure the
                // remote has it.
                self.push(&[path])?;
                Err(Error::AlreadyExists(p))
            }
            Err(e) => Err(e),
        }
    }

    fn put_new_many(&self, files: &[(String, Vec<u8>)], progress: &mut dyn FnMut(usize)) -> Result<usize> {
        let (mut created, mut done) = (0, 0);
        for batch in files.chunks(PUSH_BATCH) {
            let mut new = vec![];
            for (path, data) in batch {
                match self.mirror.put_new(path, data) {
                    Ok(()) => new.push(path),
                    // Pushed again below, in case an earlier run was interrupted.
                    Err(Error::AlreadyExists(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            let paths: Vec<&str> = batch.iter().map(|(p, _)| p.as_str()).collect();
            if let Err(e) = self.push(&paths) {
                // Not on the remote: don't let the mirror pretend they are.
                for p in new {
                    let _ = std::fs::remove_file(self.mirror_root.join(p));
                }
                return Err(e);
            }
            created += new.len();
            done += batch.len();
            progress(done);
        }
        Ok(created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn have_rsync() -> bool {
        Command::new("rsync").arg("--version").output().is_ok()
    }

    #[test]
    fn remote_paths() {
        let m = tempfile::tempdir().unwrap();
        let r = |s: &str| Rsync::new(s, m.path()).remote;
        assert_eq!(r("me@box:~/cpasfait/"), "me@box:cpasfait");
        assert_eq!(r("box:/srv/x"), "box:/srv/x");
        assert_eq!(r("/local/a:~/b"), "/local/a:~/b");
        assert_eq!(r("box:"), "box:.");
        assert_eq!(r("box:~/"), "box:.");
        assert_eq!(Rsync::new("box:", m.path()).remote_path("t"), "box:./t");
        let b = Rsync::new("box:x", m.path());
        assert_eq!(b.split_remote(), Some(("box", "x")));
        assert_eq!(Rsync::new("/srv/a:b", m.path()).split_remote(), None);
    }

    /// A remote shell that answers like a busy rrsync the first `busy`
    /// times, then runs the command locally (the "host" is ignored).
    fn fake_ssh(dir: &Path, busy: u32) -> String {
        let script = dir.join("ssh");
        let count = dir.join("count");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nn=$(cat '{c}' 2>/dev/null || echo 0)\necho $((n+1)) > '{c}'\n\
                 if [ \"$n\" -lt {busy} ]; then\n  echo '/usr/bin/rrsync error: {RRSYNC_BUSY}.' >&2\n  exit 1\nfi\n\
                 shift\nexec \"$@\"\n",
                c = count.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script.display().to_string()
    }

    #[test]
    fn busy_rrsync_is_retried() {
        if !have_rsync() {
            eprintln!("rsync not installed: skipped");
            return;
        }
        let (remote, mirror, bin) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let target = format!("fakehost:{}", remote.path().join("root").display());
        std::fs::create_dir_all(remote.path().join("root")).unwrap();
        let b = Rsync::new(&target, mirror.path()).with_ssh(&fake_ssh(bin.path(), 2));
        b.put_new("t/ops/d/1.seg", b"hello").unwrap();
        assert_eq!(
            std::fs::read(remote.path().join("root/t/ops/d/1.seg")).unwrap(),
            b"hello"
        );
        assert_eq!(std::fs::read_to_string(bin.path().join("count")).unwrap().trim(), "3");

        // Busy for too long: an error that says what to do.
        let (mirror, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let b = Rsync::new(&target, mirror.path()).with_ssh(&fake_ssh(bin.path(), 100));
        let e = b.list("t/ops").unwrap_err().to_string();
        assert!(e.contains("-no-lock"), "{e}");
    }

    #[test]
    fn put_list_get_local_target() {
        if !have_rsync() {
            eprintln!("rsync not installed: skipped");
            return;
        }
        let (remote, m1, m2) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let root = remote.path().join("root");
        let b1 = Rsync::new(root.to_str().unwrap(), m1.path());
        let b2 = Rsync::new(root.to_str().unwrap(), m2.path());
        assert_eq!(b1.list("t/ops").unwrap(), Vec::<String>::new());
        b1.put_new("t/ops/d/1.seg", b"hello").unwrap();
        assert_eq!(std::fs::read(root.join("t/ops/d/1.seg")).unwrap(), b"hello");
        assert!(matches!(
            b1.put_new("t/ops/d/1.seg", b"again"),
            Err(Error::AlreadyExists(_))
        ));
        assert_eq!(b2.list("t/ops").unwrap(), vec!["t/ops/d/1.seg"]);
        assert_eq!(b2.get("t/ops/d/1.seg").unwrap(), b"hello");
        assert_eq!(b2.list("").unwrap(), vec!["t/ops/d/1.seg"]);
        // a failed push leaves nothing behind in the mirror
        let m3 = tempfile::tempdir().unwrap();
        let bad = Rsync::new("/nonexistent/dir/root", m3.path());
        assert!(bad.put_new("t/x", b"x").is_err());
        assert!(bad.mirror.list("t").unwrap().is_empty());
    }
}
