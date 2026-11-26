use super::RemoteBackend;
use crate::error::{Error, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A plain directory. Works with any external file-sync tool.
pub struct LocalDir {
    root: PathBuf,
}

impl LocalDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let mut p = self.root.clone();
        for part in path.split('/').filter(|s| !s.is_empty()) {
            if part == ".." || part == "." || part.contains('\\') {
                return Err(Error::Invalid(format!("bad remote path {path:?}")));
            }
            p.push(part);
        }
        Ok(p)
    }
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for e in entries {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // temp files, sync-tool metadata
        }
        let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if e.file_type()?.is_dir() {
            walk(&e.path(), &r, out)?;
        } else {
            out.push(r);
        }
    }
    Ok(())
}

impl RemoteBackend for LocalDir {
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut out = vec![];
        walk(&self.resolve(prefix)?, prefix.trim_end_matches('/'), &mut out)?;
        out.sort();
        Ok(out)
    }

    fn get(&self, path: &str) -> Result<Vec<u8>> {
        Ok(fs::read(self.resolve(path)?)?)
    }

    fn put_new(&self, path: &str, data: &[u8]) -> Result<()> {
        let target = self.resolve(path)?;
        let dir = target.parent().ok_or_else(|| Error::Invalid("no parent".into()))?;
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::now_v7()));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        let res = publish(&tmp, &target, |a, b| fs::hard_link(a, b));
        let _ = fs::remove_file(&tmp);
        match res {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(Error::AlreadyExists(path.to_owned())),
            Err(e) => Err(e.into()),
        }
    }
}

/// Give the written file `tmp` its name `target`, unless that exists.
/// `link` (a hard link) fails if the target exists: an atomic create-only
/// publish. Where hard links are refused (Android app storage, FAT/exFAT
/// cards), check then rename: another writer could slip in between, but
/// names are unique per device, so only the same device could race itself.
fn publish(tmp: &Path, target: &Path, link: impl Fn(&Path, &Path) -> std::io::Result<()>) -> std::io::Result<()> {
    use std::io::ErrorKind;
    match link(tmp, target) {
        Err(e) if matches!(e.kind(), ErrorKind::PermissionDenied | ErrorKind::Unsupported) => {
            if target.exists() {
                return Err(ErrorKind::AlreadyExists.into());
            }
            fs::rename(tmp, target)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_list_get() {
        let dir = tempfile::tempdir().unwrap();
        let b = LocalDir::new(dir.path());
        b.put_new("t/ops/d/1.jsonl", b"hello").unwrap();
        assert!(b.put_new("t/ops/d/1.jsonl", b"again").is_err());
        assert_eq!(b.list("t/ops").unwrap(), vec!["t/ops/d/1.jsonl"]);
        assert_eq!(b.list("nothing").unwrap(), Vec::<String>::new());
        assert_eq!(b.get("t/ops/d/1.jsonl").unwrap(), b"hello");
        assert!(b.get("../x").is_err());
    }

    /// Where hard links are refused, files are still published, once.
    #[test]
    fn without_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp, target) = (dir.path().join(".tmp"), dir.path().join("f"));
        let refuse = |_: &Path, _: &Path| Err(std::io::ErrorKind::PermissionDenied.into());
        fs::write(&tmp, b"one").unwrap();
        publish(&tmp, &target, refuse).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"one");
        assert!(!tmp.exists());
        fs::write(&tmp, b"two").unwrap();
        let e = publish(&tmp, &target, refuse).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&target).unwrap(), b"one");
    }
}
