//! Synchronisation of a tab with a remote backend.
//!
//! Remote layout (see `docs/format.md`):
//! `<tab-id>/ops/<device-id>/<first-seq>-<last-seq>.jsonl`, one JSON op per line.
//! Each device only creates files in its own directory, never modifies them.

use crate::backend::RemoteBackend;
use crate::error::{Error, Result};
use crate::ids::{DeviceId, TabId};
use crate::op::Op;
use crate::repo::Repo;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// New ops received.
    pub pulled: usize,
    /// Ops sent.
    pub pushed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Segment {
    first: u64,
    last: u64,
}

fn segment_name(first: u64, last: u64) -> String {
    format!("{first:020}-{last:020}.jsonl")
}

fn parse_segment_path(tab: TabId, path: &str) -> Option<(DeviceId, Segment)> {
    let rest = path.strip_prefix(&format!("{tab}/ops/"))?;
    let (device, file) = rest.split_once('/')?;
    let (first, last) = file.strip_suffix(".jsonl")?.split_once('-')?;
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

/// Fetch new ops of other devices.
pub fn pull(repo: &mut Repo, tab: TabId, backend: &dyn RemoteBackend) -> Result<usize> {
    let mut per_device: BTreeMap<DeviceId, BTreeSet<Segment>> = BTreeMap::new();
    for path in backend.list(&format!("{tab}/ops"))? {
        if let Some((device, seg)) = parse_segment_path(tab, &path) {
            per_device.entry(device).or_default().insert(seg);
        }
    }
    let mut new = 0;
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
            let ops = decode_ops(&backend.get(&path)?)?;
            if ops.iter().any(|o| o.tab != tab || o.hlc.device != device) {
                return Err(Error::Invalid(format!("segment {path} contains foreign ops")));
            }
            new += repo.ingest(&ops)?;
            cursor = seg.last;
            repo.set_cursor(tab, device, cursor)?;
        }
    }
    Ok(new)
}

/// Upload this device's unpushed ops as one new segment.
pub fn push(repo: &mut Repo, tab: TabId, backend: &dyn RemoteBackend) -> Result<usize> {
    let ops = repo.unpushed_ops(tab)?;
    let (Some(first), Some(last)) = (ops.first(), ops.last()) else {
        return Ok(0);
    };
    let (first, last) = (first.seq, last.seq);
    let path = format!("{tab}/ops/{}/{}", repo.device(), segment_name(first, last));
    match backend.put_new(&path, &encode_ops(&ops)?) {
        // Already there: an earlier push succeeded but was not recorded.
        Ok(()) | Err(Error::AlreadyExists(_)) => {}
        Err(e) => return Err(e),
    }
    repo.mark_pushed(tab, last)?;
    let device = repo.device();
    repo.set_cursor(tab, device, last)?;
    Ok(ops.len())
}

pub fn sync_tab(repo: &mut Repo, tab: TabId, backend: &dyn RemoteBackend) -> Result<SyncStats> {
    let pulled = pull(repo, tab, backend)?;
    let pushed = push(repo, tab, backend)?;
    Ok(SyncStats { pulled, pushed })
}

/// Sync every tab that has a remote configured.
pub fn sync_all(repo: &mut Repo) -> Result<Vec<(TabId, Result<SyncStats>)>> {
    let mut out = vec![];
    for tab in repo.tabs()? {
        if let Some(remote) = &tab.remote {
            let res = crate::backend::open(remote).and_then(|b| sync_tab(repo, tab.id, b.as_ref()));
            out.push((tab.id, res));
        }
    }
    Ok(out)
}

/// Tab ids present in a remote (to join a shared tab).
pub fn remote_tabs(backend: &dyn RemoteBackend) -> Result<Vec<TabId>> {
    let mut set = BTreeSet::new();
    for path in backend.list("")? {
        if let Some((tab, rest)) = path.split_once('/')
            && rest.starts_with("ops/")
            && let Ok(id) = tab.parse()
        {
            set.insert(id);
        }
    }
    Ok(set.into_iter().collect())
}
