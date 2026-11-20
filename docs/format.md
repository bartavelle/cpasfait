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
| `task.title` | string | |
| `task.notes` | string | free text |
| `task.status` | `"todo"` \| `"done"` | default `todo` |
| `task.priority` | integer | default 0, higher is more important (UI uses 0–9) |
| `task.due` | `"YYYY-MM-DD"` \| null | |
| `task.parent` | task id \| null | null/absent = root |
| `task.position` | string | fractional index among siblings, see below |
| `task.tag:<name>` | bool | tag present or not; one register per tag |
| `task.blocked_by:<task id>` | bool | this task requires that one (same tab); one register per pair |
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

- A requirement on a task that is deleted or unknown locally is ignored.
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
<root>/<tab-id>/ops/<device-id>/<first-seq>-<last-seq>.jsonl
```

- Seq numbers are zero-padded to 20 digits. The file holds one op per line, in seq order.
- A device only creates files in its own directory and never modifies or
  deletes them. Because each name is unique, the remote needs no locking or
  compare-and-swap: listing, reading and create-only writes are enough.
- Readers keep a cursor per (tab, device) holding the last seq ingested.
  Segments are read in order. If the next segment does not start at
  `cursor + 1`, the reader stops for that device until the missing segment
  shows up (for example, when a sync tool has not delivered it yet).
- Files whose names start with `.` are ignored (temporary files, sync-tool metadata).

Joining a shared tab = knowing the remote root plus the tab id, then pulling.

### Google Drive

The root is a Drive folder, named by its id (`gdrive:<folder-id>`). Each path
component is a sub-folder with that name, and segment files are regular files
created with a single multipart upload, so they are never seen half-written.

Drive allows several children with the same name, and has no create-if-absent,
so two clients may create duplicate `<tab-id>` or `ops` folders. Clients must
therefore:

- resolve a path to the *set* of all folders matching it, and list the union
  of their contents;
- when writing, use an existing matching folder (the one with the smallest id)
  and create a folder only when none exists.

A duplicate *file* (a retried upload) has identical content and can be read
from either copy. Trashed files are ignored.

