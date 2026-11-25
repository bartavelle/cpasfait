//! Desktop notifications for tasks that fall due.

use crate::repo::DueNotice;
use std::process::Command;

/// A notification: summary line and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub summary: String,
    pub body: String,
}

/// Above this many tasks falling due at once (e.g. the first run after a
/// while), one notification lists them instead of one each.
pub const MAX_SEPARATE: usize = 3;

/// The notifications to show for `notices`.
pub fn notifications(notices: &[DueNotice]) -> Vec<Notification> {
    if notices.len() > MAX_SEPARATE {
        let mut body: Vec<String> = notices
            .iter()
            .take(10)
            .map(|n| format!("{} ({}, {})", n.title, n.due, n.tab_name))
            .collect();
        if notices.len() > 10 {
            body.push(format!("and {} more", notices.len() - 10));
        }
        return vec![Notification {
            summary: format!("{} tasks are due", notices.len()),
            body: body.join("\n"),
        }];
    }
    notices
        .iter()
        .map(|n| Notification {
            summary: format!("Due: {}", n.title),
            body: format!("{} · {}", n.due, n.tab_name),
        })
        .collect()
}

/// Show a notification that stays until dismissed, with `notify-send`
/// (libnotify): critical urgency, no expiry.
pub fn send(n: &Notification) -> crate::Result<()> {
    let out = Command::new("notify-send")
        .args(["--app-name=cpasfait", "--urgency=critical", "--expire-time=0"])
        .args(["--icon=appointment-soon", "--"])
        .arg(&n.summary)
        .arg(&n.body)
        .output()
        .map_err(|e| crate::Error::Invalid(format!("notify-send: {e} (install libnotify-bin?)")))?;
    if !out.status.success() {
        return Err(crate::Error::Invalid(format!(
            "notify-send: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}
