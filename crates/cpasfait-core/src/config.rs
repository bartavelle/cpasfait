//! The configuration file, `config.toml` (location: [`crate::paths::config_path`]).

use crate::error::{Error, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Recorded as the author of the tasks this user creates.
    pub email: Option<String>,
    /// Display name (else the one stored in the database, see `:user`).
    pub name: Option<String>,
    /// The secret key (age identity file or unencrypted SSH private key).
    /// `~/` is the home directory; a relative path is relative to the
    /// configuration file's directory. See [`crate::paths::identity_path`].
    pub identity: Option<PathBuf>,
    /// Desktop notifications when tasks fall due, from the TUI (default: on).
    pub notifications: Option<bool>,
    /// Where the file was read from (resolves relative paths).
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        let c: Config = toml::from_str(text).map_err(|e| Error::Invalid(e.to_string()))?;
        if let Some(e) = &c.email {
            check_email(e)?;
        }
        Ok(c)
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)?;
        let mut c = Self::parse(&text).map_err(|e| match e {
            Error::Invalid(m) => Error::Invalid(format!("in {}: {m}", path.display())),
            e => e,
        })?;
        c.path = Some(path.to_owned());
        Ok(c)
    }

    /// The file at [`crate::paths::config_path`], if there is one.
    pub fn load_default() -> Result<Option<Config>> {
        let path = crate::paths::config_path();
        if !path.exists() {
            return Ok(None);
        }
        Self::load(&path).map(Some)
    }

    /// The configured identity file, resolved.
    pub fn identity_path(&self) -> Option<PathBuf> {
        let p = self.identity.as_ref()?;
        if let Ok(rest) = p.strip_prefix("~")
            && let Some(home) = directories::BaseDirs::new()
        {
            return Some(home.home_dir().join(rest));
        }
        Some(match self.path.as_ref().and_then(|c| c.parent()) {
            Some(dir) if p.is_relative() => dir.join(p),
            _ => p.clone(),
        })
    }

    /// The file written on first run.
    pub fn template(email: &str) -> String {
        format!(
            "# cpasfait configuration\n\
             \n\
             # Recorded as the author of the tasks you create.\n\
             email = {}\n\
             \n\
             # Display name, shown to the people you share tabs with\n\
             # (default: your login name; :user NAME in the app changes it when this is unset).\n\
             # name = \"Your Name\"\n\
             \n\
             # Secret key for shared tabs: an age identity file or an SSH private key\n\
             # without passphrase (default: identity.txt in the data directory, else\n\
             # ~/.config/age/keys.txt, else a new key is created in the data directory).\n\
             # identity = \"~/.ssh/id_ed25519\"\n\
             \n\
             # Desktop notification (notify-send) when a task falls due, while the\n\
             # TUI runs (cpasfait-notify --watch does it without the TUI).\n\
             # notifications = false\n",
            toml::Value::String(email.to_owned())
        )
    }
}

pub fn check_email(e: &str) -> Result<()> {
    match e.split_once('@') {
        Some((user, host)) if !user.is_empty() && !host.is_empty() && !e.contains(char::is_whitespace) => Ok(()),
        _ => Err(Error::Invalid(format!("{e:?} is not an email address"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses() {
        let c = Config::parse(&Config::template("a\"b@example.org")).unwrap();
        assert_eq!(c.email.as_deref(), Some("a\"b@example.org"));
        assert_eq!((c.name, c.identity), (None, None));
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert!(Config::parse("emial = \"x@y\"").is_err());
        assert_eq!(
            Config::parse("notifications = false").unwrap().notifications,
            Some(false)
        );
        assert!(Config::parse("email = \"nope\"").is_err());
        let c = Config::parse("name = \"Alice\"\nemail = \"a@b.c\"").unwrap();
        assert_eq!(c.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn identity_paths() {
        let with = |id: &str| Config {
            identity: Some(id.into()),
            path: Some("/etc/cpf/config.toml".into()),
            ..Default::default()
        };
        assert_eq!(with("/k/id").identity_path(), Some("/k/id".into()));
        assert_eq!(with("keys/id").identity_path(), Some("/etc/cpf/keys/id".into()));
        let home = directories::BaseDirs::new().unwrap().home_dir().to_owned();
        assert_eq!(with("~/.ssh/id").identity_path(), Some(home.join(".ssh/id")));
        assert_eq!(Config::default().identity_path(), None);
    }
}
