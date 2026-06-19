# Correctness Fixes, Cut Visual Cue & Plain-Path Copy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the Tier-1 data-loss/panic bugs found in the audit, add a visual cue for cut items in the tree, and make `Y` copy the plain absolute path so it pastes cleanly into terminals and editors.

**Architecture:** All changes are localized to existing modules. New logic is extracted into small pure helpers (in `fs_ops.rs` / `nav.rs`) so it is unit-testable without spawning threads or a TUI. Rendering changes thread one extra `bool` through `tree_row`.

**Tech Stack:** Rust, ratatui (TUI), fs_extra (copy/move), trash, std::fs. Tests use `tempfile` and the existing `#[cfg(test)]` inline-module + `tests/*_int.rs` integration style.

## Global Constraints

- Crate name for integration tests is `explorenova` (e.g. `use explorenova::fs_ops::...`).
- No new dependencies. Everything below uses std + already-present crates.
- IGNORE the vendored trees `src/explorenova/` and `src/cargo-home/` — never edit them.
- Run the full suite with `cargo test`; lint with `cargo clippy` (must stay warning-free on real source).
- Match existing style: status messages are lowercase terse phrases set via `app.set_status(...)`.
- Paths stored in `Node.path` and returned by `App::targets()` are already absolute (arena is built from an absolute root). No extra canonicalization needed.

---

### Task 1: `Y` copies the plain absolute path (not a `file://` URI)

**Why:** `Y` (`SysClipCopy`) currently writes newline-separated `file://` percent-encoded URIs advertised as `text/uri-list`. Pasting into a terminal or editor yields a URI or nothing. User wants the raw absolute path.

**Files:**
- Modify: `src/clipboard.rs` (replace `build_payload`, delete `encode_path`, drop the `--type`/`-t` MIME args)
- Test: `src/clipboard.rs` inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing new.
- Produces: `clipboard::copy_paths(&[PathBuf]) -> anyhow::Result<usize>` (signature unchanged) now writing plain newline-joined absolute paths as plain text.

- [ ] **Step 1: Replace the payload tests with plain-path expectations**

In `src/clipboard.rs`, replace the existing `encode_preserves_safe_chars`, `encode_percent_escapes_spaces_and_unicode`, and `payload_joins_with_newlines_and_file_scheme` tests with:

```rust
    #[test]
    fn payload_is_plain_newline_joined_paths() {
        let p = build_payload(&[
            PathBuf::from("/a/b"),
            PathBuf::from("/c with space"),
        ]);
        assert_eq!(p, "/a/b\n/c with space");
    }

    #[test]
    fn payload_single_path_has_no_trailing_newline() {
        let p = build_payload(&[PathBuf::from("/home/u/x.txt")]);
        assert_eq!(p, "/home/u/x.txt");
    }
```

Leave `empty_paths_errors` as-is.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib clipboard`
Expected: FAIL — `build_payload` still emits `file://...`, and `encode_path` no longer referenced by the old (now-deleted) tests.

- [ ] **Step 3: Rewrite `build_payload`, delete `encode_path`, drop MIME args**

Replace `encode_path` (lines ~15-31) and `build_payload` (lines ~33-43) with just:

```rust
fn build_payload(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}
```

In `copy_paths`, change the wl-copy command from:

```rust
        let mut c = Command::new("wl-copy");
        c.args(["--type", "text/uri-list"]);
```
to:
```rust
        let mut c = Command::new("wl-copy");
```

and the xclip command from:

```rust
        c.args(["-selection", "clipboard", "-t", "text/uri-list", "-i"]);
```
to:
```rust
        c.args(["-selection", "clipboard", "-i"]);
```

Update the module doc comment (lines 1-9) to say it writes plain newline-separated absolute paths.

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test --lib clipboard && cargo clippy 2>&1 | grep -A3 clipboard`
Expected: tests PASS; no clippy warnings (no unused `encode_path`, no unused `Path` import — remove `Path` from the `use` line if clippy flags it).

- [ ] **Step 5: Commit**

```bash
git add src/clipboard.rs
git commit -m "fix(clipboard): Y copies plain absolute path, not file:// URI"
```

---

### Task 2: Visual cue for cut items in the tree

**Why:** After `dd`, nothing in the tree shows which items are cut, so the user can't tell the cut registered. `app.clipboard = Some((paths, is_cut))` already holds the state; the renderer just ignores it.

**Files:**
- Modify: `src/ui/panes.rs` (`tree_row` — add a `cut: bool` param, dim the row when set)
- Modify: `src/ui/panes_render.rs:73-87` (compute the cut set, pass the flag)
- Test: `src/ui/panes.rs` inline tests

**Interfaces:**
- Consumes: `app.clipboard: Option<(Vec<PathBuf>, bool)>` (already exists).
- Produces: `tree_row(arena, id, selected, theme, icons, marked, cut, width) -> Line` — one new `bool` param inserted before `width`.

- [ ] **Step 1: Write a failing test for the cut style**

In `src/ui/panes.rs` tests module, add (and update the existing `row_renders_name` call to pass the new `false` arg before `80`):

```rust
    #[test]
    fn cut_row_is_dimmed() {
        use ratatui::style::Modifier;
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("z.txt"), "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let child = a.nodes[a.root].children.as_ref().unwrap()[0];
        let line = tree_row(&a, child, false, &Theme::default(), true, false, true, 80);
        // The name span carries the DIM modifier when cut.
        let name_span = line
            .spans
            .iter()
            .find(|s| s.content.contains("z.txt"))
            .expect("name span present");
        assert!(name_span.style.add_modifier.contains(Modifier::DIM));
    }
```

Also update the existing `row_renders_name` test's call:
```rust
        let line = tree_row(&a, child, true, &Theme::default(), true, false, false, 80);
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib panes::tests`
Expected: FAIL — `tree_row` takes 7 args, test passes 8 (won't compile) → fix is the signature change in Step 3.

- [ ] **Step 3: Add the `cut` param and apply DIM**

In `src/ui/panes.rs`, change the signature:

```rust
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
    icons: bool,
    marked: bool,
    cut: bool,
    width: u16,
) -> Line<'a> {
```

Add `use ratatui::style::Modifier;` to the imports at the top. Change the name span (line ~87) from:

```rust
    spans.push(Span::styled(name.clone(), Style::default().fg(type_color)));
```
to:
```rust
    let name_style = if cut {
        Style::default().fg(type_color).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(type_color)
    };
    spans.push(Span::styled(name.clone(), name_style));
```

- [ ] **Step 4: Thread the flag from the renderer**

In `src/ui/panes_render.rs`, just before `let items: Vec<ListItem> = visible_ref` (line ~73), add:

```rust
    // Cut items (dd) render dimmed. Clipboard is shared across tabs, so
    // read app.clipboard regardless of which slot we're drawing.
    let cut_paths: Option<&[std::path::PathBuf]> = match &app.clipboard {
        Some((paths, true)) => Some(paths),
        _ => None,
    };
```

Then inside the `.map(|(idx, &id)| { ... })`, change the `tree_row(...)` call to pass the flag (insert before `inner_w`):

```rust
            let path = &arena_ref.nodes[id].path;
            ListItem::new(crate::ui::panes::tree_row(
                arena_ref,
                id,
                idx == selected_ref,
                theme,
                app.config.icons,
                marks_ref.contains(path),
                cut_paths.map_or(false, |ps| ps.iter().any(|p| p == path)),
                inner_w,
            ))
```

(Note: this replaces the existing `marks_ref.contains(&arena_ref.nodes[id].path)` with the hoisted `path` binding to avoid borrowing twice. `cut_paths` membership is a linear scan — clipboards are tiny, so this is fine. `// ponytail: O(n) scan, clipboard is a handful of paths`.)

- [ ] **Step 5: Run tests + build**

Run: `cargo test --lib panes && cargo build 2>&1 | grep -E 'warning|error'`
Expected: tests PASS; no warnings/errors.

- [ ] **Step 6: Commit**

```bash
git add src/ui/panes.rs src/ui/panes_render.rs
git commit -m "feat(ui): dim cut items in the tree for visual feedback"
```

---

### Task 3: Reject pasting a directory into itself or a subdirectory

**Why:** Cutting `foo/` and pasting while the cursor is inside `foo/` (or on `foo/bar/`) makes the move destination a descendant of the source. `fs::rename` returns `EINVAL`, the worker falls through to `fs_extra::move_dir`, which copies the dir into a growing copy of itself then deletes the original — data loss.

**Files:**
- Modify: `src/fs_ops.rs` (add pure helper `paste_would_recurse`)
- Modify: `src/dispatch.rs` `fn paste` (guard before spawning)
- Test: `src/fs_ops.rs` inline tests

**Interfaces:**
- Produces: `fs_ops::paste_would_recurse(src: &Path, dst_dir: &Path) -> bool` — true when `dst_dir` is `src` or lives under `src`.

- [ ] **Step 1: Write the failing test**

In `src/fs_ops.rs` tests module:

```rust
    #[test]
    fn paste_recurse_detection() {
        use std::path::Path;
        // dst inside src → recursion
        assert!(paste_would_recurse(Path::new("/a/foo"), Path::new("/a/foo")));
        assert!(paste_would_recurse(Path::new("/a/foo"), Path::new("/a/foo/bar")));
        // dst is the source's own parent (paste back into same dir) → OK
        assert!(!paste_would_recurse(Path::new("/a/foo"), Path::new("/a")));
        // unrelated → OK
        assert!(!paste_would_recurse(Path::new("/a/foo"), Path::new("/b")));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib paste_recurse_detection`
Expected: FAIL — `paste_would_recurse` not defined.

- [ ] **Step 3: Implement the helper**

Add to `src/fs_ops.rs` (near `unique_dest`):

```rust
/// True when `dst_dir` is `src` itself or a descendant of it — pasting
/// there would move/copy a directory into itself. `starts_with` on a
/// component basis already covers the equality case.
pub fn paste_would_recurse(src: &Path, dst_dir: &Path) -> bool {
    dst_dir.starts_with(src)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib paste_recurse_detection`
Expected: PASS.

- [ ] **Step 5: Guard in `dispatch::paste`**

In `src/dispatch.rs` `fn paste`, after `dst_dir` is computed and before `let mode = ...` (line ~1046), insert:

```rust
    if let Some(bad) = srcs.iter().find(|s| crate::fs_ops::paste_would_recurse(s, &dst_dir)) {
        app.set_status(format!(
            "can't paste {} into itself",
            bad.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        ));
        return;
    }
```

(Placed before the clipboard-clearing block so a rejected cut keeps its clipboard intact.)

- [ ] **Step 6: Run full suite + manual sanity**

Run: `cargo test`
Expected: all PASS. (Manual: cut a dir, enter it, press `p` → status "can't paste X into itself", no data moved.)

- [ ] **Step 7: Commit**

```bash
git add src/fs_ops.rs src/dispatch.rs
git commit -m "fix(paste): reject pasting a directory into itself or a subdir"
```

---

### Task 4: Validate rename — no overwrite, no path separators

**Why:** `fs_ops::rename` calls `std::fs::rename`, which silently clobbers an existing target. Typing `a/b` relocates the file out of its directory; an empty name or `..` misbehaves.

**Files:**
- Modify: `src/fs_ops.rs` `fn rename` (validate name, reject existing target)
- Test: `tests/fs_ops_int.rs`

**Interfaces:**
- Produces: `fs_ops::rename(path, new_name) -> io::Result<PathBuf>` — same signature; now returns `Err(InvalidInput)` for bad names and `Err(AlreadyExists)` when the target exists. `commit_rename` already surfaces the error via `set_status`, so no caller change is needed.

- [ ] **Step 1: Write failing integration tests**

In `tests/fs_ops_int.rs`, add (note `rename` is already imported):

```rust
#[test]
fn rename_refuses_existing_target() {
    let d = tempdir().unwrap();
    let a = d.path().join("a.txt");
    let b = d.path().join("b.txt");
    fs::write(&a, "1").unwrap();
    fs::write(&b, "2").unwrap();
    let r = rename(&a, "b.txt");
    assert!(r.is_err(), "renaming onto an existing name must error");
    assert_eq!(fs::read_to_string(&b).unwrap(), "2", "target must be untouched");
}

#[test]
fn rename_refuses_path_separator() {
    let d = tempdir().unwrap();
    let p = d.path().join("a.txt");
    fs::write(&p, "1").unwrap();
    assert!(rename(&p, "sub/a.txt").is_err());
    assert!(rename(&p, "").is_err());
    assert!(rename(&p, "..").is_err());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --test fs_ops_int rename_refuses`
Expected: FAIL — current `rename` accepts these.

- [ ] **Step 3: Add validation to `fs_ops::rename`**

Replace `fn rename` body in `src/fs_ops.rs`:

```rust
pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    if new_name.is_empty()
        || new_name == "."
        || new_name == ".."
        || new_name.contains('/')
        || new_name.contains(std::path::MAIN_SEPARATOR)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "name must be a single path component",
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let np = parent.join(new_name);
    if np.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a file with that name already exists",
        ));
    }
    fs::rename(path, &np)?;
    Ok(np)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test fs_ops_int && cargo test --lib`
Expected: PASS, including the existing `rename_changes_name`.

- [ ] **Step 5: Commit**

```bash
git add src/fs_ops.rs tests/fs_ops_int.rs
git commit -m "fix(rename): reject existing targets and path separators"
```

---

### Task 5: Avoid batch-paste collision clobber

**Why:** In the paste worker, each source's destination is computed with `unique_dest` independently before earlier writes land, so two sources resolving to the same `name (1).ext` can overwrite each other.

**Files:**
- Modify: `src/fs_ops.rs` (add `claim_dest` that also avoids already-claimed names)
- Modify: `src/ops.rs` `spawn_paste` worker (track claimed destinations)
- Test: `src/fs_ops.rs` inline tests

**Interfaces:**
- Produces: `fs_ops::claim_dest(dir: &Path, file_name: &OsStr, claimed: &mut HashSet<PathBuf>) -> PathBuf` — like `unique_dest`, but treats any path already in `claimed` as taken, and inserts the chosen path into `claimed` before returning.

- [ ] **Step 1: Write the failing test**

In `src/fs_ops.rs` tests module:

```rust
    #[test]
    fn claim_dest_avoids_already_claimed() {
        use std::collections::HashSet;
        use std::ffi::OsStr;
        let d = tempdir().unwrap();
        let mut claimed: HashSet<std::path::PathBuf> = HashSet::new();
        // Nothing on disk; two claims of the same name must differ.
        let a = claim_dest(d.path(), OsStr::new("f.txt"), &mut claimed);
        let b = claim_dest(d.path(), OsStr::new("f.txt"), &mut claimed);
        assert_ne!(a, b, "second claim must not collide with the first");
        assert_eq!(a.file_name().unwrap(), "f.txt");
        assert_eq!(b.file_name().unwrap(), "f (1).txt");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib claim_dest_avoids_already_claimed`
Expected: FAIL — `claim_dest` not defined.

- [ ] **Step 3: Implement `claim_dest`**

Add to `src/fs_ops.rs` (next to `unique_dest`; ensure `use std::collections::HashSet;` and `use std::ffi::OsStr;` exist or fully-qualify):

```rust
/// Like [`unique_dest`] but also avoids any path already in `claimed`,
/// so a batch of pastes sharing a stem don't resolve to the same name.
/// Inserts the chosen path into `claimed` before returning.
pub fn claim_dest(
    dir: &Path,
    file_name: &std::ffi::OsStr,
    claimed: &mut std::collections::HashSet<PathBuf>,
) -> PathBuf {
    let taken = |p: &Path| p.exists() || claimed.contains(p);
    let direct = dir.join(file_name);
    let chosen = if !taken(&direct) {
        direct
    } else {
        let name = Path::new(file_name);
        let stem = name.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = name.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut i = 1u64;
        loop {
            let cand = dir.join(format!("{stem} ({i}){ext}"));
            if !taken(&cand) {
                break cand;
            }
            i += 1;
        }
    };
    claimed.insert(chosen.clone());
    chosen
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib claim_dest_avoids_already_claimed`
Expected: PASS.

- [ ] **Step 5: Use `claim_dest` in the worker**

In `src/ops.rs` `spawn_paste`, inside `thread::spawn`, before the `for src in &srcs_for_worker` loop (line ~117), add:

```rust
        let mut claimed: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
```

Then change `let dst = unique_dest(&dst_for_worker, fname);` (line ~123) to:

```rust
            let dst = crate::fs_ops::claim_dest(&dst_for_worker, fname, &mut claimed);
```

Leave the `reveal_hint` (line ~108) using `unique_dest` — it is only a best-effort cursor target, and `unique_dest` stays in use there and in its own unit test.

- [ ] **Step 6: Run full suite**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add src/fs_ops.rs src/ops.rs
git commit -m "fix(paste): claim destinations per-batch to avoid collision clobber"
```

---

### Task 6: Fix two reachable panics in `nav`

**Why:** `longest_common` indexes `names[0]` when every candidate lacks a filename (panic). `nav::expand` slices `&bytes[1..i]` inside `${...}` on a byte index that may not be a char boundary, e.g. `${€}` (panic).

**Files:**
- Modify: `src/nav.rs` (`longest_common` guard, `expand` boundary-safe slice)
- Test: `src/nav.rs` inline tests

**Interfaces:**
- No signature changes. (`longest_common`'s unused `input` param is left for a separate cleanup commit to keep this change panic-focused.)

- [ ] **Step 1: Write failing tests**

In `src/nav.rs` tests module:

```rust
    #[test]
    fn longest_common_handles_no_file_names() {
        // candidates non-empty but none has a file_name → must not panic
        let c = vec![PathBuf::from("/")];
        assert_eq!(longest_common("", &c), None);
    }

    #[test]
    fn expand_handles_multibyte_in_braces() {
        // ${€} — multibyte var name must not panic; undefined → empty
        let out = expand("a${€}b");
        assert_eq!(out, "ab");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib nav::tests`
Expected: FAIL with a panic (index out of bounds / byte index not a char boundary).

- [ ] **Step 3: Guard `longest_common`**

In `src/nav.rs`, after building `names` (line ~69), add:

```rust
    if names.is_empty() {
        return None;
    }
```

- [ ] **Step 4: Make `expand`'s brace slice boundary-safe**

In `src/nav.rs` `expand`, replace the `${...}` arm (lines ~14-18):

```rust
        let (name, end) = if bytes.starts_with('{') {
            match bytes.find('}') {
                Some(i) => (&bytes[1..i], start + 1 + i + 1),
                None => break,
            }
        } else {
```

with a version that takes the inner slice by char-safe means:

```rust
        let (name, end) = if let Some(inner) = bytes.strip_prefix('{') {
            match inner.find('}') {
                // inner starts after '{'; `i` is a byte offset into `inner`
                // and `find` returns a char boundary, so this slice is safe.
                Some(i) => (&inner[..i], start + 1 + 1 + i + 1),
                None => break,
            }
        } else {
```

(`start + 1` skips `$`, `+ 1` skips `{`, `+ i` reaches `}`, `+ 1` consumes `}`.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib nav::tests`
Expected: PASS (including the existing `longest_common_prefix_cycle_base` and expand happy-path tests).

- [ ] **Step 6: Commit**

```bash
git add src/nav.rs
git commit -m "fix(nav): guard longest_common empty names and multibyte \${} expansion"
```

---

### Task 7: Surface the real OS error on delete/trash failures

**Why:** `permanent_delete` and `trash_path` failures are recorded as just the path; the user is told "1 failed" with no reason (permission denied vs read-only fs vs busy). The paste worker already includes the error — make these consistent.

**Files:**
- Modify: `src/app.rs` `confirm_yes` (line ~659)
- Modify: `src/dispatch.rs` `trash_targets` (line ~951)
- Test: none (one-line message-formatting change; existing tests cover the success path).

**Interfaces:** none.

- [ ] **Step 1: Include the error in `confirm_yes`**

In `src/app.rs`, change:

```rust
                    Err(_) => failed.push(p.display().to_string()),
```
to:
```rust
                    Err(e) => failed.push(format!("{}: {e}", p.display())),
```

- [ ] **Step 2: Include the error in `trash_targets`**

In `src/dispatch.rs`, change:

```rust
            Err(_) => failed.push(p.display().to_string()),
```
to:
```rust
            Err(e) => failed.push(format!("{}: {e}", p.display())),
```

- [ ] **Step 3: Build + test**

Run: `cargo build 2>&1 | grep -E 'warning|error'; cargo test`
Expected: no warnings/errors; all tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/dispatch.rs
git commit -m "fix(ops): include OS error reason in delete/trash failure messages"
```

---

## Final verification

- [ ] Run `cargo test` — all green.
- [ ] Run `cargo clippy 2>&1 | grep -v 'cargo-home\|explorenova/'` — no warnings on real source.
- [ ] Manual smoke (optional): `dd` shows dimmed rows; `Y` then paste into a terminal yields a bare path; renaming onto an existing name shows "already exists"; cut a dir + paste inside it is refused.

## Notes / explicitly out of scope

- The async-preview and incremental-arena-rebuild performance items from the audit are larger and are **not** in this plan.
- `longest_common`'s dead `input` parameter and the arena-rebuild / batch-summary dedups are left for a separate refactor commit.
- Clipboard format stays single-representation plain text (no `text/uri-list`), per the chosen design — GUI file managers will get a plain path rather than a URI.
