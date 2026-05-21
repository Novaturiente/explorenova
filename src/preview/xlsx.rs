//! Spreadsheet preview via calamine (pure Rust). Renders each worksheet
//! header line ("== Sheet: X (rows x cols) ==") then up to 40 rows by
//! 10 cols of plain text. Supports xlsx / xls / xlsb / ods / xlsm.

use calamine::{open_workbook_auto, Data, Reader};
use std::path::Path;

pub fn is_spreadsheet(ext_lower: &str) -> bool {
    matches!(ext_lower, "xlsx" | "xls" | "xlsb" | "xlsm" | "ods")
}

const ROW_LIMIT: usize = 40;
const COL_LIMIT: usize = 10;

fn cell_to_string(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => format!("{f}"),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERR({e:?})"),
    }
}

pub fn text_from(path: &Path) -> Option<String> {
    let mut wb = open_workbook_auto(path).ok()?;
    let sheets = wb.sheet_names();
    if sheets.is_empty() {
        return Some("(no sheets)".into());
    }
    let mut out = String::new();
    for name in sheets {
        let Ok(range) = wb.worksheet_range(&name) else { continue };
        let (rows, cols) = range.get_size();
        out.push_str(&format!("== {name}  ({rows} × {cols}) ==\n"));
        for row in range.rows().take(ROW_LIMIT) {
            let line: Vec<String> = row
                .iter()
                .take(COL_LIMIT)
                .map(cell_to_string)
                .collect();
            out.push_str(&line.join("\t"));
            out.push('\n');
        }
        if rows > ROW_LIMIT {
            out.push_str(&format!("… ({} more rows)\n", rows - ROW_LIMIT));
        }
        out.push('\n');
    }
    Some(out)
}
