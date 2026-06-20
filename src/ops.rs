//! Background long-running file ops (copy/move/extract) with a
//! progress channel back to the main loop.

use crate::fs_ops::{is_archive, unique_dest, PasteMode};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    Extract,
}

impl OpKind {
    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Copy => "Copying",
            OpKind::Move => "Moving",
            OpKind::Extract => "Extracting",
        }
    }
}

pub enum OpMsg {
    /// (done_bytes, total_bytes). For indeterminate ops only the
    /// `Finished` variant is ever sent.
    Progress(u64, u64),
    /// `Ok(status_text)` on success, `Err(error_text)` on failure.
    Finished(Result<String, String>),
    /// The worker's actual destination for the first item, so the cursor
    /// lands on the real pasted file even when a name collision renamed it.
    Reveal(PathBuf),
}

pub struct OpState {
    pub kind: OpKind,
    pub label: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub indeterminate: bool,
    pub started_at: Instant,
    pub rx: mpsc::Receiver<OpMsg>,
    /// Path to focus in the tree once the op finishes.
    pub reveal_hint: Option<PathBuf>,
    /// Set when a terminal `OpMsg::Finished` arrives. Main loop reads
    /// this to know the op is over and update status / refresh.
    pub finished: Option<Result<String, String>>,
}

impl OpState {
    /// Drain any pending messages from the worker. Returns true if the
    /// op transitioned to finished during this poll.
    pub fn poll(&mut self) -> bool {
        let mut just_finished = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                OpMsg::Progress(d, t) => {
                    self.done_bytes = d;
                    self.total_bytes = t;
                }
                OpMsg::Reveal(p) => {
                    self.reveal_hint = Some(p);
                }
                OpMsg::Finished(r) => {
                    self.finished = Some(r);
                    just_finished = true;
                    break;
                }
            }
        }
        just_finished
    }
}

/// Recursive byte total (best-effort; symlinks not followed).
fn size_of(p: &Path) -> u64 {
    let m = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if m.file_type().is_symlink() {
        return 0;
    }
    if m.is_file() {
        return m.len();
    }
    if !m.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            total = total.saturating_add(size_of(&e.path()));
        }
    }
    total
}

/// Spawn a paste worker. Caller stashes the returned OpState on App.
pub fn spawn_paste(srcs: Vec<PathBuf>, dst_dir: PathBuf, mode: PasteMode) -> OpState {
    let (tx, rx) = mpsc::channel();
    let kind = match mode {
        PasteMode::Copy => OpKind::Copy,
        PasteMode::Move => OpKind::Move,
    };
    let n = srcs.len();
    let label = format!("{} {n} item{}", kind.verb(), if n == 1 { "" } else { "s" });
    // Reveal destination of the first item once the op completes.
    let reveal_hint = srcs
        .first()
        .and_then(|s| s.file_name().map(|fname| unique_dest(&dst_dir, fname)));

    let srcs_for_worker = srcs.clone();
    let dst_for_worker = dst_dir.clone();
    thread::spawn(move || {
        // Pre-walk: compute total bytes for the progress denominator.
        let total: u64 = srcs_for_worker.iter().map(|s| size_of(s)).sum();
        let _ = tx.send(OpMsg::Progress(0, total));
        let mut done = 0u64;
        let mut failed: Vec<String> = Vec::new();
        let mut claimed: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
        let mut first = true;
        for src in &srcs_for_worker {
            let Some(fname) = src.file_name() else {
                failed.push(src.display().to_string());
                continue;
            };
            let dst = crate::fs_ops::claim_dest(&dst_for_worker, fname, &mut claimed);
            if first {
                // Report the real first destination so the post-op cursor
                // lands on the actual (possibly renamed) pasted file.
                let _ = tx.send(OpMsg::Reveal(dst.clone()));
                first = false;
            }
            let src_size = size_of(src);
            let base_done = done;
            let result: Result<(), String> = match mode {
                PasteMode::Copy => {
                    if src.is_dir() {
                        let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
                        let tx2 = tx.clone();
                        fs_extra::dir::copy_with_progress(src, &dst, &opts, |p| {
                            let _ = tx2.send(OpMsg::Progress(
                                base_done + p.copied_bytes,
                                total,
                            ));
                            fs_extra::dir::TransitProcessResult::ContinueOrAbort
                        })
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                    } else {
                        std::fs::copy(src, &dst).map(|_| ()).map_err(|e| e.to_string())
                    }
                }
                PasteMode::Move => {
                    // Try rename first (instant on same filesystem).
                    match std::fs::rename(src, &dst) {
                        Ok(_) => Ok(()),
                        // Only a cross-device rename warrants the slow
                        // copy+remove fallback. Any other error (permission
                        // denied, etc.) is reported as-is rather than masked
                        // by a doomed copy attempt.
                        Err(e) if e.raw_os_error() != Some(libc::EXDEV) => {
                            Err(format!("move failed: {e}"))
                        }
                        Err(_) => {
                            // Cross-device → copy+remove.
                            if src.is_dir() {
                                let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
                                let tx2 = tx.clone();
                                fs_extra::dir::move_dir_with_progress(
                                    src, &dst, &opts, |p| {
                                        let _ = tx2.send(OpMsg::Progress(
                                            base_done + p.copied_bytes,
                                            total,
                                        ));
                                        fs_extra::dir::TransitProcessResult::ContinueOrAbort
                                    },
                                )
                                .map(|_| ())
                                .map_err(|e| format!("dir move partial — check both locations: {e}"))
                            } else {
                                crate::fs_ops::move_file_cross_fs(src, &dst)
                            }
                        }
                    }
                }
            };
            match result {
                Ok(_) => done = done.saturating_add(src_size),
                Err(e) => failed.push(format!("{}: {e}", src.display())),
            }
            let _ = tx.send(OpMsg::Progress(done, total));
        }
        let msg = if failed.is_empty() {
            Ok(format!(
                "{} {n} -> {}",
                match kind {
                    OpKind::Copy => "copied",
                    OpKind::Move => "moved",
                    _ => "done",
                },
                dst_for_worker.display()
            ))
        } else {
            let preview = failed
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let tail = if failed.len() > 2 { ", …" } else { "" };
            Err(format!("{} failed: {preview}{tail}", failed.len()))
        };
        let _ = tx.send(OpMsg::Finished(msg));
    });

    OpState {
        kind,
        label,
        done_bytes: 0,
        total_bytes: 0,
        indeterminate: false,
        started_at: Instant::now(),
        rx,
        reveal_hint,
        finished: None,
    }
}

/// Spawn an extract worker. Shell tools don't expose a progress stream,
/// so the op is indeterminate; the UI shows a spinner + elapsed time.
pub fn spawn_extract(archive: PathBuf) -> Option<OpState> {
    if !is_archive(&archive) {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    let label = format!(
        "Extracting {}",
        archive.file_name().and_then(|s| s.to_str()).unwrap_or("?")
    );
    let reveal_archive = archive.clone();
    thread::spawn(move || {
        let result = crate::fs_ops::extract_archive(&archive)
            .map(|dest| format!("extracted -> {}", dest.display()))
            .map_err(|e| e.to_string());
        let _ = tx.send(OpMsg::Finished(result));
    });
    Some(OpState {
        kind: OpKind::Extract,
        label,
        done_bytes: 0,
        total_bytes: 0,
        indeterminate: true,
        started_at: Instant::now(),
        rx,
        // We don't know the destination here; main loop derives it from
        // the Finished status text (or just refreshes + lets the user see
        // the new dir appear in the tree).
        reveal_hint: Some(
            reveal_archive
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| reveal_archive.clone()),
        ),
        finished: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_kind_verbs() {
        assert_eq!(OpKind::Copy.verb(), "Copying");
        assert_eq!(OpKind::Move.verb(), "Moving");
        assert_eq!(OpKind::Extract.verb(), "Extracting");
    }

    #[test]
    fn size_of_file_returns_len() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.txt");
        std::fs::write(&p, b"hello world").unwrap();
        assert_eq!(size_of(&p), 11);
    }

    #[test]
    fn size_of_recursive_dir() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), b"1234").unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/b"), b"abcdef").unwrap();
        assert_eq!(size_of(d.path()), 10);
    }
}
