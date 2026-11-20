//! OAuth for Google Drive: the "installed app" loopback flow with PKCE, and
//! refresh-token handling.
//!
//! Users bring their own OAuth client (a "Desktop app" client from the Google
//! Cloud console), like rclone does: cpasfait has no verified Google app.
//! Credentials live in `<data dir>/gdrive.json` (mode 0600).

use super::http::{TokenSource, backend_err, check};
use crate::error::{Error, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Full Drive access: needed to see folders *other* users shared with you.
pub const SCOPE_FULL: &str = "https://www.googleapis.com/auth/drive";
/// Only files created/opened by this app: enough when a single account uses the remote.
pub const SCOPE_FILE: &str = "https://www.googleapis.com/auth/drive.file";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Client {
    pub client_id: String,
    pub client_secret: String,
}

impl Client {
    /// Parse the JSON downloaded from the Cloud console (`{"installed": {...}}`)
    /// or a plain `{"client_id", "client_secret"}` object.
    pub fn from_json(data: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        struct Wrapped {
            installed: Option<Client>,
            web: Option<Client>,
        }
        if let Ok(Wrapped { installed, web }) = serde_json::from_slice::<Wrapped>(data)
            && let Some(c) = installed.or(web)
        {
            return Ok(c);
        }
        serde_json::from_slice(data).map_err(|e| Error::Invalid(format!("not an OAuth client JSON file: {e}")))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    #[serde(flatten)]
    pub client: Client,
    pub scope: String,
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: String,
    /// Unix seconds.
    #[serde(default)]
    pub expires_at: i64,
}

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Credentials {
    pub fn read(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::Backend(format!(
                "not logged in to Google Drive ({} missing): run `cpasfait-gdrive login`",
                path.display()
            )),
            _ => e.into(),
        })?;
        Ok(serde_json::from_slice(&data)?)
    }

    /// Atomic write, readable by the owner only.
    pub fn write(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".gdrive-{}.tmp", uuid::Uuid::now_v7().simple()));
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            let mut f = opts.open(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(self)?)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

fn token_request(agent: &ureq::Agent, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let resp = agent
        .post(TOKEN_URL)
        .send_form(form.iter().copied())
        .map_err(backend_err)?;
    let resp = check(resp, "token request").map_err(|e| match e {
        Error::Backend(m) if m.contains("invalid_grant") || m.contains("expired or revoked") => Error::Backend(
            format!("Google Drive authorization expired or was revoked: run `cpasfait-gdrive login` ({m})"),
        ),
        e => e,
    })?;
    resp.into_body().read_json().map_err(backend_err)
}

/// Credentials from a file, refreshed (and written back) when they expire.
/// Several processes may refresh concurrently: each token stays valid, the
/// last write wins.
pub struct Auth {
    path: PathBuf,
    creds: Mutex<Credentials>,
}

impl Auth {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_owned(),
            creds: Mutex::new(Credentials::read(path)?),
        })
    }
}

impl TokenSource for Auth {
    fn token(&self, agent: &ureq::Agent, refresh: bool) -> Result<String> {
        let mut c = self.creds.lock().unwrap_or_else(|e| e.into_inner());
        if !refresh && !c.access_token.is_empty() && c.expires_at > now_s() + 60 {
            return Ok(c.access_token.clone());
        }
        // Another process may have refreshed already.
        if !refresh
            && let Ok(disk) = Credentials::read(&self.path)
            && disk.expires_at > now_s() + 60
            && disk.client == c.client
        {
            *c = disk;
            return Ok(c.access_token.clone());
        }
        let t = token_request(
            agent,
            &[
                ("client_id", &c.client.client_id),
                ("client_secret", &c.client.client_secret),
                ("refresh_token", &c.refresh_token),
                ("grant_type", "refresh_token"),
            ],
        )?;
        c.access_token = t.access_token;
        c.expires_at = now_s() + t.expires_in.unwrap_or(3600);
        if let Some(r) = t.refresh_token {
            c.refresh_token = r;
        }
        // Failing to cache the token is not fatal.
        let _ = c.write(&self.path);
        Ok(c.access_token.clone())
    }
}

/// Revoke the refresh token (and the access tokens derived from it).
pub fn revoke(creds: &Credentials) -> Result<()> {
    let resp = super::http::agent()
        .post("https://oauth2.googleapis.com/revoke")
        .send_form([("token", creds.refresh_token.as_str())])
        .map_err(backend_err)?;
    check(resp, "revoke")?;
    Ok(())
}

/// Percent-encode for a URL query component.
pub(crate) fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub(crate) fn pct_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => match (b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c))) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| pct_decode(v))
    })
}

fn pkce_pair() -> (String, String) {
    // 64 chars from [0-9a-f]: within the 43..128 allowed unreserved characters.
    let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Interactive login: `show_url` must get the user to open the URL in a
/// browser; we wait for Google's redirect on a loopback port, exchange the
/// code and store the credentials at `path`.
pub fn login(path: &Path, client: Client, scope: &str, show_url: &mut dyn FnMut(&str)) -> Result<Credentials> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let (verifier, challenge) = pkce_pair();
    let state = uuid::Uuid::new_v4().simple().to_string();
    let url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt=consent",
        pct_encode(&client.client_id),
        pct_encode(&redirect),
        pct_encode(scope),
    );
    show_url(&url);

    let code = loop {
        let (stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        let reply = |reader: BufReader<std::net::TcpStream>, msg: &str| {
            let mut s = reader.into_inner();
            let body = format!("<!doctype html><meta charset=utf-8><title>cpasfait</title><p>{msg}</p>");
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        };
        if query_param(query, "state").as_deref() != Some(state.as_str()) {
            reply(reader, "Unexpected request.");
            continue; // favicon, stray requests
        }
        if let Some(err) = query_param(query, "error") {
            reply(reader, "Authorization failed, you can close this page.");
            return Err(Error::Backend(format!("Google authorization failed: {err}")));
        }
        match query_param(query, "code") {
            Some(code) => {
                reply(
                    reader,
                    "cpasfait is now connected to Google Drive. You can close this page.",
                );
                break code;
            }
            None => reply(reader, "Missing code."),
        }
    };

    let agent = super::http::agent();
    let t = token_request(
        &agent,
        &[
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("code", &code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
            ("grant_type", "authorization_code"),
        ],
    )?;
    let refresh_token = t
        .refresh_token
        .ok_or_else(|| Error::Backend("Google returned no refresh token".into()))?;
    let creds = Credentials {
        client,
        scope: t.scope.unwrap_or_else(|| scope.to_owned()),
        refresh_token,
        access_token: t.access_token,
        expires_at: now_s() + t.expires_in.unwrap_or(3600),
    };
    creds.write(path)?;
    Ok(creds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_coding() {
        assert_eq!(pct_encode("http://127.0.0.1:80 x"), "http%3A%2F%2F127.0.0.1%3A80%20x");
        assert_eq!(pct_decode("4%2F0Ab_x+y%"), "4/0Ab_x y%");
        assert_eq!(
            query_param("state=s&code=4%2Fabc&scope=x", "code").as_deref(),
            Some("4/abc")
        );
    }

    #[test]
    fn client_json() {
        let c = Client::from_json(
            br#"{"installed":{"client_id":"id","client_secret":"s","redirect_uris":["http://localhost"]}}"#,
        )
        .unwrap();
        assert_eq!(c.client_id, "id");
        let c = Client::from_json(br#"{"client_id":"a","client_secret":"b"}"#).unwrap();
        assert_eq!(c.client_secret, "b");
        assert!(Client::from_json(b"{}").is_err());
    }

    #[test]
    fn pkce() {
        let (v, c) = pkce_pair();
        assert_eq!(v.len(), 64);
        assert_eq!(c.len(), 43);
    }

    #[test]
    fn credentials_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gdrive.json");
        let c = Credentials {
            client: Client {
                client_id: "id".into(),
                client_secret: "s".into(),
            },
            scope: SCOPE_FULL.into(),
            refresh_token: "r".into(),
            access_token: "a".into(),
            expires_at: 5,
        };
        c.write(&p).unwrap();
        assert_eq!(Credentials::read(&p).unwrap(), c);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
