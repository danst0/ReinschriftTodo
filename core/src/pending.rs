//! Writes that are already on screen but not yet stored.
//!
//! The GUI applies a change to its list right away and hands the write to a
//! background queue, so the window never waits for the network. Until the write
//! has landed, the change exists only in memory: closing the app, a crash or a
//! logout would lose it without a trace. So every queued operation is recorded
//! in a small [`Journal`] file first and crossed off once it was stored; what is
//! still in the journal at the next start is replayed.
//!
//! Operations are semantic ("complete ^abc"), not file snapshots. They are
//! applied to the file as it is *then*, so replaying one never rolls back what
//! other devices wrote in between, and applying one twice does no harm.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::conflict::ConflictError;
use crate::todo::{
    assign_project_context_batch, delete_todos, insert_line, set_due_batch, set_myday_today,
    toggle_todos, unset_myday, update_todo_details, DueTarget, InsertMode,
};
use crate::types::{TodoItem, TodoKey};

/// How often an operation is re-applied after another writer got in between
/// its read and its write. Each attempt reads the file afresh, so a retry
/// applies the change on top of the other writer's, never over it.
const CONFLICT_ATTEMPTS: usize = 3;

/// A change to the todo file, described by what it does rather than by the
/// content it produces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PendingOp {
    SetDone { keys: Vec<TodoKey>, done: bool },
    SetDue { keys: Vec<TodoKey>, target: DueTarget },
    SetMyday { key: TodoKey, on: bool },
    Update { item: TodoItem },
    Delete { keys: Vec<TodoKey> },
    /// A new line, rendered up front so the marker is known before the write:
    /// the GUI shows the todo and may queue further changes to it right away.
    Add { line: String, marker: String },
    Assign {
        keys: Vec<TodoKey>,
        projects: Vec<String>,
        contexts: Vec<String>,
    },
}

/// Whether an operation runs for the first time or is replayed from the
/// journal of an earlier run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    Live,
    Replay,
}

impl PendingOp {
    /// Prepare an operation from an earlier run for replay.
    ///
    /// The file has moved on since, so a line index proves nothing: resolving
    /// a key by index would hit whatever sits on that line now. Every key must
    /// therefore resolve by marker alone; an operation on a todo without one
    /// cannot be replayed safely and yields `None`.
    pub fn for_replay(mut self) -> Option<Self> {
        fn pin(key: &mut TodoKey) -> Option<()> {
            key.marker.as_ref().filter(|m| !m.is_empty())?;
            key.line_index = usize::MAX;
            Some(())
        }
        match &mut self {
            PendingOp::SetDone { keys, .. }
            | PendingOp::SetDue { keys, .. }
            | PendingOp::Delete { keys }
            | PendingOp::Assign { keys, .. } => {
                for key in keys.iter_mut() {
                    pin(key)?;
                }
            }
            PendingOp::SetMyday { key, .. } => pin(key)?,
            PendingOp::Update { item } => pin(&mut item.key)?,
            PendingOp::Add { .. } => {}
        }
        Some(self)
    }

    /// Point every reference to `from` at `to`.
    ///
    /// An add can end up under another marker than the one it was queued with
    /// when another device took that marker first; changes queued behind the
    /// add must follow it.
    pub fn rename_marker(&mut self, from: &str, to: &str) {
        let rename = |key: &mut TodoKey| {
            if key.marker.as_deref() == Some(from) {
                key.marker = Some(to.to_string());
            }
        };
        match self {
            PendingOp::SetDone { keys, .. }
            | PendingOp::SetDue { keys, .. }
            | PendingOp::Delete { keys }
            | PendingOp::Assign { keys, .. } => keys.iter_mut().for_each(rename),
            PendingOp::SetMyday { key, .. } => rename(key),
            PendingOp::Update { item } => rename(&mut item.key),
            PendingOp::Add { .. } => {}
        }
    }

    /// Store the operation. Returns the marker the todo carries in the file
    /// for an add (it may differ from the requested one), `None` otherwise.
    pub fn apply(&self, mode: ApplyMode) -> Result<Option<String>> {
        let mut attempt = 1;
        loop {
            match self.apply_once(mode) {
                Err(err)
                    if attempt < CONFLICT_ATTEMPTS
                        && err.downcast_ref::<ConflictError>().is_some() =>
                {
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    fn apply_once(&self, mode: ApplyMode) -> Result<Option<String>> {
        match self {
            PendingOp::SetDone { keys, done } => toggle_todos(keys, *done).map(|_| None),
            PendingOp::SetDue { keys, target } => set_due_batch(keys, *target).map(|_| None),
            PendingOp::SetMyday { key, on: true } => set_myday_today(key).map(|_| None),
            PendingOp::SetMyday { key, on: false } => unset_myday(key).map(|_| None),
            PendingOp::Update { item } => update_todo_details(item).map(|_| None),
            PendingOp::Delete { keys } => delete_todos(keys).map(|_| None),
            PendingOp::Add { line, marker } => {
                let insert_mode = match mode {
                    ApplyMode::Live => InsertMode::Fresh,
                    ApplyMode::Replay => InsertMode::Replay,
                };
                insert_line(line.clone(), marker.clone(), insert_mode).map(|key| key.marker)
            }
            PendingOp::Assign {
                keys,
                projects,
                contexts,
            } => assign_project_context_batch(keys, projects, contexts).map(|_| None),
        }
    }
}

/// One queued operation, tied to the database it was meant for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: u64,
    /// [`crate::config::backend_identity`] of the target database.
    pub backend: String,
    pub op: PendingOp,
}

#[derive(Default, Serialize, Deserialize)]
struct JournalFile {
    entries: Vec<JournalEntry>,
}

/// Queued operations, persisted to a file after every change.
pub struct Journal {
    path: PathBuf,
    entries: Vec<JournalEntry>,
    next_id: u64,
}

impl Journal {
    /// Open the journal at `path`, keeping whatever an earlier run left in it.
    ///
    /// An unreadable journal is set aside rather than overwritten, so its
    /// content can still be recovered by hand.
    pub fn open(path: PathBuf) -> Self {
        let entries = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<JournalFile>(&text) {
                Ok(file) => file.entries,
                Err(err) => {
                    eprintln!("Unreadable journal {}: {err}", path.display());
                    let _ = fs::rename(&path, path.with_extension("corrupt"));
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        let next_id = entries.iter().map(|e| e.id + 1).max().unwrap_or(1);
        Self {
            path,
            entries,
            next_id,
        }
    }

    /// Entries from an earlier run that belong to `backend`, oldest first.
    ///
    /// Entries for other databases stay in the journal untouched; they are
    /// replayed once that database is configured again.
    pub fn leftovers(&self, backend: &str) -> Vec<JournalEntry> {
        self.entries
            .iter()
            .filter(|e| e.backend == backend)
            .cloned()
            .collect()
    }

    /// Record `op` and persist it before returning its id.
    pub fn push(&mut self, backend: &str, op: PendingOp) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(JournalEntry {
            id,
            backend: backend.to_string(),
            op,
        });
        self.save()?;
        Ok(id)
    }

    /// Replace the operation of entry `id` (after a marker rename).
    pub fn replace(&mut self, id: u64, op: PendingOp) -> Result<()> {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.op = op;
            self.save()?;
        }
        Ok(())
    }

    /// Cross off entry `id` once it was stored (or definitively failed).
    pub fn remove(&mut self, id: u64) -> Result<()> {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() != before {
            self.save()?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Write the journal via a temporary file and a rename, so a crash in the
    /// middle of saving leaves either the old or the new journal, never half.
    fn save(&self) -> Result<()> {
        if self.entries.is_empty() {
            return match fs::remove_file(&self.path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
                _ => Ok(()),
            };
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = tmp_path(&self.path);
        let text = serde_json::to_string(&JournalFile {
            entries: self.entries.clone(),
        })?;
        fs::write(&tmp, text).with_context(|| format!("Could not write {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("Could not write {}", self.path.display()))
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todo::tests::{file_lock, read, setup};
    use crate::todo::title_line;
    use crate::util::generate_marker;

    fn key(marker: &str, line_index: usize) -> TodoKey {
        TodoKey {
            line_index,
            marker: Some(marker.to_string()),
        }
    }

    fn journal_path() -> PathBuf {
        std::env::temp_dir().join(format!("reinschrift_journal_{}.json", generate_marker()))
    }

    #[test]
    fn journal_survives_a_restart_and_is_removed_when_drained() {
        let path = journal_path();
        let op = PendingOp::SetDone {
            keys: vec![key("aaa1", 0)],
            done: true,
        };

        let mut journal = Journal::open(path.clone());
        let id = journal.push("file:/a", op.clone()).expect("push");
        journal
            .push("file:/b", PendingOp::Delete { keys: vec![key("bbb2", 1)] })
            .expect("push");

        // A new process sees what the old one left behind, per database.
        let reopened = Journal::open(path.clone());
        let leftovers = reopened.leftovers("file:/a");
        assert_eq!(leftovers.len(), 1);
        assert_eq!(leftovers[0].op, op);
        assert_eq!(leftovers[0].id, id);

        let mut journal = reopened;
        journal.remove(id).expect("remove");
        assert!(journal.leftovers("file:/a").is_empty());
        assert_eq!(journal.leftovers("file:/b").len(), 1, "other database untouched");

        let other = journal.leftovers("file:/b")[0].id;
        journal.remove(other).expect("remove");
        assert!(!path.exists(), "an empty journal leaves no file behind");
    }

    #[test]
    fn replay_refuses_keys_without_marker_and_ignores_line_index() {
        let unmarked = PendingOp::Delete {
            keys: vec![TodoKey {
                line_index: 3,
                marker: None,
            }],
        };
        assert_eq!(unmarked.for_replay(), None);

        let marked = PendingOp::Delete {
            keys: vec![key("aaa1", 3)],
        };
        assert_eq!(
            marked.for_replay(),
            Some(PendingOp::Delete {
                keys: vec![key("aaa1", usize::MAX)]
            })
        );
    }

    #[test]
    fn replaying_a_delete_that_already_landed_leaves_other_lines_alone() {
        let _guard = file_lock();
        // ^bbb2 was deleted, then the app died before crossing it off.
        let path = setup("- [ ] Eins ^aaa1\n- [ ] Drei ^ccc3\n");

        let op = PendingOp::Delete {
            keys: vec![key("bbb2", 1)],
        }
        .for_replay()
        .expect("marked");
        op.apply(ApplyMode::Replay).expect("apply");

        assert_eq!(read(&path), "- [ ] Eins ^aaa1\n- [ ] Drei ^ccc3\n");
    }

    #[test]
    fn replaying_an_add_that_already_landed_does_not_duplicate_it() {
        let _guard = file_lock();
        let path = setup("- [ ] Eins ^aaa1\n");
        let (line, marker) = title_line("Neu", false).expect("line");
        let op = PendingOp::Add {
            line,
            marker: marker.clone(),
        };

        op.apply(ApplyMode::Live).expect("first write");
        let once = read(&path);
        let stored = op.apply(ApplyMode::Replay).expect("replay");

        assert_eq!(read(&path), once, "replay must not insert a second copy");
        assert_eq!(stored.as_deref(), Some(marker.as_str()));
    }

    #[test]
    fn a_live_add_whose_marker_is_taken_reports_the_new_marker() {
        let _guard = file_lock();
        let path = setup("- [ ] Fremd ^taken\n");
        let op = PendingOp::Add {
            line: "- [ ] Neu ^taken".to_string(),
            marker: "taken".to_string(),
        };

        let stored = op.apply(ApplyMode::Live).expect("apply").expect("marker");

        assert_ne!(stored, "taken");
        assert!(read(&path).contains(&format!("- [ ] Neu ^{stored}")));

        let mut follow_up = PendingOp::SetDone {
            keys: vec![key("taken", usize::MAX)],
            done: true,
        };
        follow_up.rename_marker("taken", &stored);
        follow_up.apply(ApplyMode::Live).expect("follow-up");
        let content = read(&path);
        assert!(content.contains("- [ ] Fremd ^taken"), "{content}");
        assert!(content.contains(&format!("- [x] Neu")), "{content}");
    }

    #[test]
    fn replaying_a_completion_twice_does_not_spawn_a_second_occurrence() {
        let _guard = file_lock();
        let path = setup("- [ ] Gießen rec:daily due:2026-01-01T09:00 ^aaa1\n");
        let op = PendingOp::SetDone {
            keys: vec![key("aaa1", 0)],
            done: true,
        };

        op.apply(ApplyMode::Live).expect("first");
        let once = read(&path);
        op.clone()
            .for_replay()
            .expect("marked")
            .apply(ApplyMode::Replay)
            .expect("replay");

        assert_eq!(read(&path), once);
        assert_eq!(once.lines().count(), 2, "one completion, one next occurrence");
    }
}
