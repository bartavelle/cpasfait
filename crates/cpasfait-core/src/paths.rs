//! Default locations.

use std::path::PathBuf;

/// `$CPASFAIT_DATA_DIR`, else the platform data dir (`~/.local/share/cpasfait` on Linux).
pub fn data_dir() -> PathBuf {
    std::env::var_os("CPASFAIT_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::ProjectDirs::from("", "", "cpasfait")
                .map(|d| d.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(".cpasfait"))
        })
}

/// `<data dir>/cpasfait.db`.
pub fn default_db_path() -> PathBuf {
    data_dir().join("cpasfait.db")
}

/// `<data dir>/gdrive.json`: Google OAuth client and tokens (secret, mode 0600).
pub fn gdrive_credentials_path() -> PathBuf {
    data_dir().join("gdrive.json")
}
