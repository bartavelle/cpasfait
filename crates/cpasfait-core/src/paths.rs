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

/// `<data dir>/identity.txt`: the age identity cpasfait creates when there is none.
pub fn own_identity_path() -> PathBuf {
    data_dir().join("identity.txt")
}

/// The identity (secret key) used to read shared tabs, first found of:
/// `$CPASFAIT_IDENTITY` (an age identity file or an unencrypted SSH private
/// key), `<data dir>/identity.txt`, and, unless `$CPASFAIT_DATA_DIR` is set,
/// age's usual `~/.config/age/keys.txt`.
pub fn identity_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CPASFAIT_IDENTITY") {
        return Some(PathBuf::from(p));
    }
    let own = own_identity_path();
    if own.exists() {
        return Some(own);
    }
    if std::env::var_os("CPASFAIT_DATA_DIR").is_none() {
        let age = directories::BaseDirs::new()?.config_dir().join("age").join("keys.txt");
        if age.exists() {
            return Some(age);
        }
    }
    None
}

/// The configuration file: `$CPASFAIT_CONFIG`, else `config.toml` in
/// `$CPASFAIT_DATA_DIR` when that is set (separate data dirs are separate
/// profiles), else in the platform config dir (`~/.config/cpasfait/config.toml`
/// on Linux).
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CPASFAIT_CONFIG") {
        return PathBuf::from(p);
    }
    if let Some(d) = std::env::var_os("CPASFAIT_DATA_DIR") {
        return PathBuf::from(d).join("config.toml");
    }
    directories::ProjectDirs::from("", "", "cpasfait")
        .map(|d| d.config_dir().join("config.toml"))
        .unwrap_or_else(|| PathBuf::from(".cpasfait/config.toml"))
}
