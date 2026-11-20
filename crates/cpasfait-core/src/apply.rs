//! Applying ops to the read model (incremental materialization).
//!
//! For each `(entity, field)` the op with the highest HLC wins. Applying the
//! same set of ops in any order, any number of times, gives the same state.

use crate::error::Result;
use crate::hlc::Hlc;
use crate::ids::DeviceId;
use crate::op::{Change, Op};
use rusqlite::{Connection, OptionalExtension, params};

/// Insert `op` into the log and update the read model. Returns false if the op
/// was already known.
pub(crate) fn apply_op(conn: &Connection, op: &Op, pushed: bool) -> Result<bool> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO op(hlc_wall, hlc_counter, device, seq, tab, entity, field, value, pushed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            op.hlc.wall,
            op.hlc.counter,
            op.hlc.device.to_string(),
            op.seq as i64,
            op.tab.to_string(),
            op.entity.to_string(),
            op.field,
            serde_json::to_string(&op.value)?,
            pushed,
        ],
    )?;
    if inserted == 0 {
        return Ok(false);
    }
    apply_to_model(conn, op)?;
    Ok(true)
}

/// LWW-apply an op (already in the log) to the read model.
pub(crate) fn apply_to_model(conn: &Connection, op: &Op) -> Result<()> {
    let entity = op.entity.to_string();
    let current: Option<(i64, u32, String)> = conn
        .query_row(
            "SELECT hlc_wall, hlc_counter, device FROM field_hlc WHERE entity = ?1 AND field = ?2",
            params![entity, op.field],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((wall, counter, device)) = current {
        let device: DeviceId = device.parse().map_err(|_| crate::Error::Invalid("device id".into()))?;
        if (Hlc { wall, counter, device }) >= op.hlc {
            return Ok(());
        }
    }
    conn.execute(
        "INSERT INTO field_hlc(entity, field, hlc_wall, hlc_counter, device) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(entity, field) DO UPDATE SET hlc_wall = ?3, hlc_counter = ?4, device = ?5",
        params![entity, op.field, op.hlc.wall, op.hlc.counter, op.hlc.device.to_string()],
    )?;
    let Some(change) = Change::parse(&op.field, &op.value) else {
        return Ok(()); // unknown field: kept in the log, no local effect
    };
    let tab = op.tab.to_string();
    let ensure_task = || {
        conn.execute(
            "INSERT OR IGNORE INTO task(id, tab) VALUES (?1, ?2)",
            params![entity, tab],
        )
    };
    let ensure_session = || {
        conn.execute(
            "INSERT OR IGNORE INTO session(id, tab) VALUES (?1, ?2)",
            params![entity, tab],
        )
    };
    let ensure_tab = || conn.execute("INSERT OR IGNORE INTO tab(id) VALUES (?1)", params![entity]);
    macro_rules! set {
        ($ensure:ident, $table:literal, $col:literal, $val:expr) => {{
            $ensure()?;
            conn.execute(
                concat!("UPDATE ", $table, " SET ", $col, " = ?2 WHERE id = ?1"),
                params![entity, $val],
            )?;
        }};
    }
    use Change::*;
    match change {
        TabName(s) => set!(ensure_tab, "tab", "name", s),
        TabDeleted(b) => set!(ensure_tab, "tab", "deleted", b),
        TaskTitle(s) => set!(ensure_task, "task", "title", s),
        TaskNotes(s) => set!(ensure_task, "task", "notes", s),
        TaskStatus(s) => set!(ensure_task, "task", "status", s.as_str()),
        TaskPriority(p) => set!(ensure_task, "task", "priority", p),
        TaskDue(d) => set!(ensure_task, "task", "due", d.map(|d| d.to_string())),
        TaskParent(p) => set!(ensure_task, "task", "parent", p.map(|p| p.to_string())),
        TaskPosition(s) => set!(ensure_task, "task", "position", s),
        TaskDeleted(b) => set!(ensure_task, "task", "deleted", b),
        TaskTag { tag, on } => {
            ensure_task()?;
            if on {
                conn.execute(
                    "INSERT OR IGNORE INTO task_tag(task, tag) VALUES (?1, ?2)",
                    params![entity, tag],
                )?;
            } else {
                conn.execute(
                    "DELETE FROM task_tag WHERE task = ?1 AND tag = ?2",
                    params![entity, tag],
                )?;
            }
        }
        SessionTask(t) => set!(ensure_session, "session", "task", t.to_string()),
        SessionDevice(d) => set!(ensure_session, "session", "device", d.to_string()),
        SessionUser(s) => set!(ensure_session, "session", "user", s),
        SessionStart(t) => set!(ensure_session, "session", "start", t),
        SessionEnd(t) => set!(ensure_session, "session", "end", t),
        SessionNote(s) => set!(ensure_session, "session", "note", s),
        SessionDeleted(b) => set!(ensure_session, "session", "deleted", b),
    }
    Ok(())
}
