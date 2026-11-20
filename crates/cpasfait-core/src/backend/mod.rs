//! Remote storage backends.
//!
//! The sync protocol only ever creates new, uniquely-named, immutable files,
//! so a backend needs no locking or compare-and-swap: listing, reading and
//! create-only writes are enough (local dir, Google Drive, WebDAV, S3, ...).

#[cfg(feature = "gdrive")]
pub mod gdrive;
pub mod localdir;

use crate::error::Result;
use crate::model::RemoteConfig;

pub trait RemoteBackend: Send {
    /// All file paths (relative, `/`-separated) under `prefix`, recursively.
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
    fn get(&self, path: &str) -> Result<Vec<u8>>;
    /// Create `path` with `data`. Must fail if it already exists, and must not
    /// expose a partially written file to readers.
    fn put_new(&self, path: &str, data: &[u8]) -> Result<()>;
}

pub fn open(config: &RemoteConfig) -> Result<Box<dyn RemoteBackend>> {
    match config {
        RemoteConfig::LocalDir { path } => Ok(Box::new(localdir::LocalDir::new(path))),
        #[cfg(feature = "gdrive")]
        RemoteConfig::GDrive { folder } => Ok(Box::new(gdrive::open(folder)?)),
        #[cfg(not(feature = "gdrive"))]
        RemoteConfig::GDrive { .. } => Err(crate::Error::Backend(
            "this build has no Google Drive support (cargo feature `gdrive`)".into(),
        )),
    }
}
