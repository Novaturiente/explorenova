# Maintainer: Nova <eecglobal.dev@gmail.com>

pkgname=explorenova
pkgver=0.1.0.r150.98bc772a
pkgrel=1
pkgdesc='Daily-driver Rust TUI file manager (ratatui + crossterm)'
arch=('x86_64')
url='https://github.com/Novaturiente/explorenova'
license=('MIT')
depends=(
  'gcc-libs' 'glibc'
  # Native C libraries linked dynamically (vendored builds fail under
  # makepkg's CachyOS toolchain — force system libs via env in build()).
  'libgit2' 'libssh2' 'oniguruma' 'openssl' 'zlib'
  # Archive extraction (Enter on .tar/.zip/.7z/.gz/.bz2/.xz/.zst).
  'tar' 'gzip' 'bzip2' 'xz' 'zstd' 'unzip' 'p7zip'
  # Preview pipeline (PDF/AVIF/video thumbnails).
  'poppler' 'libavif' 'ffmpegthumbnailer'
  # Devices modal mount/unmount/list.
  'udiskie'
)
makedepends=('cargo' 'git' 'pkgconf')
optdepends=(
  'unrar: extract .rar archives (non-free, not pulled by default)'
  'wl-clipboard: copy-paths under Wayland (wl-copy)'
  'xclip: copy-paths under X11'
)
provides=("$pkgname")
conflicts=("$pkgname")
# Build straight from the local checkout: run `makepkg -si` inside the
# repo. Cloning .git via file:// pulls the current branch HEAD; commit
# any pending edits first or they will not land in the built package.
source=("$pkgname::git+file://$startdir/.git")
sha256sums=('SKIP')

pkgver() {
  cd "$srcdir/$pkgname"
  # Format: <cargo-version>.r<commit-count>.<short-sha> so reinstalls
  # after new commits are seen as upgrades by pacman.
  local cargo_ver
  cargo_ver=$(awk -F'"' '/^version[[:space:]]*=/ {print $2; exit}' Cargo.toml)
  printf '%s.r%s.%s' \
    "$cargo_ver" \
    "$(git rev-list --count HEAD)" \
    "$(git rev-parse --short=8 HEAD)"
}

prepare() {
  cd "$srcdir/$pkgname"
  # Vendor + lock the dep graph so the build is deterministic and works
  # without network access in the sandbox.
  export CARGO_HOME="$srcdir/cargo-home"
  cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

# Shared env: link against system libgit2/libssh2/oniguruma/openssl
# instead of the crate-vendored copies. The vendored builds compile fine
# but emit no -l<name> flag on CachyOS, leaving rust-lld with undefined
# C symbols at final link.
_export_system_libs() {
  export CARGO_HOME="$srcdir/cargo-home"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  export LIBGIT2_SYS_USE_PKG_CONFIG=1
  export LIBSSH2_SYS_USE_PKG_CONFIG=1
  export OPENSSL_NO_VENDOR=1
  export RUSTONIG_SYSTEM_LIBONIG=1
}

build() {
  cd "$srcdir/$pkgname"
  _export_system_libs
  cargo build --frozen --release --bin "$pkgname"
}

check() {
  cd "$srcdir/$pkgname"
  _export_system_libs
  cargo test --frozen --release
}

package() {
  cd "$srcdir/$pkgname"
  install -Dm755 "target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
