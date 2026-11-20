//! In-memory [`DriveApi`] with Drive's quirks (ids, duplicate names allowed),
//! for tests.

use super::{DriveApi, Entry};
use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct Node {
    parent: String,
    name: String,
    data: Option<Vec<u8>>, // None = folder
}

/// Clones share the same storage, like two clients of one Drive.
#[derive(Clone, Default)]
pub struct MemDrive {
    inner: Arc<Mutex<(u64, BTreeMap<String, Node>)>>,
}

impl MemDrive {
    pub const ROOT: &str = "root";

    pub fn new() -> Self {
        Self::default()
    }

    pub fn folder_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .1
            .values()
            .filter(|n| n.data.is_none())
            .count()
    }

    fn insert(&self, parent: &str, name: &str, data: Option<Vec<u8>>) -> Result<String> {
        let mut g = self.inner.lock().unwrap();
        if parent != Self::ROOT && !g.1.get(parent).is_some_and(|n| n.data.is_none()) {
            return Err(Error::NotFound(format!("folder {parent}")));
        }
        g.0 += 1;
        // Not in creation order, like real Drive ids.
        let id = format!("id{:08x}", g.0.wrapping_mul(0x9e37_79b9) & 0xffff_ffff);
        g.1.insert(
            id.clone(),
            Node {
                parent: parent.to_owned(),
                name: name.to_owned(),
                data,
            },
        );
        Ok(id)
    }
}

impl DriveApi for MemDrive {
    fn children(&self, folder: &str) -> Result<Vec<Entry>> {
        let g = self.inner.lock().unwrap();
        Ok(g.1
            .iter()
            .filter(|(_, n)| n.parent == folder)
            .map(|(id, n)| Entry {
                id: id.clone(),
                name: n.name.clone(),
                folder: n.data.is_none(),
            })
            .collect())
    }

    fn create_folder(&self, parent: &str, name: &str) -> Result<String> {
        self.insert(parent, name, None)
    }

    fn upload(&self, parent: &str, name: &str, data: &[u8]) -> Result<String> {
        self.insert(parent, name, Some(data.to_vec()))
    }

    fn download(&self, id: &str) -> Result<Vec<u8>> {
        let g = self.inner.lock().unwrap();
        g.1.get(id)
            .and_then(|n| n.data.clone())
            .ok_or_else(|| Error::NotFound(id.to_owned()))
    }
}
