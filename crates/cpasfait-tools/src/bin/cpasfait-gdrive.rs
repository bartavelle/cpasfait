//! Google Drive account and shared-folder management for cpasfait.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use cpasfait_core::backend::RemoteBackend;
use cpasfait_core::backend::gdrive::{self, DriveApi, FOLDER_MIME, auth};
use cpasfait_core::paths::gdrive_credentials_path;
use std::path::PathBuf;

/// Connect cpasfait to Google Drive and manage shared folders.
///
/// Setup: in the Google Cloud console, enable the Drive API, create an OAuth
/// client of type "Desktop app" and download its JSON, then run
/// `cpasfait-gdrive login --client client_secret.json` (just `login` if this
/// build has a client built in, see `status`). In the TUI, attach a
/// tab with `:tabremote gdrive:FOLDER-ID` and join it elsewhere with
/// `:tabjoin gdrive:FOLDER-ID`.
#[derive(Parser)]
#[command(version)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Scope {
    /// Full Drive access: required to use folders shared by other accounts.
    Full,
    /// Only files created by cpasfait: enough if a single Google account uses the remote.
    File,
}

#[derive(Subcommand)]
enum Cmd {
    /// Authorize cpasfait (opens a browser).
    Login {
        /// OAuth client JSON downloaded from the Cloud console (default: the
        /// stored client, else the one built into this binary, if any).
        #[arg(long)]
        client: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "full")]
        scope: Scope,
        /// Only print the URL.
        #[arg(long)]
        no_browser: bool,
    },
    /// Revoke the authorization and delete the stored credentials.
    Logout,
    /// Show the connected account.
    Status,
    /// Create a folder to use as a remote (in My Drive, or in --parent).
    Mkdir {
        name: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Give other Google accounts write access to a folder.
    Share {
        /// Folder id, gdrive:ID or folder URL.
        folder: String,
        #[arg(required = true)]
        emails: Vec<String>,
        /// Don't send a notification e-mail.
        #[arg(long)]
        no_notify: bool,
    },
    /// List the files under a remote folder (debugging).
    Ls { folder: String },
}

fn folder_id(spec: &str) -> Result<String> {
    let spec = if spec.starts_with("gdrive:") || spec.starts_with("https://") {
        spec.to_owned()
    } else {
        format!("gdrive:{spec}")
    };
    match cpasfait_core::RemoteConfig::parse(&spec).map_err(anyhow::Error::msg)? {
        cpasfait_core::RemoteConfig::GDrive { folder } => Ok(folder),
        _ => bail!("not a Google Drive folder: {spec}"),
    }
}

fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn main() -> Result<()> {
    let args = Args::parse();
    let path = gdrive_credentials_path();
    match args.cmd {
        Cmd::Login {
            client,
            scope,
            no_browser,
        } => {
            let client = match client {
                Some(f) => {
                    auth::Client::from_json(&std::fs::read(&f).with_context(|| format!("reading {}", f.display()))?)?
                }
                None => match auth::Credentials::read(&path) {
                    Ok(c) => c.client,
                    Err(_) => auth::Client::embedded()
                        .context("no OAuth client in this build: pass --client client_secret.json")?,
                },
            };
            let scope = match scope {
                Scope::Full => auth::SCOPE_FULL,
                Scope::File => auth::SCOPE_FILE,
            };
            auth::login(&path, client, scope, &mut |url| {
                eprintln!("Open this URL to authorize cpasfait:\n\n  {url}\n\nWaiting for the authorization...");
                if !no_browser {
                    open_browser(url);
                }
            })?;
            let user = gdrive::open("root")?.api().about()?;
            println!("Connected as {} <{}>", user.display_name, user.email_address);
            println!("Credentials stored in {}", path.display());
        }
        Cmd::Logout => match auth::Credentials::read(&path) {
            Ok(c) => {
                if let Err(e) = auth::revoke(&c) {
                    eprintln!("warning: could not revoke the token: {e}");
                }
                std::fs::remove_file(&path)?;
                println!("Logged out.");
            }
            Err(_) => println!("Not logged in."),
        },
        Cmd::Status => {
            match auth::Client::embedded() {
                Some(c) => println!("Built-in client: {}", c.client_id),
                None => println!("Built-in client: none"),
            }
            let c = auth::Credentials::read(&path)?;
            let user = gdrive::open("root")?.api().about()?;
            println!("Account: {} <{}>", user.display_name, user.email_address);
            println!("Scope:   {}", c.scope);
            println!("Client:  {}", c.client.client_id);
            println!("File:    {}", path.display());
        }
        Cmd::Mkdir { name, parent } => {
            let parent = match parent {
                Some(p) => folder_id(&p)?,
                None => "root".to_owned(),
            };
            let d = gdrive::open(&parent)?;
            let id = d.api().create_folder(&parent, &name)?;
            println!("gdrive:{id}");
            eprintln!("https://drive.google.com/drive/folders/{id}");
        }
        Cmd::Share {
            folder,
            emails,
            no_notify,
        } => {
            let id = folder_id(&folder)?;
            let d = gdrive::open(&id)?;
            let meta = d.api().metadata(&id)?;
            if meta.mime_type != FOLDER_MIME {
                bail!("{} is not a folder", meta.name);
            }
            for e in &emails {
                d.api().share(&id, e, !no_notify)?;
                println!("{e} can now write to {:?}", meta.name);
            }
        }
        Cmd::Ls { folder } => {
            let d = gdrive::open(&folder_id(&folder)?)?;
            for f in d.list("")? {
                println!("{f}");
            }
        }
    }
    Ok(())
}
