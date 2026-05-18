use std::path::{Path, PathBuf};
use std::process::Command;

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
