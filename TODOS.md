# TODOS

## Distribution pipeline — make explorenova installable by non-developers

**What:** Ship explorenova so strangers can install it without cloning and
building: GitHub Releases with prebuilt binaries via CI, an AUR package, and
optionally crates.io.

**Why:** Biggest "open source / users" gap. Today the only way for someone else
to use explorenova is to install Rust and `cargo build`. A distribution pipeline
means a downloaded binary or a one-line package-manager install.

**Pros:** Turns the repo from "mine" into "installable by anyone." Pairs
naturally with the configuration feature (a configurable app people can install).

**Cons:** Real CI/release machinery, cross-platform build matrix
(linux/darwin, amd64/arm64), version/tag discipline. A separate chunk of work.

**Context:** `PKGBUILD` already exists at repo root (versioned `pkgver`, local
`makepkg` only — the `src/cargo-home/` and `src/explorenova/` build artifacts
are from it). No `.github/workflows` or release automation exists. Single-crate
cargo binary, so cross-compile is tractable (`cargo-dist` or a hand-rolled
release workflow).

**Depends on / blocked by:** Nothing. Independent of the config feature.

**Surfaced by:** /office-hours (D2) + /plan-eng-review distribution check,
2026-06-20.
