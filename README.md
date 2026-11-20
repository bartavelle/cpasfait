# cpasfait

A terminal todo app with vim-like keys. It has nested tasks, tabs, a work log
(time tracking), and conflict-free sync of shared tabs between several people
and devices.

## Crates

| crate | what |
|---|---|
| `cpasfait-core` | model, op log, SQLite storage, sync. No UI. Meant to be shared with a future mobile app (UniFFI). |
| `cpasfait-tui` | the `cpasfait` terminal UI |
| `cpasfait-tools` | `cpasfait-next`, `cpasfait-report` and `cpasfait-gdrive` |

See `docs/architecture.md` for the design and `docs/format.md` for the data and sync format.

## Usage

```
cargo run --release -p cpasfait-tui            # the TUI (binary: cpasfait)
cargo run --release -p cpasfait-tools --bin cpasfait-next -- --tab work
cargo run --release -p cpasfait-tools --bin cpasfait-report -- --from -1w --by day
```

The database is `$CPASFAIT_DATA_DIR/cpasfait.db`, or by default
`~/.local/share/cpasfait/cpasfait.db`. Every change is saved immediately.
`CPASFAIT_USER` sets the display name on first run; `:user NAME` changes it later.

Press `?` in the TUI for all keys. The essentials:

- `j`/`k` move, `h`/`l` fold or unfold, `o`/`O`/`a` add below/above/child, `i` edit, `x` done, `dd` delete
- `>>`/`<<` indent or outdent, `J`/`K` reorder, `+`/`-` priority
- `s` start or stop working on the task; `S` stops whatever is running
- `gn` jumps to the next task, the same one `cpasfait-next` prints
- `:due fri`, `:tag +home -work`, `:tabnew Work`, `gt`/`gT` switch tabs

### Sharing a tab

```
:tabremote ~/Sync/cpasfait     # on the first device: attach the current tab to a directory
:tabjoin ~/Sync/cpasfait       # on the other devices: join it
```

The directory can be anything that some tool keeps in sync between machines
(Syncthing, Google Drive desktop, Dropbox, a network share). Every device only
ever creates new files there and never edits them, so concurrent use is safe.
The TUI syncs at startup, every minute, a few seconds after an edit, on `:sync`
and on exit.

### Sharing through Google Drive

cpasfait talks to the Drive API directly. It has no Google-verified app, so
each user needs an OAuth client of their own (or a shared one from whoever
set things up):

1. In the [Google Cloud console](https://console.cloud.google.com/), create a
   project, enable the **Google Drive API**, configure the OAuth consent
   screen (add yourself and the other users as test users), and create an
   OAuth client of type **Desktop app**. Download its JSON.
2. `cpasfait-gdrive login --client client_secret.json` opens a browser and
   stores the credentials in `<data dir>/gdrive.json`.
3. `cpasfait-gdrive mkdir cpasfait` creates a folder and prints `gdrive:ID`;
   `cpasfait-gdrive share gdrive:ID friend@example.com` gives someone write access.
4. In the TUI: `:tabremote gdrive:ID` on the first device, `:tabjoin gdrive:ID`
   (a folder URL works too) on the others.

Notes:

- The default scope is full Drive access, because the narrower `drive.file`
  scope cannot see folders created by another account. Use
  `login --scope file` if a single Google account uses the remote.
- While the consent screen is in "Testing" status, Google expires the
  authorization after 7 days; run `cpasfait-gdrive login` again (the client is
  remembered). Publishing the app, or a Workspace "Internal" app, avoids this.

### Companion tools

`cpasfait-next [--tab T] [-p] [-l] [--json]` prints the most important task:
not done, without open subtasks, highest priority first, then earliest due
date, then tree order. Exit status is 2 when there is nothing to do. It opens
the database read-only and takes a few milliseconds, so it suits status bars.

`cpasfait-report [--from D] [--to D] [--tab T] [--user U] [--by task|tab|user|day] [--json]`
shows where the time went. Subtask time is rolled up into parents, and running
sessions count up to now. Dates accept `2026-11-01`, `today`, `yesterday`,
`-1w`, `mon`, and so on.

## Development

```
cargo test --workspace           # includes property tests checking that replicas converge
cargo clippy --workspace --all-targets
cargo run --release -p cpasfait-tools --example gen_db -- /tmp/big.db 10000   # benchmark data
```
