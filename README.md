# cpasfait

A terminal todo app with vim-like keys. It has nested tasks, tabs, a work log
(time tracking), and conflict-free sync of shared tabs between several people
and devices.

## Crates

| crate | what |
|---|---|
| `cpasfait-core` | model, op log, SQLite storage, sync. No UI. |
| `cpasfait-tui` | the `cpasfait` terminal UI |
| `cpasfait-tools` | `cpasfait-next`, `cpasfait-report`, `cpasfait-notify`, `cpasfait-key` and `cpasfait-gdrive` |
| `cpasfait-ffi` | the core for the mobile apps (Kotlin bindings with UniFFI): see its [README](crates/cpasfait-ffi/README.md) |

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
- `s` start or stop working on the task (starting one stops the other)
- Forgot to stop or switch? `S` stops what is running and `W` switches to the selected task, at a time you give: they open the command line prefilled with the current time (`:stop 14:32`, `:switch 14:32`), Enter to accept or edit it first. Times: `14:30` (a time not reached yet today is yesterday's), `-20m`, `-1h30`, `yesterday 18:00`, `2026-11-22 18:00`. With nothing running, `W` starts the task back-dated (not before your last session ended)
- `E` (or `:edit`) opens the whole task in `$VISUAL`/`$EDITOR`, like `task edit`: its fields, description, notes, and work log of the last 30 days (`:edit all` for the whole log, `:edit 2026-10-01` or `:edit -3m` from a date; older sessions are not shown and stay as they are):

  ```
  title: Write the report
  status: todo
  priority: 1
  due: 2026-12-01
  # tags: TAG OTHER-TAG
  # requires: TASK-ID

  description:
  Quarterly report, figures from the finance team.

  notes:
  @@ 1 · 2026-11-27 14:30 · alice
  Sent the draft to Bob.

  work log:
  2026-11-20 09:00:00 - 2026-11-20 10:30:00
  2026-11-23 14:00:00 -                      # running
  ```

  Optional fields that are unset are shown commented out, with an example value (the due date is now): uncomment one to set it, comment it out or delete it to unset it. Due dates can be relative (`tomorrow`, `fri 9:00`); `requires` takes a task id per line. The description and the notes are kept as written (`#` lines included). Change a note to correct it, delete its block (from its `@@` line) to delete it, and add one with a line `@@` followed by its text. In the work log, correct a line to fix a session, delete it to delete the session, add one to log time (seconds are optional; a running session has no end). Overlapping sessions are merged into one (sessions that only touch stay apart). Times are local, `#` starts a comment, and other members' sessions are labelled with their name. A section left out entirely (description, notes or work log) is left alone. On save, all the changes are applied, or none: on an error (shown in the status line) pressing `E` again reopens your text with the error on top. An empty file, or an editor exiting with an error (vim's `:cq`), cancels
- `N` (or `:note TEXT`) adds a note to the task: a status update, dated and signed with your name, shown newest first in the details pane (the 5 latest). Notes written by several people at once all survive. Longer notes, and fixing or deleting notes, happen in `E` (`:note` alone opens it). `e` edits just the task's description
- `:` remembers the commands you run (`↑`/`↓`, only those starting with what is typed; kept next to the database) and completes with `Tab`: commands, tags, tab names, members, settings, and remotes you used before
- `gn` jumps to the next task, the same one `cpasfait-next` prints
- `Ctrl-P` or `/` finds a task by fuzzy search on titles across all tabs (`↑`/`↓` to choose, `Enter` to go)
- The details pane (`D`) shows the task's total time, today's and this week's, and its 8 latest sessions
- `T` shows time spent per task, in all tabs (other tabs' tasks are marked `[tab]`; deleted tasks and their subtasks are left out), in a box at the lower right, cycling through hidden, today, this week and this month
- `:due fri`, `:tag +home -work`, `:tabnew Work`, `gt`/`gT` switch tabs
- Due dates can have a time: `:due 17:30` (today), `:due fri 9:00`, `:due tom 8:15:30`, `:due 2026-12-01 14:30`; `:due none` removes it. Times are local: in a tab shared across time zones, everyone sees the same clock time
- When a task falls due (at its time, or at midnight for a date alone) the TUI shows a desktop notification that stays until dismissed (`notify-send`, critical urgency), once per due date; more than 3 at once (e.g. after a while away) make a single one. Done and deleted tasks are skipped. `notifications = false` in the configuration turns them off. Without the TUI running, `cpasfait-notify --watch` does the same (see below)
- `:tabset` shows the tab's settings, `:tabset default_priority 2` changes one, `:tabunset default_priority` resets it. Settings are shared with the tab's members. For now there is one: `default_priority`, the priority of new tasks in the tab (-3 to 3, default 0)
- `:moveto TAB` (or `:mv`) moves the task, with its subtasks, metadata and work log, to the end of another tab. Tasks belong to their tab's op log (and its encryption key and members), so the move copies them there under new ids and deletes the originals; their sessions move with them (time is counted once, a running session keeps running), and tasks that required them follow the copies. Members of the old tab who are not in the new one no longer see the task. It is refused while someone else is working on one of the tasks
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
- A key restricted to rsync works: with
  `command="/usr/bin/rrsync /srv/cpasfait",restrict ssh-ed25519 AAAA…` in the
  server's `authorized_keys`, paths are relative to `/srv/cpasfait`
  (`rsync:cpasfait@box:` for that directory itself, `rsync:cpasfait@box:team`
  for a subdirectory, created if missing). Only one missing level can be
  created through rrsync. rrsync allows only one transfer at a time and
  refuses the others ("Another instance of rrsync is already accessing this
  directory"): cpasfait retries a few times, but with several members or
  tabs on one directory, add `-no-lock` (`command="/usr/bin/rrsync -no-lock
  /srv/cpasfait",restrict …`). That is safe: cpasfait only ever adds new
  files there, never changes or deletes one. To check such a remote by hand (`ssh` itself is
  refused): `rsync --list-only -e "ssh -o BatchMode=yes" cpasfait@box:./`

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

   To spare the other users this step, build the client into the binaries:
   put the JSON at `gdrive-client.json` in the workspace root (ignored by
   git), or point `CPASFAIT_GDRIVE_CLIENT` to it, then build. Users of that
   build just run `cpasfait-gdrive login` (they must still be test users of
   your consent screen). `cpasfait-gdrive status` shows the built-in client.
   A desktop OAuth client is not really secret (Google expects it to ship
   inside apps), but avoid publishing it.
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

- `(priority + 3) × 100`, so priority matters most. Priorities go from -3
  to 3 (`+`/`-`, `:prio N`), 0 by default (300 points): shown as `!`…`!!!`
  above 0, and as a dimmed `↓`…`↓↓↓` below (for things that can wait).
  Scores are never negative, so a task always scores at least as much as
  any task it blocks (see below);
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

`cpasfait-notify [--watch] [--interval SECS] [--print]` announces the tasks
that have fallen due, like the TUI does; the two share the record of what was
announced, so running both is fine. As a systemd user service,
`~/.config/systemd/user/cpasfait-notify.service`:

```ini
[Unit]
Description=cpasfait due-date notifications
After=graphical-session.target
PartOf=graphical-session.target

[Service]
ExecStart=%h/.cargo/bin/cpasfait-notify --watch
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

then `systemctl --user enable --now cpasfait-notify`. It reads the local
database only: due dates set on other devices arrive when the TUI syncs.

`cpasfait-report [PERIOD | --from D --to D] [--by …] [--me | --user U] [--tab T] [--json]`
shows where the time went, across all tabs (or one with `--tab`):

```
cpasfait-report last-week --by tab,task,tags --me
```

- `PERIOD`: `today`, `yesterday`, `this-week`, `last-week` (weeks start on
  Monday), `this-month`, `last-month`, `this-year`, `last-year`, or an ISO
  week: `week 23` (this year), `week 23 2025`, `w23`, `2025-W23`. Without one,
  `--from`/`--to` (default: the last 7 days); dates accept `2026-11-01`,
  `today`, `yesterday`, `-1w`, `mon`, and so on.
- `--by` (comma-separated, one section each): `task` (default; subtask time
  rolled up into parents), `tab`, `tags` (per combination of tags, e.g.
  `#acme + #dev`), `tag` (per tag; a task counts for each of its tags, so these
  add up to more than the total), `user`, `day`. A task has its ancestors'
  tags too: tag a project's parent task and all its subtasks count for it.
- `--me`: only your time; shared tabs also hold the other members' sessions.
- Running sessions count up to now.

## Development

```
cargo test --workspace           # includes property tests checking that replicas converge
cargo clippy --workspace --all-targets
cargo run --release -p cpasfait-tools --example gen_db -- /tmp/big.db 10000   # benchmark data
```
