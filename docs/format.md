# cpasfait data format

This is the contract between clients (the Rust TUI and tools today, a mobile app
later). The reference implementation is `crates/cpasfait-core`.

## Model

Everything is a set of **last-writer-wins registers**: `(entity, field) → value`.
An **op** assigns one register. For each register, the op with the greatest
**HLC** wins. Applying the same set of ops in any order, any number of
times, gives the same state. That is what makes concurrent writers safe.

### Ids

All ids are UUIDv7 strings: tabs, tasks, work sessions, devices. Each install
(database) is one device.

### HLC (hybrid logical clock)

`{"wall": <unix ms>, "counter": <u32>, "device": "<uuid>"}`. Compared as the
tuple `(wall, counter, device)`. For a local op, if `now > wall` then
`wall = now, counter = 0`, otherwise `counter += 1`. When a device receives
ops, it moves its clock forward to the largest HLC seen, so its later edits
are ordered after them.

### Op

```json
{"hlc": {...}, "seq": 12, "tab": "<tab id>", "entity": "<uuid>", "field": "task.title", "value": "Buy milk"}
```

- `seq`: sequence number per (device, tab), starting at 1, without gaps.
- `tab`: the tab the entity belongs to. For tab fields, `entity == tab`.
- A client keeps ops with an unknown `field` (and forwards them) but ignores them.

### Fields

| field | value | notes |
|---|---|---|
| `tab.name` | string | |
| `tab.deleted` | bool | tombstone |
| `tab.move` | `{"id": uuid, "remote": spec, "by": name}` | the tab's data was moved to another remote (spec as typed in `:tabremote`) |
| `tab.member:<public key>` | string \| `false` | a member of a shared tab and their display name; `false` once removed. The key is an age X25519 recipient (`age1…`) or an SSH public key without comment (`ssh-ed25519 AAAA…`) |
| `task.title` | string | |
| `task.notes` | string | free text |
| `task.status` | `"todo"` \| `"done"` | default `todo` |
| `task.priority` | integer | default 0, higher is more important (UI uses 0–9) |
| `task.due` | `"YYYY-MM-DD"` \| null | |
| `task.parent` | task id \| null | null/absent = root |
| `task.position` | string | fractional index among siblings, see below |
| `task.tag:<name>` | bool | tag present or not; one register per tag |
| `task.blocked_by:<task id>` | tab id \| `true` \| `false` | this task requires that one; the value is the required task's tab (it may be in another tab, `true` = tab unknown), `false` removes the link; one register per pair |
| `task.deleted` | bool | tombstone; hides the subtree |
| `session.task` | task id | work session → task |
| `session.device` | device id | the device that runs the timer |
| `session.user` | string | display name |
| `session.start` | unix ms | |
| `session.end` | unix ms \| null | null = running |
| `session.note` | string | |
| `session.deleted` | bool | tombstone |

An entity exists as soon as any op targets it. Missing fields take their defaults.

### Tree rules

- Siblings are ordered by `(position, id)`. Positions are strings over
  `0-9A-Za-z` (ASCII order), never ending in `0`. A key strictly between any
  two keys always exists (`position::key_between`).
- A parent that is unknown locally: the task is shown at the root until the parent arrives.
- A deleted parent hides its whole subtree.
- Parent cycles (two concurrent moves): the member of the cycle with the
  smallest id is treated as a root. This is deterministic, so every replica
  shows the same tree.

### Dependency rules

- Requirements may cross tabs. A requirement on a task that is deleted, or
  unknown locally (its tab is not shared with this device, or not synced
  yet), does not block; clients should show it as a problem.
- A task is blocked while any of its requirements is not done (a task under
  a done parent counts as done). Blocked tasks are never the "next task".
- Clients refuse to create self-requirements, requirements on an ancestor,
  and cycles, but concurrent edits can still produce a cycle: every task in it
  is then blocked until someone removes a link. Nothing else breaks.

### Work log rules

Each work session is its own entity, so concurrent sessions never conflict.
A device keeps at most one open session (`end` null) for itself: starting
work on a task closes the device's open session(s) first.

## Remote layout

```
<root>/<tab-id>/keys/<epoch-id>/<device-id>-<random-uuid>.age
<root>/<tab-id>/ops/<device-id>/<first-seq>-<last-seq>.seg
```

- Seq numbers are zero-padded to 20 digits. A segment holds one op per line,
  in seq order, encrypted (see below).
- A device only creates files in its own directory (key files: under its own
  name) and never modifies or deletes them. Because each name is unique, the
  remote needs no locking or compare-and-swap: listing, reading and
  create-only writes are enough.
- Readers keep a cursor per (tab, device) holding the last seq ingested.
  Segments are read in order. If the next segment does not start at
  `cursor + 1`, or uses a key the reader does not have yet, the reader stops
  for that device until it can go on (for example, when a sync tool has not
  delivered a file yet).
- Files whose names start with `.` are ignored (temporary files, sync-tool metadata).

Joining a shared tab = knowing the remote root plus the tab id, having been
invited (a key file encrypted to you), then pulling.

A device whose own segments are missing from the remote (a lost file, a new
remote) uploads its ops again from the end of its unbroken run of segments:
`<end+1>-<last pushed>`. Segments of a device may thus overlap; readers ingest
any segment that starts at or before `cursor + 1` and skip ops they have.

### Moving to another remote

1. The mover records a `tab.move` op and syncs it to the old remote.
2. It copies every file of the tab that the new remote lacks, unchanged
   (files are only bound to their path relative to the root, so they stay
   valid).
3. It switches to the new remote.

Other devices see the op on the old remote. To follow, a device:
- checks that the new remote (as given, or edited for this machine) already
  has key files of the tab;
- copies from the old remote whatever the new one lacks (files written there
  after the mover's copy);
- switches to the new remote.

Nothing is written to the old remote any more. If the old remote is gone,
devices upload their own missing ops again (see above). Locally, each device
remembers the id of the last move it made or followed; a newer `tab.move` is
pending until it follows it.

## Encryption

Everything on a remote is encrypted; there is no plaintext mode.

**Epochs.** A shared tab has one or more epochs, each with a random 256-bit
key. The first is created by the device that shares the tab. A new one
(*rotation*) is created when someone who was given the newest key is no
longer a member. Writers use the newest epoch: the longest chain, then the
highest epoch id (UUIDv7), so concurrent rotations settle on the same one.

**Segments** are `"CPF1" | epoch id (16 bytes) | nonce (24 bytes) |
XChaCha20-Poly1305 ciphertext`. The associated data is the 44-byte header
followed by the file's remote path (`<tab>/ops/<device>/<first>-<last>.seg`),
so a segment cannot be moved, renamed or replayed elsewhere.

**Key files** are [age](https://age-encryption.org/v1) files encrypted to
members' public keys. The plaintext is JSON:

```json
{"v": 1, "tab": "<tab id>", "epoch": "<epoch id>", "key": "<hex>",
 "prev": null | {"epoch": "<parent epoch id>", "mac": "<hex>"},
 "recipients": ["age1…", "ssh-ed25519 AAAA…"],
 "exclude": [{"recipient": "age1…", "hlc": {"wall": 0, "counter": 0, "device": "…"}}]}
```

- `mac` is HMAC-SHA256, keyed with the parent epoch's key, of
  `"cpasfait epoch v1\0" | tab id (16 bytes) | epoch id (16 bytes) | key`.
  Only someone who already had a key of the tab can produce it, so the
  remote's owner cannot introduce a key of its own.
- `recipients` lists everyone the file is encrypted to: every current member
  allowed to have this key. So any member reading it knows who already has
  the key.
- `exclude` lists the members removed when the file was written, with the
  clock of their removal. Nobody may give this epoch to such a recipient
  unless they were re-added later (a newer `tab.member` op).

**Accepting keys.** A reader accepts a key file when:
- its `tab` and `epoch` match its path;
- the epoch is either already held with the same key, or chained (valid
  `mac`) to an epoch it holds.

A device that holds no key yet (joining) trusts the root epoch (`prev: null`)
if exactly one is encrypted to it; with several, it refuses to join. Anything
else is ignored.

**On every sync**, after fetching keys and pulling segments, a member:

1. starts a new epoch if a recipient of the newest one is no longer a member;
2. for every epoch it holds, if some current member allowed to have it is
   not among the `recipients` of the listed key files, writes a new key file
   for that epoch to all such members.

So a new member is given every key (the whole history), concurrent
invitations and removals converge, and a removed member gets no key made
after their removal. A removed member who keeps write access to the remote
can still add segments under keys they had: removing their access to the
remote itself is part of removing them.

### rsync over SSH

The root is a directory on an SSH host. Clients keep a local mirror and only
run `rsync -r --ignore-existing` (never `--delete`): remote → mirror before
listing, and a single file mirror → remote when creating it. rsync writes to a
`.`-prefixed temporary file and renames it, so readers never see a partial
file. Because files are immutable and uniquely named, this union of both sides
is a correct merge, even with several clients at once.

### Google Drive

The root is a Drive folder, named by its id (`gdrive:<folder-id>`). Each path
component is a sub-folder with that name, and segment files are regular files
created with a single multipart upload, so they are never seen half-written.

Drive allows several children with the same name, and has no create-if-absent,
so two clients may create duplicate `<tab-id>`, `ops`, `keys`… folders. Clients must
therefore:

- resolve a path to the *set* of all folders matching it, and list the union
  of their contents;
- when writing, use an existing matching folder (the one with the smallest id)
  and create a folder only when none exists.

A duplicate *file* (a retried upload) has identical content and can be read
from either copy. Trashed files are ignored.

