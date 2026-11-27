use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 9;

const SCHEMA_V1: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);

-- This install. Exactly one row. The clock and sequence live here so that
-- several processes on the same database never hand out duplicate stamps.
CREATE TABLE device(
    id TEXT PRIMARY KEY,
    user TEXT NOT NULL,
    hlc_wall INTEGER NOT NULL,
    hlc_counter INTEGER NOT NULL
);

-- Next op sequence number of this device, per tab (segments are per tab and
-- must be gap-free).
CREATE TABLE tab_seq(tab TEXT PRIMARY KEY, next_seq INTEGER NOT NULL) WITHOUT ROWID;

-- The op log (source of truth).
CREATE TABLE op(
    hlc_wall INTEGER NOT NULL,
    hlc_counter INTEGER NOT NULL,
    device TEXT NOT NULL,
    seq INTEGER NOT NULL,
    tab TEXT NOT NULL,
    entity TEXT NOT NULL,
    field TEXT NOT NULL,
    value TEXT NOT NULL,
    pushed INTEGER NOT NULL,
    PRIMARY KEY(hlc_wall, hlc_counter, device)
);
CREATE UNIQUE INDEX op_device_seq ON op(device, tab, seq);
CREATE INDEX op_unpushed ON op(tab, pushed, device);

-- Read model. Everything below can be rebuilt from `op`, except tab.remote.
CREATE TABLE field_hlc(
    entity TEXT NOT NULL,
    field TEXT NOT NULL,
    hlc_wall INTEGER NOT NULL,
    hlc_counter INTEGER NOT NULL,
    device TEXT NOT NULL,
    PRIMARY KEY(entity, field)
) WITHOUT ROWID;

CREATE TABLE tab(
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    deleted INTEGER NOT NULL DEFAULT 0,
    remote TEXT -- local-only JSON RemoteConfig
);

CREATE TABLE task(
    id TEXT PRIMARY KEY,
    tab TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'todo',
    priority INTEGER NOT NULL DEFAULT 0,
    due TEXT,
    parent TEXT,
    position TEXT NOT NULL DEFAULT '',
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX task_tab ON task(tab, deleted, status);

CREATE TABLE task_tag(task TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY(task, tag)) WITHOUT ROWID;

CREATE TABLE session(
    id TEXT PRIMARY KEY,
    tab TEXT NOT NULL,
    task TEXT,
    device TEXT,
    user TEXT NOT NULL DEFAULT '',
    start INTEGER,
    end INTEGER,
    note TEXT NOT NULL DEFAULT '',
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX session_task ON session(task);
CREATE INDEX session_start ON session(start);
CREATE INDEX session_open ON session(device, end);

-- Sync progress: highest op seq of `device` ingested for `tab`.
CREATE TABLE remote_cursor(
    tab TEXT NOT NULL,
    device TEXT NOT NULL,
    last_seq INTEGER NOT NULL,
    PRIMARY KEY(tab, device)
) WITHOUT ROWID;
"#;

/// Task dependencies: `task` is blocked by (requires) `blocker`.
const SCHEMA_V2: &str = r#"
CREATE TABLE task_dep(task TEXT NOT NULL, blocker TEXT NOT NULL, PRIMARY KEY(task, blocker)) WITHOUT ROWID;
CREATE INDEX task_dep_blocker ON task_dep(blocker);
"#;

/// The blocker's tab (dependencies may cross tabs).
const SCHEMA_V3: &str = "ALTER TABLE task_dep ADD COLUMN blocker_tab TEXT;";

/// Encryption of shared tabs.
const SCHEMA_V4: &str = r#"
-- Read model: members of shared tabs (name NULL = removed).
CREATE TABLE tab_member(
    tab TEXT NOT NULL,
    recipient TEXT NOT NULL,
    name TEXT,
    PRIMARY KEY(tab, recipient)
) WITHOUT ROWID;

-- Local only (not rebuilt from ops): the epoch keys this device holds.
CREATE TABLE tab_epoch(
    tab TEXT NOT NULL,
    epoch TEXT NOT NULL,
    key BLOB NOT NULL,
    prev TEXT,
    depth INTEGER NOT NULL,
    PRIMARY KEY(tab, epoch)
) WITHOUT ROWID;

-- Local only: remote key files already examined (so they are downloaded once).
-- status: 'valid' (then epoch/recipients/exclude are set), 'foreign' (not
-- encrypted to us), 'invalid'.
CREATE TABLE key_file(
    tab TEXT NOT NULL,
    path TEXT NOT NULL,
    status TEXT NOT NULL,
    epoch TEXT,
    recipients TEXT,
    exclude TEXT,
    PRIMARY KEY(tab, path)
) WITHOUT ROWID;
"#;

/// Moves to another remote: the latest move (read model), and the move this
/// device followed or made (local).
const SCHEMA_V5: &str = "
ALTER TABLE tab ADD COLUMN move_id TEXT;
ALTER TABLE tab ADD COLUMN move_remote TEXT;
ALTER TABLE tab ADD COLUMN move_by TEXT;
ALTER TABLE tab ADD COLUMN move_done TEXT;
";

/// Tab settings (read model): JSON values, absent = default.
const SCHEMA_V6: &str = "
CREATE TABLE tab_setting(
    tab TEXT NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY(tab, name)
) WITHOUT ROWID;
";

/// Task authors (read model), and this device's email (local).
const SCHEMA_V7: &str = "
ALTER TABLE task ADD COLUMN author_name TEXT;
ALTER TABLE task ADD COLUMN author_email TEXT;
ALTER TABLE device ADD COLUMN email TEXT;
";

/// Due dates already announced by a desktop notification (local, not
/// synced): `due` is the task's due date at the time, so a new one is
/// announced again.
const SCHEMA_V8: &str = "
CREATE TABLE due_notified(task TEXT PRIMARY KEY, due TEXT NOT NULL) WITHOUT ROWID;
";

/// Dated notes on tasks (read model).
const SCHEMA_V9: &str = "
CREATE TABLE note(
    id TEXT PRIMARY KEY,
    tab TEXT NOT NULL,
    task TEXT,
    at INTEGER,
    author_name TEXT,
    author_email TEXT,
    text TEXT NOT NULL DEFAULT '',
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX note_task ON note(task);
";

/// Bring the schema up to date. Returns true when the read model must be
/// rebuilt from the op log (a new table may interpret ops already received).
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<bool> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(false);
    }
    let tx = conn.transaction()?;
    if version < 1 {
        tx.execute_batch(SCHEMA_V1)?;
    }
    if version < 2 {
        tx.execute_batch(SCHEMA_V2)?;
    }
    if version < 3 {
        tx.execute_batch(SCHEMA_V3)?;
    }
    if version < 4 {
        tx.execute_batch(SCHEMA_V4)?;
    }
    if version < 5 {
        tx.execute_batch(SCHEMA_V5)?;
    }
    if version < 6 {
        tx.execute_batch(SCHEMA_V6)?;
    }
    if version < 7 {
        tx.execute_batch(SCHEMA_V7)?;
    }
    if version < 8 {
        tx.execute_batch(SCHEMA_V8)?;
    }
    if version < 9 {
        tx.execute_batch(SCHEMA_V9)?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(version >= 1)
}
