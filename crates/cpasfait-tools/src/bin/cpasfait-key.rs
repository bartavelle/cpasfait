//! Shows (creating it if needed) the public key other members use to invite
//! you to a shared tab (`:tabinvite KEY` in the TUI).

use anyhow::{Result, bail};
use clap::Parser;
use cpasfait_core::crypto::Identity;

/// Print your public key (creating your secret key if you have none).
///
/// Shared tabs are encrypted to their members' keys. The secret key is the
/// first found of: $CPASFAIT_IDENTITY (an age identity file, or an
/// unencrypted SSH private key), <data dir>/identity.txt, and
/// ~/.config/age/keys.txt. If none exists, one is created in the data dir.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Print the secret key's location instead.
    #[arg(long)]
    path: bool,
    /// Fail instead of creating a key.
    #[arg(long)]
    no_create: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (id, created) = if args.no_create {
        match Identity::load_default()? {
            Some(id) => (id, false),
            None => bail!("no key found"),
        }
    } else {
        Identity::load_or_create_default()?
    };
    let path = id.path().map(|p| p.display().to_string()).unwrap_or_default();
    if created {
        eprintln!("created a new key in {path} (keep it safe; back it up)");
    }
    if args.path {
        println!("{path}");
    } else {
        println!("{}", id.recipient());
    }
    Ok(())
}
