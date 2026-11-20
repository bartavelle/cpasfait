use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 3;

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
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(version >= 1)
}
