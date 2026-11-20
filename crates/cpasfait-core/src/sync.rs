//! Synchronisation of a tab with a remote backend. Everything on the remote
//! is encrypted (see [`crate::crypto`] and `docs/format.md`).
//!
//! Remote layout, per tab:
//! - `<tab>/keys/<epoch>/<device>-<random>.age`: an epoch key, age-encrypted
//!   to the tab's members;
//! - `<tab>/ops/<device>/<first-seq>-<last-seq>.seg`: ops (JSON lines),
//!   sealed with an epoch key.
//!
//! Files are only ever created, never modified; each device writes segments
//! in its own directory only.
//!
//! Membership is ordinary LWW state (`tab.member:<public key>` ops), and keys
//! follow it on every sync:
//! - every key is (re)distributed to every current member, so new members
//!   can read the whole history;
//! - when someone who received the current key is no longer a member, a new
//!   epoch is started (chained to the previous one by an HMAC), so they
//!   cannot read anything written afterwards.

use crate::backend::RemoteBackend;
use crate::crypto::{self, Identity, Key};
use crate::error::{Error, Result};
use crate::hlc::Hlc;
use crate::ids::{DeviceId, TabId};
use crate::model::Epoch;
use crate::op::Op;
use crate::repo::Repo;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// New ops received.
    pub pulled: usize,
    /// Ops sent.
    pub pushed: usize,
    /// Devices whose next segment uses a key this device does not have (yet).
    pub waiting_for_key: usize,
    /// Key files uploaded (new members, new epochs).
    pub keys_sent: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Segment {
    first: u64,
    last: u64,
}

fn segment_name(first: u64, last: u64) -> String {
    format!("{first:020}-{last:020}.seg")
}

fn parse_segment_path(tab: TabId, path: &str) -> Option<(DeviceId, Segment)> {
    let rest = path.strip_prefix(&format!("{tab}/ops/"))?;
    let (device, file) = rest.split_once('/')?;
    let (first, last) = file.strip_suffix(".seg")?.split_once('-')?;
    let seg = Segment {
        first: first.parse().ok()?,
        last: last.parse().ok()?,
    };
    (seg.first <= seg.last).then_some(())?;
    Some((device.parse().ok()?, seg))
}

pub fn encode_ops(ops: &[Op]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for op in ops {
        serde_json::to_writer(&mut out, op)?;
        out.push(b'\n');
    }
    Ok(out)
}

pub fn decode_ops(data: &[u8]) -> Result<Vec<Op>> {
    data.split(|&b| b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .map(|l| serde_json::from_slice(l).map_err(Error::from))
        .collect()
}

// ------------------------------------------------------------------ keys

/// A member removal, as known by the writer of a key file: the epoch must not
/// be given to `recipient` unless they were re-added after `hlc`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exclusion {
    pub recipient: String,
    pub hlc: Hlc,
}

/// What this device knows about a remote key file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyFile {
    Valid {
        epoch: Uuid,
        recipients: Vec<String>,
        exclude: Vec<Exclusion>,
    },
    /// Not encrypted to this device.
    Foreign,
    /// Malformed, or not provably made by a member.
    Invalid,
}

/// Plaintext of a key file.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct KeyPayload {
    v: u32,
    tab: TabId,
    epoch: Uuid,
    /// Hex.
    key: String,
    prev: Option<PrevLink>,
    /// Everyone this file is encrypted to.
    recipients: Vec<String>,
    exclude: Vec<Exclusion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PrevLink {
    epoch: Uuid,
    /// Hex HMAC-SHA256 of the new epoch under the previous epoch's key.
    mac: String,
}

fn key_dir(tab: TabId) -> String {
    format!("{tab}/keys")
}

fn parse_key_path(tab: TabId, path: &str) -> Option<Uuid> {
    let rest = path.strip_prefix(&format!("{tab}/keys/"))?;
    let (epoch, file) = rest.split_once('/')?;
    file.ends_with(".age").then_some(())?;
    epoch.parse().ok()
}

/// Who holds each epoch's key according to the remote key files, and who must
/// not get it.
#[derive(Default)]
struct Coverage {
    recipients: HashSet<String>,
    exclude: Vec<Exclusion>,
}

fn add_coverage(cov: &mut HashMap<Uuid, Coverage>, kf: &KeyFile) {
    if let KeyFile::Valid {
        epoch,
        recipients,
        exclude,
    } = kf
    {
        let c = cov.entry(*epoch).or_default();
        c.recipients.extend(recipients.iter().cloned());
        c.exclude.extend(exclude.iter().cloned());
    }
}

/// Download the key files not examined yet and adopt the epochs they prove.
/// Returns the coverage of every epoch (from the files currently listed).
fn fetch_keys(
    repo: &mut Repo,
    tab: TabId,
    backend: &dyn RemoteBackend,
    me: &Identity,
) -> Result<HashMap<Uuid, Coverage>> {
    let listed: Vec<(String, Uuid)> = backend
        .list(&key_dir(tab))?
        .into_iter()
        .filter_map(|p| parse_key_path(tab, &p).map(|e| (p, e)))
        .collect();
    let mut seen = repo.key_files(tab)?;
    let mut pending: Vec<(String, KeyPayload, Key)> = vec![];
    for (path, epoch) in &listed {
        if seen.contains_key(path) {
            continue;
        }
        let kf = match me.unwrap(&backend.get(path)?) {
            Ok(None) => KeyFile::Foreign,
            Err(_) => KeyFile::Invalid,
            Ok(Some(plain)) => match serde_json::from_slice::<KeyPayload>(&plain) {
                Ok(p) if p.v == 1 && p.tab == tab && p.epoch == *epoch => {
                    match crypto::unhex(&p.key).and_then(|k| Key::try_from(k).ok()) {
                        Some(k) => {
                            pending.push((path.clone(), p, k));
                            continue;
                        }
                        None => KeyFile::Invalid,
                    }
                }
                _ => KeyFile::Invalid,
            },
        };
        repo.record_key_file(tab, path, &kf)?;
        seen.insert(path.clone(), kf);
    }
    // Validate against the epochs already held, to a fixpoint (a file may
    // prove an epoch whose child is proved by another file).
    let mut known: HashMap<Uuid, Epoch> = repo.epochs(tab)?.into_iter().map(|e| (e.id, e)).collect();
    loop {
        let mut progress = false;
        let mut rest = vec![];
        for (path, p, key) in pending {
            let verdict = if let Some(e) = known.get(&p.epoch) {
                Some(e.key == key)
            } else if let Some(link) = &p.prev {
                match known.get(&link.epoch).map(|prev| (prev.key, prev.depth)) {
                    None => None,
                    Some((prev_key, prev_depth)) => {
                        let ok = crypto::unhex(&link.mac)
                            .is_some_and(|mac| crypto::verify_epoch_mac(&prev_key, tab, p.epoch, &key, &mac));
                        if ok {
                            let e = Epoch {
                                id: p.epoch,
                                key,
                                prev: Some(link.epoch),
                                depth: prev_depth + 1,
                            };
                            known.insert(e.id, e);
                        }
                        Some(ok)
                    }
                }
            } else if known.is_empty() {
                None // a first epoch: decided below
            } else {
                Some(false) // a second "first" epoch: not from a member
            };
            match verdict {
                Some(valid) => {
                    let kf = if valid {
                        KeyFile::Valid {
                            epoch: p.epoch,
                            recipients: p.recipients,
                            exclude: p.exclude,
                        }
                    } else {
                        KeyFile::Invalid
                    };
                    repo.record_key_file(tab, &path, &kf)?;
                    seen.insert(path, kf);
                    progress = true;
                }
                None => rest.push((path, p, key)),
            }
        }
        pending = rest;
        if !progress && known.is_empty() {
            // Joining: trust on first use of the tab's root key, which must be unique.
            let roots: BTreeSet<Uuid> = pending
                .iter()
                .filter(|(_, p, _)| p.prev.is_none())
                .map(|(_, p, _)| p.epoch)
                .collect();
            match roots.len() {
                0 => break,
                1 => {
                    let (_, p, key) = pending.iter().find(|(_, p, _)| p.prev.is_none()).expect("a root");
                    known.insert(
                        p.epoch,
                        Epoch {
                            id: p.epoch,
                            key: *key,
                            prev: None,
                            depth: 0,
                        },
                    );
                    continue;
                }
                _ => {
                    return Err(Error::Crypto(format!(
                        "tab {tab} has several root keys encrypted to you: refusing to guess which one is genuine"
                    )));
                }
            }
        }
        if !progress {
            break; // the rest waits for its parent epoch's key file
        }
    }
    for e in known.values() {
        repo.add_epoch(tab, e)?;
    }
    let mut cov = HashMap::new();
    for (path, _) in &listed {
        if let Some(kf) = seen.get(path) {
            add_coverage(&mut cov, kf);
        }
    }
    Ok(cov)
}

/// Rotate the key if a non-member holds the current one, and give every
/// current member every key. Returns the number of key files written.
fn distribute_keys(
    repo: &mut Repo,
    tab: TabId,
    backend: &dyn RemoteBackend,
    me: &Identity,
    cov: &mut HashMap<Uuid, Coverage>,
) -> Result<usize> {
    let members = repo.members(tab)?;
    let active: HashMap<&str, Hlc> = members
        .iter()
        .filter(|m| m.active)
        .map(|m| (m.recipient.as_str(), m.hlc))
        .collect();
    if !active.contains_key(me.recipient()) {
        return Ok(0); // not a member (any more): not ours to hand out keys
    }
    let removed: Vec<Exclusion> = members
        .iter()
        .filter(|m| !m.active)
        .map(|m| Exclusion {
            recipient: m.recipient.clone(),
            hlc: m.hlc,
        })
        .collect();
    if let Some(w) = repo.write_epoch(tab)?
        && cov
            .get(&w.id)
            .is_some_and(|c| c.recipients.iter().any(|r| !active.contains_key(r.as_str())))
    {
        repo.add_epoch(
            tab,
            &Epoch {
                id: Uuid::now_v7(),
                key: crypto::random_key(),
                prev: Some(w.id),
                depth: w.depth + 1,
            },
        )?;
    }
    let epochs = repo.epochs(tab)?;
    let by_id: HashMap<Uuid, &Epoch> = epochs.iter().map(|e| (e.id, e)).collect();
    let mut written = 0;
    for e in &epochs {
        let c = cov.entry(e.id).or_default();
        // Removed (and not re-added since) according to any file of this epoch.
        let allowed = |r: &str, hlc: &Hlc| !c.exclude.iter().any(|x| x.recipient == r && x.hlc >= *hlc);
        let mut to: Vec<String> = active
            .iter()
            .filter(|(r, h)| allowed(r, h))
            .map(|(r, _)| (*r).to_owned())
            .collect();
        if to.iter().all(|r| c.recipients.contains(r)) {
            continue;
        }
        to.sort();
        let prev = match e.prev {
            None => None,
            Some(p) => {
                let pk = by_id
                    .get(&p)
                    .ok_or_else(|| Error::Crypto(format!("missing parent key of epoch {}", e.id)))?;
                Some(PrevLink {
                    epoch: p,
                    mac: crypto::hex(&crypto::epoch_mac(&pk.key, tab, e.id, &e.key)),
                })
            }
        };
        let payload = KeyPayload {
            v: 1,
            tab,
            epoch: e.id,
            key: crypto::hex(&e.key),
            prev,
            recipients: to.clone(),
            exclude: removed.clone(),
        };
        let data = crypto::wrap(&to, &serde_json::to_vec(&payload)?)?;
        let path = format!("{}/{}/{}-{}.age", key_dir(tab), e.id, repo.device(), Uuid::new_v4());
        backend.put_new(&path, &data)?;
        let kf = KeyFile::Valid {
            epoch: e.id,
            recipients: to,
            exclude: removed.clone(),
        };
        repo.record_key_file(tab, &path, &kf)?;
        add_coverage(cov, &kf);
        written += 1;
    }
    Ok(written)
}

// ------------------------------------------------------------------- ops

/// Fetch new ops of other devices. Returns (new ops, devices waiting for a key).
fn pull(repo: &mut Repo, tab: TabId, backend: &dyn RemoteBackend) -> Result<(usize, usize)> {
    let keys: HashMap<Uuid, Key> = repo.epochs(tab)?.into_iter().map(|e| (e.id, e.key)).collect();
    let mut per_device: BTreeMap<DeviceId, BTreeSet<Segment>> = BTreeMap::new();
    for path in backend.list(&format!("{tab}/ops"))? {
        if let Some((device, seg)) = parse_segment_path(tab, &path) {
            per_device.entry(device).or_default().insert(seg);
        }
    }
    let (mut new, mut waiting) = (0, 0);
    for (device, segments) in per_device {
        let mut cursor = repo.cursor(tab, device)?;
        for seg in segments {
            if seg.last <= cursor {
                continue;
            }
            if seg.first > cursor + 1 {
                // A segment is missing (not uploaded/synced yet): don't skip past it.
                break;
            }
            let path = format!("{tab}/ops/{device}/{}", segment_name(seg.first, seg.last));
            let data = backend.get(&path)?;
            let Some(key) = keys.get(&crypto::sealed_epoch(&data)?) else {
                waiting += 1;
                break;
            };
            let ops = decode_ops(&crypto::open(key, &path, &data)?)?;
            if ops.iter().any(|o| o.tab != tab || o.hlc.device != device) {
                return Err(Error::Invalid(format!("segment {path} contains foreign ops")));
            }
            new += repo.ingest(&ops)?;
            cursor = seg.last;
            repo.set_cursor(tab, device, cursor)?;
        }
    }
    Ok((new, waiting))
}

/// Upload this device's unpushed ops as one new segment.
fn push(repo: &mut Repo, tab: TabId, backend: &dyn RemoteBackend, me: &Identity) -> Result<usize> {
    let ops = repo.unpushed_ops(tab)?;
    let (Some(first), Some(last)) = (ops.first(), ops.last()) else {
        return Ok(0);
    };
    let epoch = repo
        .write_epoch(tab)?
        .ok_or_else(|| Error::NotInvited(me.recipient().to_owned()))?;
    let (first, last) = (first.seq, last.seq);
    let path = format!("{tab}/ops/{}/{}", repo.device(), segment_name(first, last));
    let data = crypto::seal(&epoch.key, epoch.id, &path, &encode_ops(&ops)?);
    match backend.put_new(&path, &data) {
        // Already there: an earlier push succeeded but was not recorded.
        Ok(()) | Err(Error::AlreadyExists(_)) => {}
        Err(e) => return Err(e),
    }
    repo.mark_pushed(tab, last)?;
    let device = repo.device();
    repo.set_cursor(tab, device, last)?;
    Ok(ops.len())
}

/// Sync one tab, as `me`. The tab must have been shared from this device
/// ([`Repo::share_tab`]) or this device's key invited by a member; otherwise
/// [`Error::NotInvited`].
pub fn sync_tab(repo: &mut Repo, tab: TabId, backend: &dyn RemoteBackend, me: &Identity) -> Result<SyncStats> {
    let mut cov = fetch_keys(repo, tab, backend, me)?;
    if repo.epochs(tab)?.is_empty() {
        return Err(Error::NotInvited(me.recipient().to_owned()));
    }
    let (pulled, waiting_for_key) = pull(repo, tab, backend)?;
    // After pulling: membership changes received are acted upon right away.
    let keys_sent = distribute_keys(repo, tab, backend, me, &mut cov)?;
    let pushed = push(repo, tab, backend, me)?;
    Ok(SyncStats {
        pulled,
        pushed,
        waiting_for_key,
        keys_sent,
    })
}

/// Sync every tab that has a remote configured, with the default identity.
pub fn sync_all(repo: &mut Repo) -> Result<Vec<(TabId, Result<SyncStats>)>> {
    let tabs: Vec<_> = repo.tabs()?.into_iter().filter(|t| t.remote.is_some()).collect();
    if tabs.is_empty() {
        return Ok(vec![]);
    }
    let me = Identity::load_default()?.ok_or_else(|| {
        Error::Crypto("no identity (secret key) found: see `:mykey`, or set $CPASFAIT_IDENTITY".into())
    })?;
    let mut out = vec![];
    for tab in tabs {
        let remote = tab.remote.as_ref().expect("filtered");
        let res = crate::backend::open(remote).and_then(|b| sync_tab(repo, tab.id, b.as_ref(), &me));
        out.push((tab.id, res));
    }
    Ok(out)
}

/// Tab ids present in a remote (to join a shared tab).
pub fn remote_tabs(backend: &dyn RemoteBackend) -> Result<Vec<TabId>> {
    let mut set = BTreeSet::new();
    for path in backend.list("")? {
        if let Some((tab, rest)) = path.split_once('/')
            && (rest.starts_with("keys/") || rest.starts_with("ops/"))
            && let Ok(id) = tab.parse()
        {
            set.insert(id);
        }
    }
    Ok(set.into_iter().collect())
}

/// Whether some key file of `tab` is encrypted to `me` (to join a shared tab).
pub fn invited_to(backend: &dyn RemoteBackend, tab: TabId, me: &Identity) -> Result<bool> {
    for path in backend.list(&key_dir(tab))? {
        if parse_key_path(tab, &path).is_some() && matches!(me.unwrap(&backend.get(&path)?), Ok(Some(_))) {
            return Ok(true);
        }
    }
    Ok(false)
}
