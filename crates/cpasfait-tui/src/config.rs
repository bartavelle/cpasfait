//! First run: create the configuration file (see [`cpasfait_core::config`]).

use anyhow::{Context, Result, bail};
pub use cpasfait_core::config::Config;
use cpasfait_core::config::check_email;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

/// Load the configuration; when there is no file and we are in a terminal,
/// ask for the email and create it. Without a terminal, run unconfigured.
pub fn load_or_create() -> Result<Config> {
    let path = cpasfait_core::paths::config_path();
    if path.exists() {
        return Ok(Config::load(&path)?);
    }
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        eprintln!(
            "cpasfait: no configuration file ({}): run in a terminal to create it",
            path.display()
        );
        return Ok(Config::default());
    }
    let mut stdin = std::io::stdin().lock();
    let email = ask_email(&mut stdin, &mut std::io::stdout())?;
    write_new(&path, &Config::template(&email))?;
    println!("Wrote {}", path.display());
    Ok(Config::load(&path)?)
}

/// Ask until a valid email is typed. End of input aborts.
fn ask_email(input: &mut impl BufRead, out: &mut impl Write) -> Result<String> {
    writeln!(
        out,
        "Welcome to cpasfait! Your email is recorded as the author of the tasks you create."
    )?;
    loop {
        write!(out, "Email: ")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            bail!("no email given");
        }
        let email = line.trim();
        match check_email(email) {
            Ok(()) => return Ok(email.to_owned()),
            Err(cpasfait_core::Error::Invalid(e)) => writeln!(out, "{e}, try again (Ctrl-D to quit)")?,
            Err(e) => return Err(e.into()),
        }
    }
}

fn write_new(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_until_valid() {
        let mut out = vec![];
        let email = ask_email(&mut &b"\nnope\n  me@example.org \n"[..], &mut out).unwrap();
        assert_eq!(email, "me@example.org");
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.matches("try again").count(), 2, "{out}");
        assert!(ask_email(&mut &b"bad\n"[..], &mut vec![]).is_err());
    }

    #[test]
    fn first_file_is_not_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/config.toml");
        write_new(&p, "email = \"a@b\"\n").unwrap();
        assert!(write_new(&p, "x").is_err());
        assert_eq!(Config::load(&p).unwrap().email.as_deref(), Some("a@b"));
    }
}
