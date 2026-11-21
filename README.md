# cpasfait

A terminal todo app with vim-like keys. It has nested tasks, tabs, a work log
(time tracking), and conflict-free sync of shared tabs between several people
and devices.

## Crates

| crate | what |
|---|---|
| `cpasfait-core` | model, op log, SQLite storage, sync. No UI. Meant to be shared with a future mobile app (UniFFI). |
| `cpasfait-tui` | the `cpasfait` terminal UI |
| `cpasfait-tools` | `cpasfait-next`, `cpasfait-report`, `cpasfait-key` and `cpasfait-gdrive` |

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

The configuration file is `~/.config/cpasfait/config.toml` (or `$CPASFAIT_CONFIG`;
when `CPASFAIT_DATA_DIR` is set, `config.toml` in that directory, so that each
data dir is a separate profile). When it is missing, `cpasfait` asks for your
email before starting and creates it:

```toml
email = "you@example.org"   # recorded as the author of the tasks you create
# name = "Your Name"        # display name; overrides the one set with :user
# identity = "~/.ssh/id_ed25519"   # secret key for shared tabs (see below)
```

Each task records its author (name and email, shown in the details pane).
In shared tabs the other members see it.

Press `?` in the TUI for all keys. The essentials:

- `j`/`k` move, `h`/`l` fold or unfold, `n`/`a` add a task / a subtask, `i` edit, `x` done, `dd` delete
- Tasks are sorted by score (see below) among their siblings, done ones last: `+`/`-` and `:due` move them
- `>>`/`<<` indent or outdent, `+`/`-` priority
- `s` start or stop working on the task; `S` stops whatever is running
- `gn` jumps to the next task, the same one `cpasfait-next` prints
- `Ctrl-P` or `/` finds a task by fuzzy search on titles across all tabs (`↑`/`↓` to choose, `Enter` to go)
- `T` shows time spent per task in a box at the lower right, cycling through hidden, today, this week and this month
- `:due fri`, `:tag +home -work`, `:tabnew Work`, `gt`/`gT` switch tabs
- `:tabset` shows the tab's settings, `:tabset default_priority 2` changes one, `:tabunset default_priority` resets it. Settings are shared with the tab's members. For now there is one: `default_priority`, the priority of new tasks in the tab (default 0)
- `:require` / `:block` pick, in the finder, a task (any tab) this one needs or holds up (`:unrequire`, `:unblock` undo it); blocked tasks show `⊘blocked` and are never the next task

### Sharing a tab

Everything a shared tab puts on the remote is end-to-end encrypted to the
members' keys: the remote (Google, a sync service) sees only random-looking
files and their sizes and dates. Keys are [age](https://age-encryption.org)
keys; an SSH key works too.

```
:mykey                         # on the other person's device: show your public key (age1…)
:tabremote ~/Sync/cpasfait     # on the first device: share the current tab through a directory
:tabinvite age1… Bob           # give Bob access (an `ssh-ed25519 AAAA…` key works too)
:tabjoin ~/Sync/cpasfait       # on Bob's devices: join it
:tabmembers                    # who has access
:tabrevoke Bob                 # remove Bob: a new key is used from the next sync on
```

The directory can be anything that some tool keeps in sync between machines
(Syncthing, Google Drive desktop, Dropbox, a network share). Every device only
ever creates new files there and never edits them, so concurrent use is safe.
The TUI syncs at startup, every minute, a few seconds after an edit, on `:sync`
and on exit.

Keys:

- Your secret key is the first found of `$CPASFAIT_IDENTITY` (an age identity
  file, or an unencrypted SSH private key), `identity` in the configuration
  file (`~/` is your home, a relative path is relative to the configuration
  file), `<data dir>/identity.txt`, and age's `~/.config/age/keys.txt` (only
  when `CPASFAIT_DATA_DIR` is unset). The first two are used even if the file
  is missing: that is an error, not a reason to use another key. If none is
  set or found, cpasfait creates
  `<data dir>/identity.txt` (also `cpasfait-key`, which prints your public key).
  **Back it up**: without it, a new device cannot read your shared tabs (the
  data on your existing devices is not affected; another member can also
  invite a new key).
- Use the same key on all your devices, or invite each device's key.
- Joining before being invited is fine: the tab fills in once a member has
  run `:tabinvite` and synced.
- A new member can read the whole history. A removed member keeps what they
  already had, but cannot read anything written after their removal. Also
  stop sharing the remote folder with them: as long as they can still write
  there, they can still add changes.
- Passphrase-protected identities and age plugins (YubiKey…) are not
  supported yet.

### Moving a shared tab to another remote

```
:tabmove rsync:me@newbox:cpasfait     # copies everything there, then switches to it
```

The move is announced to the other members through the old remote: their tab
shows `⇅↪` and a message. `:tabaccept` opens the command line with the new
remote filled in: press Enter to follow it as is, or edit it first (the same
place may have another path on their machine, or need their own SSH login).
Changes made on the old remote in the meantime are carried over when they
follow. A path where the tab cannot be found is refused. Once everyone has
followed, the old remote can be deleted.

### Sharing through rsync over SSH

Any machine you can `ssh` into and that has `rsync` (a VPS, a NAS, a
friend's server) can hold shared tabs; its owner cannot read them.

```
:tabremote rsync:me@box.example.org:cpasfait    # path relative to the home directory, or absolute
:tabjoin rsync:friend@box.example.org:/srv/cpasfait/shared
```

- cpasfait runs the system's `rsync` and `ssh`, so `~/.ssh/config` (host
  aliases, ports, jump hosts), ssh-agent and `known_hosts` all apply.
- Sync runs in the background and cannot ask for a passphrase: load your key
  into ssh-agent (or use one without a passphrase), and connect once by hand
  so the host key is known.
- Every member needs an account (or a shared one) with write access to the
  directory.
- A copy of the remote, encrypted, is kept in `<data dir>/mirrors/`; it can
  be deleted at any time.

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
4. In the TUI: `:tabremote gdrive:ID` and `:tabinvite KEY` for each other
   member on the first device, `:tabjoin gdrive:ID` (a folder URL works too)
   on the others.

Notes:

- The default scope is full Drive access, because the narrower `drive.file`
  scope cannot see folders created by another account. Use
  `login --scope file` if a single Google account uses the remote.
- While the consent screen is in "Testing" status, Google expires the
  authorization after 7 days; run `cpasfait-gdrive login` again (the client is
  remembered). Publishing the app, or a Workspace "Internal" app, avoids this.

### Scores

Each open task has a score, shown in the details pane:

- `priority × 100`, so priority matters most;
- a due date adds `10 + 80 / (1 + days left / 7)`: 90 when due today or
  overdue, 50 a week ahead, about 25 a month ahead. At equal priority a task
  with a deadline beats one without, and gets more urgent as it approaches,
  but never by a whole priority level;
- a task that blocks others adds the scores of the open tasks that require it
  (transitively, across tabs), so whatever unblocks important work comes first.

Within each level of the tree, tasks are listed by decreasing score (earlier due date first on ties, then creation order), done tasks last. The next task (`gn`, `cpasfait-next`) is the actionable task with the highest score.

### Companion tools

`cpasfait-next [--tab T] [-p] [-l] [--json]` prints the most important task:
not done, without open subtasks, not blocked by an unfinished requirement,
highest score first, then earliest due date, then tree order. Exit status is 2 when there is nothing to do. It opens
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
