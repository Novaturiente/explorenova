use std::path::{Path, PathBuf};
use std::process::Command;

pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "wmv", "flv", "m4v", "mpg",
    "mpeg", "3gp", "ts",
];

pub fn is_video(ext_lower: &str) -> bool {
    VIDEO_EXTS.contains(&ext_lower)
}

/// Temp-cache path for a rasterized variant of `src`. Includes the
/// source mtime so an edited file produces a fresh thumbnail.
fn temp_png_for(prefix: &str, src: &Path) -> Option<PathBuf> {
    let mtime = std::fs::metadata(src)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let stem = src.file_stem()?.to_string_lossy();
    Some(std::env::temp_dir().join(format!(
        "explorenova-{prefix}-{stem}-{mtime}.png"
    )))
}

/// Rasterize PDF page 1 to a PNG in the temp dir via `pdftoppm`.
/// Returns None if poppler is not installed or the call fails.
pub fn rasterize_pdf(pdf: &Path) -> Option<PathBuf> {
    let stem = std::env::temp_dir().join(format!(
        "explorenova-pdf-{}",
        pdf.file_stem()?.to_string_lossy()
    ));
    let status = Command::new("pdftoppm")
        .args(["-png", "-singlefile", "-f", "1", "-l", "1"])
        .arg(pdf)
        .arg(&stem)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let png = stem.with_extension("png");
    png.exists().then_some(png)
}

/// Decode AVIF to PNG via `avifdec` (libavif). Returns None if the tool
/// is not installed or the file is malformed. Output cached in temp.
pub fn rasterize_avif(src: &Path) -> Option<PathBuf> {
    let out = temp_png_for("avif", src)?;
    if out.exists() {
        return Some(out);
    }
    let status = Command::new("avifdec").arg(src).arg(&out).status().ok()?;
    if !status.success() {
        return None;
    }
    out.exists().then_some(out)
}

/// First-frame thumbnail of a video via `ffmpegthumbnailer`. Default
/// size 512 px wide; ffmpegthumbnailer outputs PNG when the extension
/// is .png. Returns None if the tool is missing or extraction fails.
pub fn video_thumbnail(src: &Path) -> Option<PathBuf> {
    let out = temp_png_for("video", src)?;
    if out.exists() {
        return Some(out);
    }
    let status = Command::new("ffmpegthumbnailer")
        .args(["-s", "512", "-q", "8", "-i"])
        .arg(src)
        .arg("-o")
        .arg(&out)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    out.exists().then_some(out)
}
