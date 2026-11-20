//! Google Drive backend.
//!
//! A remote is a Drive folder (by id), shared between users with the normal
//! Drive sharing UI (or `cpasfait-gdrive share`). Remote paths map to nested
//! folders under it.
//!
//! Drive identifies files by id and allows several children with the same
//! name, so two users creating `<tab>/ops` at the same time end up with two
//! `ops` folders. Rather than trying to prevent that (Drive has no
//! create-if-absent), every read treats same-named folders as one directory:
//! a path resolves to the *set* of matching folders and listings are unions.
//! Writes go to the matching folder with the smallest id, so all clients tend
//! to converge on one. File names are unique per device by construction; a
//! duplicate file (a retried upload whose first response was lost) has the
//! same content and is harmless.
//!
//! The HTTP layer is behind [`DriveApi`] so the path logic is tested against
//! an in-memory fake ([`mem::MemDrive`]).

pub mod auth;
pub mod http;
pub mod mem;

use super::RemoteBackend;
use crate::error::{Error, Result};
use std::collections::BTreeSet;

pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub folder: bool,
}

/// The few Drive operations the backend needs.
pub trait DriveApi: Send + Sync {
    /// Non-trashed children of a folder (all pages).
    fn children(&self, folder: &str) -> Result<Vec<Entry>>;
    fn create_folder(&self, parent: &str, name: &str) -> Result<String>;
    /// Create a file in one request: it is never visible partially written.
    fn upload(&self, parent: &str, name: &str, data: &[u8]) -> Result<String>;
    fn download(&self, id: &str) -> Result<Vec<u8>>;
}

pub struct GDrive<A> {
    api: A,
    root: String,
}

fn parts(path: &str) -> Result<Vec<&str>> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.iter().any(|p| *p == "." || *p == "..") {
        return Err(Error::Invalid(format!("bad remote path {path:?}")));
    }
    Ok(parts)
}

impl<A: DriveApi> GDrive<A> {
    pub fn new(api: A, root: impl Into<String>) -> Self {
        Self { api, root: root.into() }
    }

    pub fn api(&self) -> &A {
        &self.api
    }

    fn sorted_children(&self, folder: &str) -> Result<Vec<Entry>> {
        let mut c = self.api.children(folder)?;
        c.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(c)
    }

    /// All folders matching `parts` (the union of same-named duplicates), smallest id first.
    fn resolve_dirs(&self, parts: &[&str]) -> Result<Vec<String>> {
        let mut cur = vec![self.root.clone()];
        for part in parts {
            cur = self.resolve_dirs_from(&cur, part)?;
            if cur.is_empty() {
                break;
            }
        }
        Ok(cur)
    }

    /// Sub-folders named `name` of any of `dirs`, smallest id first.
    fn resolve_dirs_from(&self, dirs: &[String], name: &str) -> Result<Vec<String>> {
        let mut next = vec![];
        for dir in dirs {
            next.extend(
                self.api
                    .children(dir)?
                    .into_iter()
                    .filter(|e| e.folder && e.name == name)
                    .map(|e| e.id),
            );
        }
        next.sort();
        Ok(next)
    }

    /// The folder to write into for `parts`, creating missing levels. Looks
    /// through all same-named duplicates before creating anything.
    fn ensure_dir(&self, parts: &[&str]) -> Result<String> {
        let mut cur = vec![self.root.clone()];
        for (i, part) in parts.iter().enumerate() {
            let next = self.resolve_dirs_from(&cur, part)?;
            if next.is_empty() {
                // Create the rest of the chain under the preferred folder.
                let mut id = cur[0].clone();
                for p in &parts[i..] {
                    id = self.api.create_folder(&id, p)?;
                }
                return Ok(id);
            }
            cur = next;
        }
        Ok(cur.swap_remove(0))
    }

    fn files_in(&self, dirs: &[String], name: &str) -> Result<Vec<Entry>> {
        let mut out = vec![];
        for d in dirs {
            out.extend(
                self.sorted_children(d)?
                    .into_iter()
                    .filter(|e| !e.folder && e.name == name),
            );
        }
        Ok(out)
    }
}

impl<A: DriveApi> RemoteBackend for GDrive<A> {
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let parts = parts(prefix)?;
        let mut out = BTreeSet::new();
        // (folders sharing one logical path, that path)
        let mut stack = vec![(self.resolve_dirs(&parts)?, parts.join("/"))];
        while let Some((dirs, rel)) = stack.pop() {
            let mut subdirs: std::collections::BTreeMap<String, Vec<String>> = Default::default();
            for d in &dirs {
                for e in self.api.children(d)? {
                    if e.name.starts_with('.') {
                        continue;
                    }
                    let path = if rel.is_empty() {
                        e.name.clone()
                    } else {
                        format!("{rel}/{}", e.name)
                    };
                    if e.folder {
                        subdirs.entry(path).or_default().push(e.id);
                    } else {
                        out.insert(path);
                    }
                }
            }
            stack.extend(subdirs.into_iter().map(|(p, ids)| (ids, p)));
        }
        Ok(out.into_iter().collect())
    }

    fn get(&self, path: &str) -> Result<Vec<u8>> {
        let parts = parts(path)?;
        let (name, dir) = parts
            .split_last()
            .ok_or_else(|| Error::Invalid("empty remote path".into()))?;
        let dirs = self.resolve_dirs(dir)?;
        match self.files_in(&dirs, name)?.first() {
            Some(e) => self.api.download(&e.id),
            None => Err(Error::NotFound(path.to_owned())),
        }
    }

    fn put_new(&self, path: &str, data: &[u8]) -> Result<()> {
        let parts = parts(path)?;
        let (name, dir) = parts
            .split_last()
            .ok_or_else(|| Error::Invalid("empty remote path".into()))?;
        let dirs = self.resolve_dirs(dir)?;
        if !self.files_in(&dirs, name)?.is_empty() {
            return Err(Error::AlreadyExists(path.to_owned()));
        }
        let target = match dirs.into_iter().next() {
            Some(d) => d,
            None => self.ensure_dir(dir)?,
        };
        self.api.upload(&target, name, data)?;
        Ok(())
    }
}

/// Open the backend for a shared folder with the stored credentials.
pub fn open(folder: &str) -> Result<GDrive<http::HttpDrive>> {
    let auth = auth::Auth::load(&crate::paths::gdrive_credentials_path())?;
    Ok(GDrive::new(http::HttpDrive::new(Box::new(auth)), folder))
}

#[cfg(test)]
mod tests {
    use super::mem::MemDrive;
    use super::*;

    #[test]
    fn put_list_get() {
        let d = GDrive::new(MemDrive::new(), MemDrive::ROOT);
        d.put_new("t/ops/d/1.jsonl", b"hello").unwrap();
        d.put_new("t/ops/d/2.jsonl", b"world").unwrap();
        assert!(matches!(
            d.put_new("t/ops/d/1.jsonl", b"again"),
            Err(Error::AlreadyExists(_))
        ));
        assert_eq!(d.list("t/ops").unwrap(), vec!["t/ops/d/1.jsonl", "t/ops/d/2.jsonl"]);
        assert_eq!(d.list("").unwrap().len(), 2);
        assert_eq!(d.list("nothing").unwrap(), Vec::<String>::new());
        assert_eq!(d.get("t/ops/d/1.jsonl").unwrap(), b"hello");
        assert!(matches!(d.get("t/ops/d/3.jsonl"), Err(Error::NotFound(_))));
        assert!(d.get("../x").is_err());
        // folders were created once
        assert_eq!(d.api().folder_count(), 3);
    }

    #[test]
    fn duplicate_folders_are_merged() {
        let mem = MemDrive::new();
        // Two clients raced to create "t" and "t/ops".
        let t1 = mem.create_folder(MemDrive::ROOT, "t").unwrap();
        let t2 = mem.create_folder(MemDrive::ROOT, "t").unwrap();
        let o1 = mem.create_folder(&t1, "ops").unwrap();
        let o2 = mem.create_folder(&t2, "ops").unwrap();
        let a = mem.create_folder(&o1, "a").unwrap();
        let b = mem.create_folder(&o2, "b").unwrap();
        let a2 = mem.create_folder(&o2, "a").unwrap();
        mem.upload(&a, "1.jsonl", b"a1").unwrap();
        mem.upload(&b, "1.jsonl", b"b1").unwrap();
        mem.upload(&a2, "2.jsonl", b"a2").unwrap();
        let d = GDrive::new(mem, MemDrive::ROOT);
        assert_eq!(
            d.list("t/ops").unwrap(),
            vec!["t/ops/a/1.jsonl", "t/ops/a/2.jsonl", "t/ops/b/1.jsonl"]
        );
        assert_eq!(d.get("t/ops/a/2.jsonl").unwrap(), b"a2");
        assert!(matches!(
            d.put_new("t/ops/a/2.jsonl", b"x"),
            Err(Error::AlreadyExists(_))
        ));
        let before = d.api().folder_count();
        d.put_new("t/ops/b/2.jsonl", b"b2").unwrap();
        assert_eq!(d.api().folder_count(), before, "reuses an existing folder");
        assert_eq!(d.get("t/ops/b/2.jsonl").unwrap(), b"b2");
    }
}
