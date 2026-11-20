# cpasfait — TUI todo app: architecture + MVP plan

## Context
The repo is a fresh `cargo new` (single package `cpasfait`, edition 2024, empty deps, hello-world `src/main.rs`).
Goal: a Rust TUI todo app with:
- **Tabs** grouping tasks; each tab has local storage and optionally a **remote** shared by several users who may write concurrently.
- **Hierarchy** (nested subtasks, collapse/expand) and **rich metadata** (due, priority, tags, notes; recurrence later).
- **Vim-like modal** interaction.
- A future **Android app** (Kotlin) must reuse the same storage/sync → shared Rust core, Kotlin bindings via UniFFI later.
- First remote target: Google Drive (Android/Google ecosystem).

### Remote backend: answering "does Google offer something with this property? is DAV enough?"
The key design choice is to **make the remote need no concurrency guarantees at all**: every device only ever
*creates new immutable files with unique names* (`ops/<device-id>/<segment>.seg`) and never overwrites or
appends to anything shared. Merging happens client-side (op-log + LWW). So the backend only needs
`list(prefix)`, `get(path)`, `put_new(path, bytes)` — no compare-and-swap, no locking.
- **Google Drive** satisfies this (REST v3: create file in a folder, list children, download). Caveats: Drive does
  *not* speak WebDAV; `appDataFolder` is per-user and can't be shared, so multi-user tabs need a regular shared
  folder. `drive.file` scope only sees files the app created/opened (sharing another user's folder may need the
  Picker, or the restricted `drive` scope requiring Google verification) — to validate before building that backend.
- **WebDAV** is fully sufficient (PROPFIND/GET/PUT, `If-None-Match: *` for create-only) → Nextcloud etc. later.
- Other Google options considered: Firestore has real transactions + offline Android SDK but locks the format to
  Firebase and is awkward from Rust; GCS has true CAS (`ifGenerationMatch`) but needs a GCP project; Google Tasks API
  is too limited (no tags/priorities). Not needed given the append-only design.
- A local-directory backend also works through any file-sync tool (Drive desktop, Syncthing, Dropbox) — that is the
  MVP backend and the test harness for sync.

## Architecture

### Cargo workspace (convert existing root `Cargo.toml`)
```
Cargo.toml                 # [workspace] members = ["crates/*"], resolver 3, shared deps
crates/cpasfait-core/      # no UI deps; future UniFFI target
  src/hlc.rs               # hybrid logical clock
  src/op.rs                # Op, Field, Value; JSON (de)serialization = documented wire format
  src/model.rs             # Task, Tab, materialized tree
  src/materialize.rs       # fold ops -> state (LWW per (entity, field)), cycle repair
  src/store.rs             # SQLite (rusqlite, bundled): ops, tabs, device info, sync cursors
  src/backend/mod.rs       # trait RemoteBackend { list, get, put_new }
  src/backend/localdir.rs  # MVP backend
  src/sync.rs              # push/pull segments
  src/api.rs               # high-level facade (Repo): the only API the TUI and later FFI use
crates/cpasfait-tools/     # companion binaries, depend only on cpasfait-core
  src/bin/cpasfait-report.rs  # time breakdown between two dates
  src/bin/cpasfait-next.rs    # most prioritary task in a tab (must be fast)
crates/cpasfait-tui/       # binary `cpasfait`: ratatui + crossterm
  src/main.rs, app.rs, mode.rs, keymap.rs, action.rs, ui/{tabs,tree,detail,statusline}.rs
docs/format.md             # op-log + remote layout spec (for portability/debugging)
```
Move the current `src/main.rs` into `crates/cpasfait-tui`. Later: `crates/cpasfait-ffi` (UniFFI), `backend/gdrive.rs`, `backend/webdav.rs`.

### Data model & merge (core)
- IDs: UUIDv7 for tasks, tabs, devices. Each install has a `device_id`.
- **HLC** = `(wall_ms: u64, counter: u32, device_id)`; total order; advanced on every local op and on every received op.
- **Op** = `{ hlc, entity: Uuid, field: Field, value: Value }` — a single "set field" operation. Entity exists once any op targets it.
  Fields: `title`, `notes`, `status` (todo/done), `priority`, `due` (date/datetime), `parent` (Option<Uuid>),
  `position` (fractional-index string for sibling order), `tag:<name>` (bool, so tags merge per-tag), `deleted` (tombstone).
  Tab-level fields (e.g. tab `name`) are ops on the tab entity.
- **Work log**: each work session is its own entity (UUIDv7) with fields `task` (Uuid), `start`, `end` (Option<timestamp>),
  `user` (display name from config), `note`, `deleted`. Separate entities mean concurrent sessions by different users
  on the same task merge cleanly (nothing to conflict). Invariant enforced locally: at most one open session per device;
  starting work on a task auto-closes the current open session. Open sessions persist, so a running timer survives restarts.
  Derived per task: total time worked (own + all users), currently-active workers.
- **Materialize**: for each (entity, field) keep the value with the highest HLC → order of delivery doesn't matter (convergence).
  Concurrent moves can create parent cycles: detect during tree build and deterministically re-root the cycle member
  with the smallest id. Children of deleted tasks are hidden.
- Materialization is **incremental and persisted** (see below), not a fold on every start: applying an op = compare its
  HLC with the stored HLC of that (entity, field) and update the row if newer, in the same transaction as the op insert.
  A full rebuild from the op log (`Repo::rebuild`) exists for repair and tests (and must equal the incremental result).

### Local storage (SQLite, WAL mode)
WAL so the companion tools can read while the TUI / sync thread writes.
- Log tables: `device(id, user, hlc_state)`, `tab_seq(tab, next_seq)` (op seq is per device *and tab*, so per-tab
  segments are gap-free), `tab(id, local_name, remote_kind, remote_config_json)`,
  `op(tab_id, hlc_wall, hlc_counter, device_id, entity, field, value_json, pushed bool, PK(hlc...))`,
  `remote_cursor(tab_id, device_id, last_segment)`.
- Materialized tables (the read model): `task(id, tab_id, title, notes, status, priority, due, parent, position, deleted)`,
  `task_tag(task_id, tag)`, `work_session(id, task_id, tab_id, user, start, end, note, deleted)`, and
  `field_hlc(entity, field, hlc...)` for LWW decisions. Indexes: `task(tab_id, status, deleted)`, `work_session(start)`, `work_session(end)`.
Data dir via `directories` crate (`~/.local/share/cpasfait/cpasfait.db`, overridable by `CPASFAIT_DATA_DIR`).
Every edit = insert op + update read model in one transaction (autosave; no `:w` needed).

### Companion apps (`cpasfait-tools`)
Both read the local DB only (no network by default; `--sync` opt-in), output plain text by default and `--json` for scripting.
- **`cpasfait-next [--tab NAME|ID]`** — prints the most prioritary task. Must be fast: open DB read-only, one indexed
  query over the materialized `task` table + a small in-memory walk for ancestors; no op-log replay, no tokio, minimal deps
  (target: a few ms, fit for status bars / shell prompts). Default rule: candidates are not done, not deleted, with no
  done/deleted ancestor, and *leaves* (no open subtasks — the actionable item); order by priority desc, due asc (nulls last),
  then tree order. Rule implemented once in core (`Repo::next_task`) so the TUI can show/jump to it too (`gn`).
- **`cpasfait-report --from DATE --to DATE [--tab ..] [--user ..] [--group task|tab|user|day]`** — time breakdown:
  sessions clipped to the range (open sessions count until now), summed per task and **rolled up through the hierarchy**
  (parent total = own + descendants), shown as an indented tree with durations and percentages. Logic in core
  (`Repo::time_report(range, filters)`) so the TUI/mobile can reuse it.

### Sync
Remote layout per tab: `<root>/<tab_id>/ops/<device_id>/<first_seq>-<last_seq>.seg` (immutable, encrypted segments) and `<root>/<tab_id>/keys/<epoch>/…age` (epoch keys, age-encrypted to the members); spec in `docs/format.md`.
- **push**: unpushed local ops → one new segment file via `put_new` (local-dir: write temp + rename) → mark pushed.
- **pull**: `list` device dirs/segments, fetch segments newer than cursor, insert ops (dedupe by HLC key), advance HLC, re-materialize.
- Triggers: startup, every 60 s, 3 s after a local edit, `:sync`, and on exit. Background thread with its own DB
  connection; the UI notices its writes through `PRAGMA data_version` and refreshes.
- Post-MVP: snapshot/compaction (each device writes a snapshot + version vector; readers load latest snapshot + later segments).

### TUI (Elm-style)
`App` state + `Mode {Normal, Insert(field editor), Command(String)}`; `keymap: (Mode, KeySeq) -> Action`; `update(Action)`; `render(&App)`.
Layout: tab bar (top) · task tree (main) · detail pane for the selected task (right, toggleable) · status/command line (bottom).
Work log display: tree rows show a marker on the active task (and on tasks others are working on, after sync); the status
line shows the running task + live elapsed time (1s tick); the detail pane lists the task's work sessions (who, start, duration, note) and total.
MVP keys: `j/k` move, `h/l` collapse/expand, `gt/gT` next/prev tab, `o/O` new sibling below/above, `a` new child,
`i`/`cc` edit title, `x`/`space` toggle done, `dd` delete, `>>`/`<<` indent/outdent, `J/K` move among siblings,
`+`/`-` priority, **`s` start/stop working on the selected task** (toggle; switches the active session if another task
was running), **`S` stop whatever is running** (from anywhere), `:worknote text` (note on the current/last session), `:tag +foo -bar`, `:due 2026-12-01`, `:tabnew name`, `:tabremote <dir>`, `:sync`, `:q`. Multi-key sequences via a pending-key buffer.

### Crates
ratatui, crossterm, rusqlite (`bundled`), serde/serde_json, uuid (`v7`, `serde`), jiff (dates), thiserror (core) / anyhow (tui),
directories, tui-input (line editing), clap (tools), fractional_index (or a small hand-written impl). Dev: proptest, insta, tempfile.

## MVP milestones
1. **Scaffold**: workspace, two crates, CI-ready `cargo test`/`clippy`.
2. **Core model**: HLC, Op, materialize, cycle repair. Proptest: any permutation/duplication of an op set materializes to the same state.
3. **Store + Repo API**: SQLite persistence; `Repo` methods (create_tab, add_task, set_field, move_task, toggle_done, delete, tree(tab),
   start_work(task), stop_work(), active_session(), work_log(task)).
4. **TUI**: tabs, tree view, detail pane, modal editing + command line wired to `Repo`; `s`/`S` work tracking with live timer.
5. **Companion apps**: `Repo::next_task`, `Repo::time_report`, the two binaries (clap, `--json`).
6. **Sync**: `RemoteBackend` trait + local-dir backend + sync thread. Test: two `Repo`s (separate DBs) sharing one remote dir, concurrent edits converge.

Post-MVP (not planned in detail): Google Drive backend (OAuth device flow on desktop), UniFFI bindings + Android prototype,
WebDAV backend, recurrence, filters/search, undo (as inverse ops), compaction, config file for keymaps/colors.

## Verification
- `cargo test --workspace`: unit tests for HLC ordering, LWW, cycle repair, work log (start on B auto-stops A; totals;
  concurrent sessions from two devices on one task both survive merge); proptest convergence; store round-trip (tempfile DB);
  sync integration test with two repos + tempdir remote.
- Read model: proptest that incremental materialization == full rebuild for any op order.
- Companion apps: unit tests for next-task ordering (priority/due/leaf/done-ancestor cases) and report clipping/roll-up
  (session spanning range boundaries, open session, nested tasks); `hyperfine 'cpasfait-next --tab work'` on a
  generated DB with ~10k tasks / 100k ops to confirm it stays in the low-ms range.
- TUI: `insta` snapshots of rendered frames via ratatui `TestBackend` for key sequences (add/indent/toggle/tab switch).
- Manual: run `cargo run -p cpasfait-tui` twice with `CPASFAIT_DATA_DIR` set to two different dirs, attach both to the same remote dir,
  edit concurrently, `:sync` in both, confirm identical trees.

## Status (2026-11-20)
Milestones 1–6 are implemented. Deviations from the plan above:
- `cpasfait-next` uses `Repo::structure` (a tree without titles, notes or tags), and the tree is stored in index-based
  vectors. On a 5,000-task tab a run takes about 7 ms, of which about 1.7 ms is process startup.
- `u` only undoes deletions made in the current session. General undo is still to do.
- Sibling placement queries siblings directly instead of building the tree, so adding a task does not get slower as the tab grows.

Known limitations / next steps:
- Google Drive backend (cargo feature `gdrive`, on by default in the binaries): `backend/gdrive/` with the path
  mapping (`mod.rs`, tolerant of duplicate same-named folders), a blocking `ureq` client (`http.rs`), and OAuth
  loopback + PKCE with a user-provided "Desktop app" client (`auth.rs`). Tested against an in-memory fake and a mock
  HTTP server, not yet against the real Drive. Shared folders need the full `drive` scope: with `drive.file` a user
  cannot see a folder that another account created. While the OAuth client's consent screen is in "Testing"
  status, Google expires refresh tokens after 7 days. `cpasfait-gdrive` handles login, mkdir and share.
- Remote data is always end-to-end encrypted (`crypto.rs`, key handling in `sync.rs`; spec in `format.md`):
  per-tab epoch keys sealing segments with XChaCha20-Poly1305, distributed as age files to the members' age or SSH
  public keys, rotated when a member is removed, and chained by HMAC so the remote cannot inject a key. Members are
  `tab.member:<key>` LWW ops. Not supported yet: passphrase-protected identities and age plugins (unwrapping happens in
  the background sync thread, which cannot prompt); a member removed but still allowed to write to the remote can still
  add segments under old keys.
- rsync over SSH backend (`backend/rsync.rs`): runs the system's rsync/ssh against a local mirror; desktop only
  (Android would need a native SFTP backend against the same directory). Tested with a fake `ssh`, not a real server.
- WebDAV backend still to do. `RemoteBackend` is the extension point.
- No compaction of op logs yet.
- A segment that never arrives (for example, deleted on the remote) blocks that device's later ops for that tab, until
  that device syncs again (it uploads its missing ops again).
- `:tabmove` / `:tabaccept` run in the UI thread: a long copy (many files on Drive) freezes the TUI meanwhile.
- Concurrent moves: the last one wins, but a device that already moved elsewhere does not see a move announced on
  the old remote.
- Keymap and colors are hard-coded (`keymap.rs`).
- The first run auto-creates an `inbox` tab, so a device that only joins shared tabs also has an empty local one.
