//! Google Drive backend: sync through the in-memory fake, and the HTTP client
//! against a small mock of the Drive REST API (backed by the same fake).
#![cfg(feature = "gdrive")]

use cpasfait_core::backend::RemoteBackend;
use cpasfait_core::backend::gdrive::http::{Agent, Endpoints, HttpDrive, TokenSource};
use cpasfait_core::backend::gdrive::mem::MemDrive;
use cpasfait_core::backend::gdrive::{DriveApi, FOLDER_MIME, GDrive};
use cpasfait_core::crypto::Identity;
use cpasfait_core::sync::{remote_tabs, sync_tab};
use cpasfait_core::{Place, Repo, TabId};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Device 1 shares a tab and invites device 2.
fn shared(b1: &dyn RemoteBackend) -> (Repo, Repo, Identity, Identity, TabId) {
    let (mut r1, r2) = (Repo::open_in_memory().unwrap(), Repo::open_in_memory().unwrap());
    let (k1, k2) = (Identity::generate(), Identity::generate());
    let tab = r1.create_tab("shared").unwrap();
    r1.share_tab(tab, k1.recipient()).unwrap();
    r1.set_member(tab, k2.recipient(), Some("bob")).unwrap();
    sync_tab(&mut r1, tab, b1, &k1).unwrap();
    (r1, r2, k1, k2, tab)
}

fn converge(b1: &dyn RemoteBackend, b2: &dyn RemoteBackend) {
    let (mut r1, mut r2, k1, k2, tab) = shared(b1);
    let a = r1.add_task(tab, Place::LastChildOf(None), "a").unwrap();
    sync_tab(&mut r1, tab, b1, &k1).unwrap();

    assert_eq!(remote_tabs(b2).unwrap(), vec![tab]);
    r2.add_known_tab(tab).unwrap();
    let s = sync_tab(&mut r2, tab, b2, &k2).unwrap();
    assert_eq!(s.pulled, 6);

    r1.set_title(a, "one").unwrap();
    r2.add_task(tab, Place::LastChildOf(Some(a)), "child").unwrap();
    r2.toggle_work(a).unwrap();
    for _ in 0..2 {
        sync_tab(&mut r1, tab, b1, &k1).unwrap();
        sync_tab(&mut r2, tab, b2, &k2).unwrap();
    }
    assert_eq!(r1.dump_model().unwrap(), r2.dump_model().unwrap());
    assert_eq!(r2.task(a).unwrap().title, "one");
    let s = sync_tab(&mut r1, tab, b1, &k1).unwrap();
    assert_eq!((s.pulled, s.pushed), (0, 0));
}

#[test]
fn two_devices_converge_through_fake_drive() {
    let mem = MemDrive::new();
    let b1 = GDrive::new(mem.clone(), MemDrive::ROOT);
    let b2 = GDrive::new(mem.clone(), MemDrive::ROOT);
    converge(&b1, &b2);
}

/// `backend` seen from inside the `<tab>` folder: `<tab>/x` is `x` there.
struct InTab<B>(B, String);

impl<B: RemoteBackend> RemoteBackend for InTab<B> {
    fn list(&self, prefix: &str) -> cpasfait_core::Result<Vec<String>> {
        let inner = prefix.strip_prefix(&self.1).unwrap_or("").trim_start_matches('/');
        Ok(self
            .0
            .list(inner)?
            .into_iter()
            .map(|p| format!("{}/{p}", self.1))
            .collect())
    }
    fn get(&self, path: &str) -> cpasfait_core::Result<Vec<u8>> {
        self.0.get(&path[self.1.len() + 1..])
    }
    fn put_new(&self, path: &str, data: &[u8]) -> cpasfait_core::Result<()> {
        self.0.put_new(&path[self.1.len() + 1..], data)
    }
}

#[test]
fn concurrently_created_tab_folders_are_merged() {
    // Both devices created the `<tab>` folder before seeing each other's.
    let mem = MemDrive::new();
    let root = GDrive::new(mem.clone(), MemDrive::ROOT);
    let (mut r1, mut r2, k1, k2, tab) = shared(&root);
    let t = mem.create_folder(MemDrive::ROOT, &tab.to_string()).unwrap();
    let dups: Vec<_> = mem.children(MemDrive::ROOT).unwrap();
    assert_eq!(dups.len(), 2);
    let second = GDrive::new(mem.clone(), t);
    let second = InTab(second, tab.to_string());

    r2.add_known_tab(tab).unwrap();
    sync_tab(&mut r2, tab, &root, &k2).unwrap();
    r1.add_task(tab, Place::LastChildOf(None), "from r1").unwrap();
    r2.add_task(tab, Place::LastChildOf(None), "from r2").unwrap();
    sync_tab(&mut r1, tab, &root, &k1).unwrap();
    sync_tab(&mut r2, tab, &second, &k2).unwrap(); // only sees the second folder
    sync_tab(&mut r1, tab, &root, &k1).unwrap();
    sync_tab(&mut r2, tab, &root, &k2).unwrap();
    assert_eq!(r1.dump_model().unwrap(), r2.dump_model().unwrap());
    assert_eq!(r1.tasks(tab).unwrap().len(), 2);
}

// ---------------------------------------------------------------- HTTP mock

struct Tokens {
    calls: AtomicUsize,
}

impl TokenSource for Tokens {
    fn token(&self, _: &Agent, refresh: bool) -> cpasfait_core::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The first token is "expired": the server only accepts the refreshed one.
        Ok(if refresh { "good" } else { "stale" }.into())
    }
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                out.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap(), 16).unwrap());
                i += 2;
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

struct Req {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn q(&self, k: &str) -> Option<&str> {
        self.query.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }
    fn h(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

fn read_req(stream: &mut std::net::TcpStream) -> Req {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let mut it = line.split_whitespace();
    let method = it.next().unwrap().to_owned();
    let target = it.next().unwrap().to_owned();
    let (path, qs) = target.split_once('?').unwrap_or((&target, ""));
    let query = qs
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect();
    let mut headers = vec![];
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':').unwrap();
        headers.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    let mut req = Req {
        method,
        path: path.to_owned(),
        query,
        headers,
        body: vec![],
    };
    if let Some(n) = req.h("content-length") {
        let mut body = vec![0; n.parse().unwrap()];
        r.read_exact(&mut body).unwrap();
        req.body = body;
    }
    assert!(req.h("transfer-encoding").is_none(), "unexpected chunked body");
    req
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Mock Drive: answers like the real API (pagination of 2 entries, 401 without the good token).
fn handle(mem: &MemDrive, req: &Req) -> (u16, Vec<u8>) {
    if req.h("authorization") != Some("Bearer good") {
        return (
            401,
            br#"{"error":{"code":401,"message":"Invalid Credentials"}}"#.to_vec(),
        );
    }
    let json = |v: serde_json::Value| (200, v.to_string().into_bytes());
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/drive/v3/files") => {
            let q = req.q("q").unwrap();
            let parent = q.strip_prefix('\'').unwrap().split('\'').next().unwrap();
            assert!(q.ends_with("in parents and trashed = false"), "{q}");
            assert_eq!(req.q("fields"), Some("nextPageToken,files(id,name,mimeType)"));
            let all = mem.children(parent).unwrap();
            let start: usize = req.q("pageToken").map(|p| p.parse().unwrap()).unwrap_or(0);
            let page: Vec<_> = all
                .iter()
                .skip(start)
                .take(2)
                .map(|e| {
                    serde_json::json!({"id": e.id, "name": e.name,
                    "mimeType": if e.folder { FOLDER_MIME } else { "application/x-ndjson" }})
                })
                .collect();
            let mut v = serde_json::json!({"files": page});
            if start + 2 < all.len() {
                v["nextPageToken"] = (start + 2).to_string().into();
            }
            json(v)
        }
        ("POST", "/drive/v3/files") => {
            let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(v["mimeType"], FOLDER_MIME);
            let id = mem
                .create_folder(v["parents"][0].as_str().unwrap(), v["name"].as_str().unwrap())
                .unwrap();
            json(serde_json::json!({ "id": id }))
        }
        ("POST", "/upload/drive/v3/files") => {
            assert_eq!(req.q("uploadType"), Some("multipart"));
            let ct = req.h("content-type").unwrap();
            let boundary = ct.strip_prefix("multipart/related; boundary=").unwrap();
            let delim = format!("--{boundary}");
            let body = &req.body;
            let parts: Vec<&[u8]> = {
                let mut v = vec![];
                let mut rest = &body[..];
                while let Some(i) = find(rest, delim.as_bytes()) {
                    v.push(&rest[..i]);
                    rest = &rest[i + delim.len()..];
                }
                v
            };
            // ["", part1, part2] then the closing "--"
            let content = |p: &[u8]| {
                let i = find(p, b"\r\n\r\n").unwrap();
                p[i + 4..p.len() - 2].to_vec()
            };
            let meta: serde_json::Value = serde_json::from_slice(&content(parts[1])).unwrap();
            let data = content(parts[2]);
            let id = mem
                .upload(
                    meta["parents"][0].as_str().unwrap(),
                    meta["name"].as_str().unwrap(),
                    &data,
                )
                .unwrap();
            json(serde_json::json!({ "id": id }))
        }
        ("GET", p) if p.starts_with("/drive/v3/files/") => {
            assert_eq!(req.q("alt"), Some("media"));
            match mem.download(&p["/drive/v3/files/".len()..]) {
                Ok(d) => (200, d),
                Err(_) => (404, br#"{"error":{"code":404,"message":"File not found"}}"#.to_vec()),
            }
        }
        _ => (400, b"{}".to_vec()),
    }
}

fn mock_server(mem: MemDrive) -> (String, std::sync::Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = std::sync::Arc::new(Mutex::new(0));
    let c2 = count.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let req = read_req(&mut stream);
            *c2.lock().unwrap() += 1;
            let (status, body) = handle(&mem, &req);
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    (url, count)
}

fn http_backend(url: &str) -> GDrive<HttpDrive> {
    let endpoints = Endpoints {
        api: url.to_owned(),
        upload: url.to_owned(),
    };
    let api = HttpDrive::with_endpoints(
        Box::new(Tokens {
            calls: AtomicUsize::new(0),
        }),
        endpoints,
    );
    GDrive::new(api, MemDrive::ROOT)
}

#[test]
fn http_client_against_mock() {
    let mem = MemDrive::new();
    let (url, _) = mock_server(mem.clone());
    let b = http_backend(&url);
    // Data with a CRLF and bytes that look like a boundary.
    let data = b"line1\r\n--cpasfait-x\r\n\xff\x00end";
    b.put_new("t/ops/d/1.jsonl", data).unwrap();
    for i in 2..7 {
        b.put_new(&format!("t/ops/d/{i}.jsonl"), b"x").unwrap(); // > 1 page
    }
    assert!(matches!(
        b.put_new("t/ops/d/1.jsonl", b"again"),
        Err(cpasfait_core::Error::AlreadyExists(_))
    ));
    assert_eq!(b.list("t").unwrap().len(), 6);
    assert_eq!(b.get("t/ops/d/1.jsonl").unwrap(), data);
    assert_eq!(mem.folder_count(), 3);
    // Errors carry the server's message.
    let err = b.api().download("nope").unwrap_err();
    assert!(matches!(err, cpasfait_core::Error::NotFound(_)), "{err}");
}

#[test]
fn two_devices_converge_over_http() {
    let mem = MemDrive::new();
    let (url, _) = mock_server(mem);
    converge(&http_backend(&url), &http_backend(&url));
}
