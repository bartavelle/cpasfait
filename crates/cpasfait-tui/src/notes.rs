//! A task's notes as a text file (`:notes`), to fix or delete them in an editor:
//!
//! ```text
//! # Notes on "Write the report", newest first. ...
//! @@ 1 · 2026-11-27 14:30 · alice
//! Sent the draft to Bob.
//!
//! @@ 2 · 2026-11-26 09:12 · bob
//! Waiting for the figures.
//! ```
//!
//! Blocks are matched back to the notes by their number: a changed text
//! updates the note, a removed block (or an emptied one) deletes it.

use anyhow::{Result, bail};
use cpasfait_core::{Note, NoteId};
use std::collections::HashMap;

const HEADER: &str = "@@ ";

pub fn render(title: &str, notes: &[Note], when: impl Fn(i64) -> String) -> String {
    let mut out = format!(
        "# Notes on {title:?}, newest first. Change a note's text, or delete its block\n\
         # (from its @@ line) to delete it. These first lines are ignored.\n"
    );
    for (i, n) in notes.iter().enumerate() {
        out.push_str(&format!("\n{HEADER}{} · {} · {}\n", i + 1, when(n.at), n.author.name));
        out.push_str(&n.text);
        out.push('\n');
    }
    out
}

/// The texts of the blocks, by number (1-based).
pub fn parse(text: &str) -> Result<HashMap<usize, String>> {
    let mut blocks: HashMap<usize, String> = HashMap::new();
    let mut current: Option<(usize, String)> = None;
    let close = |c: Option<(usize, String)>, blocks: &mut HashMap<usize, String>| -> Result<()> {
        if let Some((n, t)) = c
            && blocks.insert(n, t.trim().to_owned()).is_some()
        {
            bail!("note {n} appears twice");
        }
        Ok(())
    };
    for (i, line) in text.lines().enumerate() {
        if let Some(rest) = line.strip_prefix(HEADER) {
            close(current.take(), &mut blocks)?;
            let num = rest.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("");
            let n = num
                .parse()
                .map_err(|_| anyhow::anyhow!("line {}: a note header must start with its number", i + 1))?;
            current = Some((n, String::new()));
        } else if let Some((_, t)) = &mut current {
            t.push_str(line);
            t.push('\n');
        }
    }
    close(current, &mut blocks)?;
    Ok(blocks)
}

/// What to do with `notes` (as rendered) given the edited file: new texts
/// (empty: delete).
pub fn changes(notes: &[Note], edited: &str) -> Result<Vec<(NoteId, String)>> {
    let mut blocks = parse(edited)?;
    let mut out = vec![];
    for (i, n) in notes.iter().enumerate() {
        match blocks.remove(&(i + 1)) {
            Some(t) if t == n.text => {}
            Some(t) => out.push((n.id, t)),
            None => out.push((n.id, String::new())),
        }
    }
    if let Some(n) = blocks.keys().min() {
        bail!("there is no note {n}: add notes with :note");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpasfait_core::{Author, TabId};

    fn note(text: &str, at: i64) -> Note {
        Note {
            id: NoteId::new(),
            tab: TabId::new(),
            task: None,
            at,
            author: Author {
                name: "alice".into(),
                email: None,
            },
            text: text.into(),
            deleted: false,
        }
    }

    #[test]
    fn roundtrip_and_changes() {
        let notes = vec![note("second\n\nwith a paragraph", 2), note("first", 1)];
        let text = render("t", &notes, |ms| format!("T{ms}"));
        assert!(
            text.contains("@@ 1 · T2 · alice\nsecond\n\nwith a paragraph\n"),
            "{text}"
        );
        assert!(changes(&notes, &text).unwrap().is_empty());

        let edited = text.replace("first", "first, fixed");
        assert_eq!(
            changes(&notes, &edited).unwrap(),
            vec![(notes[1].id, "first, fixed".into())]
        );

        let cut = text.split("\n@@ 2").next().unwrap();
        assert_eq!(changes(&notes, cut).unwrap(), vec![(notes[1].id, String::new())]);

        assert!(changes(&notes, &format!("{text}@@ 1\nagain\n")).is_err());
        assert!(changes(&notes, &format!("{text}@@ 7\nnew\n")).is_err());
        assert!(changes(&notes, &format!("{text}@@ x\n")).is_err());
    }
}
