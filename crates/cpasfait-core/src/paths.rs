//! Default locations.

use std::path::PathBuf;
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Use `dir` as the data dir from now on, in place of `$CPASFAIT_DATA_DIR`
/// and the platform default (for apps that are given their directory, as on
/// Android). Only the first call counts; returns whether it did.
pub fn set_data_dir(dir: PathBuf) -> bool {
    DATA_DIR.set(dir).is_ok()
}

/// The data dir chosen explicitly: [`set_data_dir`], else `$CPASFAIT_DATA_DIR`.
fn explicit_data_dir() -> Option<PathBuf> {
    DATA_DIR
        .get()
        .cloned()
        .or_else(|| std::env::var_os("CPASFAIT_DATA_DIR").map(PathBuf::from))
}

/// [`set_data_dir`], else `$CPASFAIT_DATA_DIR`, else the platform data dir
/// (`~/.local/share/cpasfait` on Linux).
pub fn data_dir() -> PathBuf {
    explicit_data_dir().unwrap_or_else(|| {
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
/// key), `identity` in the configuration file, `<data dir>/identity.txt`,
/// and, unless the data dir was chosen explicitly, age's usual
/// `~/.config/age/keys.txt`. The first two are used even if the file is
/// missing (loading it then fails rather than using another key).
pub fn identity_path() -> crate::Result<Option<PathBuf>> {
    if let Some(p) = std::env::var_os("CPASFAIT_IDENTITY") {
        return Ok(Some(PathBuf::from(p)));
    }
    if let Some(p) = crate::config::Config::load_default()?.and_then(|c| c.identity_path()) {
        return Ok(Some(p));
    }
    let own = own_identity_path();
    if own.exists() {
        return Ok(Some(own));
    }
    if explicit_data_dir().is_none()
        && let Some(base) = directories::BaseDirs::new()
    {
        let age = base.config_dir().join("age").join("keys.txt");
        if age.exists() {
            return Ok(Some(age));
        }
    }
    Ok(None)
}

/// The configuration file: `$CPASFAIT_CONFIG`, else `config.toml` in
/// the data dir when it was chosen explicitly (separate data dirs are separate
/// profiles), else in the platform config dir (`~/.config/cpasfait/config.toml`
/// on Linux).
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CPASFAIT_CONFIG") {
        return PathBuf::from(p);
    }
    if let Some(d) = explicit_data_dir() {
        return d.join("config.toml");
    }
    directories::ProjectDirs::from("", "", "cpasfait")
        .map(|d| d.config_dir().join("config.toml"))
        .unwrap_or_else(|| PathBuf::from(".cpasfait/config.toml"))
}
