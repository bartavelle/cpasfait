//! Default locations.

use std::path::PathBuf;

/// `$CPASFAIT_DATA_DIR/cpasfait.db`, else the platform data dir
/// (`~/.local/share/cpasfait/cpasfait.db` on Linux).
pub fn default_db_path() -> PathBuf {
    let dir = std::env::var_os("CPASFAIT_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::ProjectDirs::from("", "", "cpasfait")
                .map(|d| d.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(".cpasfait"))
        });
    dir.join("cpasfait.db")
}
