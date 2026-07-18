//! Background long-running file ops (copy/move/extract) with a
//! progress channel back to the main loop.

use crate::fs_ops::{is_archive, unique_dest, PasteMode};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    Extract,
    Mount,
    Unmount,
}

impl OpKind {
    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Copy => "Copying",
            OpKind::Move => "Moving",
            OpKind::Extract => "Extracting",
            OpKind::Mount => "Mounting",
            OpKind::Unmount => "Unmounting",
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
    /// Set true to ask the worker to stop at the next item boundary.
    pub cancel: Arc<AtomicBool>,
}

impl OpState {
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

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

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
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
            if cancel_worker.load(Ordering::Relaxed) {
                break;
            }
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
                        let cancel2 = cancel_worker.clone();
                        fs_extra::dir::copy_with_progress(src, &dst, &opts, |p| {
                            let _ = tx2.send(OpMsg::Progress(
                                base_done + p.copied_bytes,
                                total,
                            ));
                            if cancel2.load(Ordering::Relaxed) {
                                return fs_extra::dir::TransitProcessResult::Abort;
                            }
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
                                let cancel2 = cancel_worker.clone();
                                fs_extra::dir::move_dir_with_progress(
                                    src, &dst, &opts, |p| {
                                        let _ = tx2.send(OpMsg::Progress(
                                            base_done + p.copied_bytes,
                                            total,
                                        ));
                                        if cancel2.load(Ordering::Relaxed) {
                                            return fs_extra::dir::TransitProcessResult::Abort;
                                        }
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
        let was_cancelled = cancel_worker.load(Ordering::Relaxed);
        let msg = if was_cancelled {
            Ok(format!("cancelled ({} done)", crate::status::human_bytes(done)))
        } else if failed.is_empty() {
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
        cancel,
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
    // ponytail: extract is one external process; cancel is a no-op until it
    // returns. Killing the child process is a future upgrade.
    let cancel = Arc::new(AtomicBool::new(false));
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
        cancel,
    })
}

/// Spawn a `gio` subcommand off-thread with a 30s ceiling and Esc-cancel.
/// gio has no connect timeout of its own — run inline it froze the whole
/// TUI for the OS TCP timeout (minutes) on an unreachable host.
/// `ok_msg`/`fail_prefix` shape the Finished text; `cancel_msg` covers the
/// cancelled-or-timed-out case. Indeterminate spinner (no byte/item total).
fn spawn_gio(
    kind: OpKind,
    args: Vec<String>,
    label: String,
    ok_msg: String,
    fail_prefix: String,
    cancel_msg: String,
) -> OpState {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    thread::spawn(move || {
        use std::io::Read;
        let child = std::process::Command::new("gio")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let result = match child {
            Err(e) => Err(format!("gio: {e}")),
            Ok(mut child) => {
                let mut pipe = child.stderr.take();
                let reader = thread::spawn(move || {
                    let mut buf = String::new();
                    if let Some(p) = pipe.as_mut() {
                        let _ = p.read_to_string(&mut buf);
                    }
                    buf
                });
                let t0 = Instant::now();
                let status = loop {
                    match child.try_wait() {
                        Ok(Some(st)) => break Some(st),
                        Ok(None)
                            if cancel_worker.load(Ordering::Relaxed)
                                || t0.elapsed() > std::time::Duration::from_secs(30) =>
                        {
                            let _ = child.kill();
                            let _ = child.wait();
                            break None;
                        }
                        Ok(None) => {
                            thread::sleep(std::time::Duration::from_millis(50))
                        }
                        Err(_) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break None;
                        }
                    }
                };
                // Child is dead either way, so the pipe is closed and the
                // reader has finished.
                let stderr_txt = reader.join().unwrap_or_default();
                match status {
                    Some(st) if st.success() => Ok(ok_msg),
                    Some(_) => Err(if stderr_txt.trim().is_empty() {
                        fail_prefix
                    } else {
                        stderr_txt.trim().to_string()
                    }),
                    None => Err(cancel_msg),
                }
            }
        };
        let _ = tx.send(OpMsg::Finished(result));
    });
    OpState {
        kind,
        label,
        done_bytes: 0,
        total_bytes: 0,
        indeterminate: true,
        started_at: Instant::now(),
        rx,
        reveal_hint: None,
        finished: None,
        cancel,
    }
}

pub fn spawn_mount(url: String, anonymous: bool) -> OpState {
    spawn_gio(
        OpKind::Mount,
        crate::smb::mount_argv(&url, anonymous),
        format!("Mounting {url}"),
        format!("mounted {url}"),
        format!("mount failed: {url}"),
        format!("mount cancelled / timed out: {url}"),
    )
}

pub fn spawn_unmount(url: String) -> OpState {
    spawn_gio(
        OpKind::Unmount,
        crate::smb::unmount_argv(&url),
        format!("Unmounting {url}"),
        format!("unmounted {url}"),
        format!("unmount failed: {url}"),
        format!("unmount cancelled / timed out: {url}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_kind_verbs() {
        assert_eq!(OpKind::Copy.verb(), "Copying");
        assert_eq!(OpKind::Move.verb(), "Moving");
        assert_eq!(OpKind::Extract.verb(), "Extracting");
        assert_eq!(OpKind::Mount.verb(), "Mounting");
        assert_eq!(OpKind::Unmount.verb(), "Unmounting");
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

    #[test]
    fn request_cancel_sets_flag() {
        use std::sync::atomic::Ordering;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), b"hello").unwrap();
        let dst = tempfile::tempdir().unwrap();
        let op = spawn_paste(
            vec![d.path().join("a")],
            dst.path().to_path_buf(),
            crate::fs_ops::PasteMode::Copy,
        );
        op.request_cancel();
        assert!(op.cancel.load(Ordering::Relaxed));
    }
}
