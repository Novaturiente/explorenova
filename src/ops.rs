//! Background long-running file ops (copy/move/extract) with a
//! progress channel back to the main loop.

use crate::fs_ops::{copy_dir_recursive, is_archive, unique_dest, CopyCtl, PasteMode};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    Trash,
}

impl OpKind {
    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Copy => "Copying",
            OpKind::Move => "Moving",
            OpKind::Extract => "Extracting",
            OpKind::Mount => "Mounting",
            OpKind::Unmount => "Unmounting",
            OpKind::Trash => "Trashing",
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

/// A finished op, kept for the footer's processes panel.
pub struct OpRecord {
    pub kind: OpKind,
    pub label: String,
    pub result: Result<String, String>,
}

impl OpState {
    /// Progress as (percent, text). `None` percent = indeterminate (spinner).
    /// Trash counts items, everything else bytes.
    pub fn progress(&self) -> (Option<u16>, String) {
        if self.indeterminate || self.total_bytes == 0 {
            return (None, "working…".to_string());
        }
        let pct = (self.done_bytes as f64 / self.total_bytes as f64 * 100.0).clamp(0.0, 100.0) as u16;
        let text = if self.kind == OpKind::Trash {
            format!("{} / {} items · {pct}%", self.done_bytes, self.total_bytes)
        } else {
            format!(
                "{} / {} · {pct}%",
                crate::status::human_bytes(self.done_bytes),
                crate::status::human_bytes(self.total_bytes)
            )
        };
        (Some(pct), text)
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Drain any pending messages from the worker. Returns true if the
    /// op transitioned to finished during this poll.
    pub fn poll(&mut self) -> bool {
        let mut just_finished = false;
        loop {
            match self.rx.try_recv() {
                Ok(OpMsg::Progress(d, t)) => {
                    self.done_bytes = d;
                    self.total_bytes = t;
                }
                Ok(OpMsg::Reveal(p)) => {
                    self.reveal_hint = Some(p);
                }
                Ok(OpMsg::Finished(r)) => {
                    self.finished = Some(r);
                    just_finished = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                // The worker dropped its sender without ever sending
                // Finished, which happens only if it panicked. Treating
                // that as "nothing to read yet" left the spinner turning
                // and the op modal up for the rest of the session.
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.finished.is_none() {
                        self.finished = Some(Err("operation stopped unexpectedly".to_string()));
                        just_finished = true;
                    }
                    break;
                }
            }
        }
        just_finished
    }
}

/// Directory test that does NOT follow symlinks. `Path::is_dir()` does, so
/// a selected symlink-to-file used to take the plain-file copy branch and
/// have its target's contents written into a regular file, destroying the
/// link; a symlink-to-directory took the `remove_dir_all` branch on a
/// cross-device move and failed with ENOTDIR after a successful copy.
fn is_real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false)
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
        // Pre-walk ONCE: record each source's size from the same walk that
        // computes the total, instead of re-walking every item a second
        // time inside the loop.
        let sizes: Vec<u64> = srcs_for_worker.iter().map(|s| size_of(s)).collect();
        let total: u64 = sizes.iter().sum();
        let _ = tx.send(OpMsg::Progress(0, total));
        // `done` flows through the copy_file_chunked/copy_dir_recursive
        // progress callback, which needs `Fn(u64)` (not `FnMut`) — an
        // atomic gives the interior mutability a plain u64 can't, even
        // though this worker is single-threaded.
        let done_shared = AtomicU64::new(0);
        let mut failed: Vec<String> = Vec::new();
        let mut claimed: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
        let mut first = true;
        // Set only when a mid-item cancel leaves a genuine partial
        // directory behind (copy_file_chunked cleans up its own partial
        // file, so single-file cancels never need this).
        let mut cancel_partial_dst: Option<PathBuf> = None;
        for (i, src) in srcs_for_worker.iter().enumerate() {
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
            let src_size = sizes[i];
            let base_done = done_shared.load(Ordering::Relaxed);
            let tx_progress = tx.clone();
            let progress = |d: u64| {
                let cur = done_shared.fetch_add(d, Ordering::Relaxed) + d;
                let _ = tx_progress.send(OpMsg::Progress(cur, total));
            };
            let ctl = CopyCtl { cancel: &cancel_worker, progress: &progress };
            // Set when the bytes landed but the source could not be removed:
            // the destination is the good copy and must survive the error.
            let mut keep_dst_on_error = false;
            // `claim_dest` picked a free name, but something can appear there
            // before the write (another process, a flaky SMB stat). Never
            // merge into, overwrite, or later clean up a path this op did
            // not create.
            if std::fs::symlink_metadata(&dst).is_ok() {
                failed.push(format!("{}: destination appeared: {}", src.display(), dst.display()));
                continue;
            }
            let result: io::Result<()> = match mode {
                // `copy_dir_recursive` classifies with `symlink_metadata` and
                // dispatches to itself, `copy_file_chunked`, or a fresh
                // symlink, so it is the right entry point for all three.
                PasteMode::Copy => copy_dir_recursive(src, &dst, &ctl),
                PasteMode::Move => {
                    // Try rename first (instant on same filesystem).
                    match std::fs::rename(src, &dst) {
                        Ok(_) => Ok(()),
                        // Only a cross-device rename warrants the slow
                        // copy+remove fallback. Any other error (permission
                        // denied, etc.) is reported as-is rather than masked
                        // by a doomed copy attempt.
                        Err(e) if e.raw_os_error() != Some(libc::EXDEV) => {
                            Err(io::Error::new(e.kind(), format!("move failed: {e}")))
                        }
                        Err(_) => {
                            // Cross-device → copy+remove, symlinks recreated
                            // (never dereferenced) by copy_dir_recursive.
                            // Remove the source ONLY on full copy success.
                            match copy_dir_recursive(src, &dst, &ctl) {
                                Err(e) => Err(e),
                                Ok(()) => {
                                    let rm = if is_real_dir(src) {
                                        std::fs::remove_dir_all(src)
                                    } else {
                                        std::fs::remove_file(src)
                                    };
                                    rm.map_err(|e| {
                                        keep_dst_on_error = true;
                                        io::Error::new(
                                            e.kind(),
                                            format!(
                                                "move partial — copied but source not removed: {e}"
                                            ),
                                        )
                                    })
                                }
                            }
                        }
                    }
                }
            };
            match result {
                Ok(_) => {
                    done_shared.store(base_done + src_size, Ordering::Relaxed);
                    let _ = tx.send(OpMsg::Progress(base_done + src_size, total));
                }
                // Interrupted alone isn't proof of cancellation — gate on the
                // flag so a stray EINTR can't end the loop reporting success.
                Err(e) if e.kind() == io::ErrorKind::Interrupted
                    && cancel_worker.load(Ordering::Relaxed) =>
                {
                    if src.is_dir() {
                        cancel_partial_dst = Some(dst.clone());
                    }
                    break;
                }
                Err(e) => {
                    done_shared.store(base_done, Ordering::Relaxed);
                    // A failed copy leaves however much got written behind.
                    // `copy_file_chunked` cleans up only its own cancel path,
                    // so a half-copied directory (or a file that failed on a
                    // full disk) would otherwise sit in the destination
                    // looking like a complete item.
                    // AlreadyExists = someone else's file is at `dst`.
                    if !keep_dst_on_error && e.kind() != io::ErrorKind::AlreadyExists {
                        let _ = if is_real_dir(&dst) {
                            std::fs::remove_dir_all(&dst)
                        } else {
                            std::fs::remove_file(&dst)
                        };
                    }
                    failed.push(format!("{}: {e}", src.display()));
                    let _ = tx.send(OpMsg::Progress(base_done, total));
                }
            }
        }
        let was_cancelled = cancel_worker.load(Ordering::Relaxed);
        let msg = if was_cancelled {
            if let Some(partial) = &cancel_partial_dst {
                Ok(format!("cancelled — partial copy left at {}", partial.display()))
            } else {
                Ok(format!(
                    "cancelled ({} done)",
                    crate::status::human_bytes(done_shared.load(Ordering::Relaxed))
                ))
            }
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
/// Cancel works like `spawn_mount`'s gio calls: a try_wait poll loop that
/// kills the child on request, then removes the partial destination dir
/// so a cancelled extract never leaves a half-unpacked archive behind.
pub fn spawn_extract(archive: PathBuf) -> Option<OpState> {
    if !is_archive(&archive) {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    let label = format!(
        "Extracting {}",
        archive.file_name().and_then(|s| s.to_str()).unwrap_or("?")
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    let reveal_archive = archive.clone();
    thread::spawn(move || {
        let dest = crate::fs_ops::extract_dest_for(&archive);
        let outcome: io::Result<PathBuf> = (|| {
            std::fs::create_dir_all(&dest)?;
            let mut cmd = crate::fs_ops::extract_command(&archive, &dest)?;
            let mut child = cmd
                .spawn()
                .map_err(|e| io::Error::other(format!("failed to spawn extractor: {e}")))?;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => return Ok(dest.clone()),
                    Ok(Some(status)) => {
                        return Err(io::Error::other(format!(
                            "extractor exited with status {status}"
                        )))
                    }
                    Ok(None) => {
                        if cancel_worker.load(Ordering::Relaxed) {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ = std::fs::remove_dir_all(&dest);
                            return Err(io::Error::new(
                                io::ErrorKind::Interrupted,
                                "extraction cancelled",
                            ));
                        }
                        thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(io::Error::other(format!("wait failed: {e}")));
                    }
                }
            }
        })();
        let result = match outcome {
            Ok(dest) => Ok(format!("extracted -> {}", dest.display())),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                Err("extraction cancelled".to_string())
            }
            Err(e) => Err(e.to_string()),
        };
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

/// Trash `targets` off-thread. Progress is an item count (not bytes) —
/// the op modal renders "{done} / {total} items". Esc cancels between
/// items; items already trashed before a cancel/failure stay trashed
/// (trash is per-item atomic, there's nothing to roll back).
///
/// ponytail: `reveal_hint` (and the generic op-finished handler's root +
/// reveal-parent self-write marks) only precisely cover the FIRST
/// target's own dir when a marked multi-select spans several
/// directories — the other dirs still refresh correctly, just via the
/// normal fs-watcher debounce instead of being pre-suppressed. Upgrade
/// path if that shows up as a visible double-refresh: thread the full
/// parent-dir set through `OpState` instead of a single `reveal_hint`.
pub fn spawn_trash(targets: Vec<PathBuf>) -> OpState {
    let (tx, rx) = mpsc::channel();
    let n = targets.len();
    let label = format!("Trashing {n} item{}", if n == 1 { "" } else { "s" });
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    // reveal_hint is the target's own path (spawn_paste/extract
    // convention) — main.rs's op-finished handler takes `.parent()` of
    // this to find the dir to mark self-written, so this must be a FILE
    // path, not a dir. (Was mistakenly set to the parent dir, which made
    // main.rs mark the grandparent instead.)
    let reveal = targets.first().cloned();
    thread::spawn(move || {
        let mut ok = 0usize;
        let mut failed: Vec<String> = Vec::new();
        let mut cancelled = false;
        for p in &targets {
            if cancel_worker.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            match crate::fs_ops::trash_path(p) {
                Ok(_) => ok += 1,
                Err(e) => failed.push(format!("{}: {e}", p.display())),
            }
            let _ = tx.send(OpMsg::Progress(ok as u64, n as u64));
        }
        // Mirrors the message shaping the old synchronous trash_targets used
        // (always a status, not an error — a partial failure still trashed
        // most of the selection).
        let msg = if cancelled {
            format!("cancelled ({ok} trashed)")
        } else if failed.is_empty() {
            if n == 1 {
                format!("trashed {}", targets[0].display())
            } else {
                format!("trashed {n}")
            }
        } else {
            let preview = failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
            let tail = if failed.len() > 2 { ", …" } else { "" };
            format!("trashed {ok}/{n} ({} failed: {preview}{tail})", failed.len())
        };
        let _ = tx.send(OpMsg::Finished(Ok(msg)));
    });
    OpState {
        kind: OpKind::Trash,
        label,
        done_bytes: 0,
        total_bytes: n as u64,
        indeterminate: false,
        started_at: Instant::now(),
        rx,
        reveal_hint: reveal,
        finished: None,
        cancel,
    }
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
        assert_eq!(OpKind::Trash.verb(), "Trashing");
    }

    #[test]
    fn spawn_trash_moves_files_and_reports_progress() {
        // Isolate the freedesktop trash dir under a tempdir so this
        // doesn't touch the real user trash (same pattern as
        // tests/apps_int.rs for XDG_DATA_HOME).
        let data_home = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_DATA_HOME", data_home.path());
        let src = tempfile::tempdir().unwrap();
        let f1 = src.path().join("a.txt");
        let f2 = src.path().join("b.txt");
        std::fs::write(&f1, "1").unwrap();
        std::fs::write(&f2, "2").unwrap();

        let mut op = spawn_trash(vec![f1.clone(), f2.clone()]);
        assert_eq!(op.kind, OpKind::Trash);
        assert_eq!(op.total_bytes, 2); // item count, not bytes
        let t0 = Instant::now();
        while !op.poll() {
            if t0.elapsed() > std::time::Duration::from_secs(10) {
                panic!("spawn_trash never finished");
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!f1.exists());
        assert!(!f2.exists());
        match op.finished {
            Some(Ok(s)) => assert_eq!(s, "trashed 2"),
            other => panic!("expected Ok(\"trashed 2\"), got {other:?}"),
        }
    }

    #[test]
    fn spawn_trash_reveal_hint_is_first_target_itself() {
        // main.rs's op-finished handler does `reveal.parent()` to find the
        // dir to mark self-written (spawn_paste/extract convention) — so
        // reveal_hint must be the target's own path, not its parent dir.
        let data_home = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_DATA_HOME", data_home.path());
        let src = tempfile::tempdir().unwrap();
        let f1 = src.path().join("a.txt");
        let f2 = src.path().join("b.txt");
        std::fs::write(&f1, "1").unwrap();
        std::fs::write(&f2, "2").unwrap();
        let mut op = spawn_trash(vec![f1.clone(), f2.clone()]);
        assert_eq!(op.reveal_hint, Some(f1));
        let t0 = Instant::now();
        while !op.poll() {
            if t0.elapsed() > std::time::Duration::from_secs(10) {
                panic!("spawn_trash never finished");
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
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
    fn poll_reports_a_worker_that_died_without_finishing() {
        // A dropped sender with no Finished means the worker panicked.
        // Reading it as "nothing yet" left the op modal and its spinner up
        // for the rest of the session with no way to dismiss them.
        let (tx, rx) = mpsc::channel();
        let mut op = OpState {
            kind: OpKind::Copy,
            label: "Copying 1 item".into(),
            done_bytes: 0,
            total_bytes: 1,
            indeterminate: false,
            started_at: Instant::now(),
            rx,
            reveal_hint: None,
            finished: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        assert!(!op.poll(), "an open channel is not a finished op");
        drop(tx);
        assert!(op.poll(), "a dead worker must end the op");
        assert!(op.finished.as_ref().unwrap().is_err());
    }

    #[test]
    fn a_failed_copy_leaves_nothing_behind_in_the_destination() {
        // copy_file_chunked cleans up only its own cancel path, so a
        // directory copy that failed on its second file used to leave a
        // half-populated directory in the destination, looking exactly
        // like a complete one.
        use std::os::unix::fs::PermissionsExt;
        let src = tempfile::tempdir().unwrap();
        let sub = src.path().join("tree");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a"), b"1234").unwrap();
        let unreadable = sub.join("b");
        std::fs::write(&unreadable, b"5678").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&unreadable).is_ok() {
            // Running as root, where mode 000 means nothing. Nothing to test.
            return;
        }
        let dst = tempfile::tempdir().unwrap();

        let mut op = spawn_paste(
            vec![sub.clone()],
            dst.path().to_path_buf(),
            crate::fs_ops::PasteMode::Copy,
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while op.finished.is_none() && Instant::now() < deadline {
            op.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(op.finished.is_some(), "paste never finished");
        assert!(
            !dst.path().join("tree").exists(),
            "a half-copied directory must not survive the failure"
        );
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
