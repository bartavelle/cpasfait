//! Drive REST v3 over HTTPS (blocking, `ureq`).

use super::{DriveApi, Entry, FOLDER_MIME};
use crate::error::{Error, Result};
use serde::Deserialize;
use std::time::Duration;
pub use ureq::Agent;

/// Supplies OAuth access tokens.
pub trait TokenSource: Send + Sync {
    /// A valid access token; `refresh` forces a new one (after a 401).
    fn token(&self, agent: &ureq::Agent, refresh: bool) -> Result<String>;
}

/// A fixed token (tests, or a token obtained elsewhere).
pub struct StaticToken(pub String);

impl TokenSource for StaticToken {
    fn token(&self, _: &ureq::Agent, _: bool) -> Result<String> {
        Ok(self.0.clone())
    }
}

#[derive(Clone, Debug)]
pub struct Endpoints {
    /// `https://www.googleapis.com`
    pub api: String,
    /// Usually the same host as `api`.
    pub upload: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: "https://www.googleapis.com".into(),
            upload: "https://www.googleapis.com".into(),
        }
    }
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(concat!("cpasfait/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub struct HttpDrive {
    agent: ureq::Agent,
    tokens: Box<dyn TokenSource>,
    endpoints: Endpoints,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<FileMeta>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub email_address: String,
}

type Response = ureq::http::Response<ureq::Body>;

pub(crate) fn backend_err(e: impl std::fmt::Display) -> Error {
    Error::Backend(format!("Google Drive: {e}"))
}

/// Turn a non-2xx response into an error carrying Google's message.
pub(crate) fn check(resp: Response, what: &str) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.into_body().read_to_string().unwrap_or_default();
    let msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            let e = v.get("error")?;
            // API errors: {"error": {"message": ..}}; OAuth: {"error": "invalid_grant", "error_description": ..}
            e.get("message")
                .or_else(|| v.get("error_description"))
                .or(Some(e))
                .and_then(|m| m.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| body.chars().take(200).collect());
    Err(match status.as_u16() {
        404 => Error::NotFound(format!("{what} (not found, or not shared with this account)")),
        code => backend_err(format!("{what}: HTTP {code}: {msg}")),
    })
}

/// Escape a value for a Drive query string literal.
fn q_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

impl HttpDrive {
    pub fn new(tokens: Box<dyn TokenSource>) -> Self {
        Self::with_endpoints(tokens, Endpoints::default())
    }

    pub fn with_endpoints(tokens: Box<dyn TokenSource>, endpoints: Endpoints) -> Self {
        Self {
            agent: agent(),
            tokens,
            endpoints,
        }
    }

    /// Run a request with a bearer token, refreshing it once on 401.
    fn send(
        &self,
        what: &str,
        req: impl Fn(&ureq::Agent, &str) -> std::result::Result<Response, ureq::Error>,
    ) -> Result<Response> {
        let mut refresh = false;
        loop {
            let token = self.tokens.token(&self.agent, refresh)?;
            let resp = req(&self.agent, &format!("Bearer {token}")).map_err(backend_err)?;
            if resp.status().as_u16() == 401 && !refresh {
                refresh = true;
                continue;
            }
            return check(resp, what);
        }
    }

    pub fn metadata(&self, id: &str) -> Result<FileMeta> {
        let url = format!("{}/drive/v3/files/{id}", self.endpoints.api);
        self.send("get folder", |a, auth| {
            a.get(&url)
                .header("Authorization", auth)
                .query("fields", "id,name,mimeType")
                .query("supportsAllDrives", "true")
                .call()
        })?
        .into_body()
        .read_json()
        .map_err(backend_err)
    }

    pub fn about(&self) -> Result<User> {
        #[derive(Deserialize)]
        struct About {
            user: User,
        }
        let url = format!("{}/drive/v3/about", self.endpoints.api);
        let about: About = self
            .send("get account", |a, auth| {
                a.get(&url).header("Authorization", auth).query("fields", "user").call()
            })?
            .into_body()
            .read_json()
            .map_err(backend_err)?;
        Ok(about.user)
    }

    /// Give `email` write access to a file or folder.
    pub fn share(&self, id: &str, email: &str, notify: bool) -> Result<()> {
        let url = format!("{}/drive/v3/files/{id}/permissions", self.endpoints.api);
        let body = serde_json::json!({"type": "user", "role": "writer", "emailAddress": email});
        self.send("share", |a, auth| {
            a.post(&url)
                .header("Authorization", auth)
                .query("supportsAllDrives", "true")
                .query("sendNotificationEmail", if notify { "true" } else { "false" })
                .send_json(&body)
        })?;
        Ok(())
    }
}

impl DriveApi for HttpDrive {
    fn children(&self, folder: &str) -> Result<Vec<Entry>> {
        let url = format!("{}/drive/v3/files", self.endpoints.api);
        let q = format!("'{}' in parents and trashed = false", q_escape(folder));
        let mut out = vec![];
        let mut page: Option<String> = None;
        loop {
            let list: FileList = self
                .send("list folder", |a, auth| {
                    let mut r = a
                        .get(&url)
                        .header("Authorization", auth)
                        .query("q", &q)
                        .query("fields", "nextPageToken,files(id,name,mimeType)")
                        .query("pageSize", "1000")
                        .query("supportsAllDrives", "true")
                        .query("includeItemsFromAllDrives", "true");
                    if let Some(p) = &page {
                        r = r.query("pageToken", p);
                    }
                    r.call()
                })?
                .into_body()
                .read_json()
                .map_err(backend_err)?;
            out.extend(list.files.into_iter().map(|f| Entry {
                folder: f.mime_type == FOLDER_MIME,
                id: f.id,
                name: f.name,
            }));
            match list.next_page_token {
                Some(p) if !p.is_empty() => page = Some(p),
                _ => return Ok(out),
            }
        }
    }

    fn create_folder(&self, parent: &str, name: &str) -> Result<String> {
        let url = format!("{}/drive/v3/files", self.endpoints.api);
        let body = serde_json::json!({"name": name, "mimeType": FOLDER_MIME, "parents": [parent]});
        let f: FileMeta = self
            .send("create folder", |a, auth| {
                a.post(&url)
                    .header("Authorization", auth)
                    .query("fields", "id")
                    .query("supportsAllDrives", "true")
                    .send_json(&body)
            })?
            .into_body()
            .read_json()
            .map_err(backend_err)?;
        Ok(f.id)
    }

    fn upload(&self, parent: &str, name: &str, data: &[u8]) -> Result<String> {
        let url = format!("{}/upload/drive/v3/files", self.endpoints.upload);
        let meta = serde_json::json!({"name": name, "parents": [parent], "mimeType": "application/x-ndjson"});
        let boundary = format!("cpasfait-{}", uuid::Uuid::now_v7().simple());
        let mut body = Vec::with_capacity(data.len() + 512);
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/x-ndjson\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let f: FileMeta = self
            .send("upload", |a, auth| {
                a.post(&url)
                    .header("Authorization", auth)
                    .header("Content-Type", format!("multipart/related; boundary={boundary}"))
                    .query("uploadType", "multipart")
                    .query("fields", "id")
                    .query("supportsAllDrives", "true")
                    .send(&body[..])
            })?
            .into_body()
            .read_json()
            .map_err(backend_err)?;
        Ok(f.id)
    }

    fn download(&self, id: &str) -> Result<Vec<u8>> {
        let url = format!("{}/drive/v3/files/{id}", self.endpoints.api);
        self.send("download", |a, auth| {
            a.get(&url)
                .header("Authorization", auth)
                .query("alt", "media")
                .query("supportsAllDrives", "true")
                .call()
        })?
        .into_body()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .map_err(backend_err)
    }
}
