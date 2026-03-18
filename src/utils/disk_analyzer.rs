// utils/disk_analyzer.rs — Disk usage analysis.
//
// Uses pure Rust std::fs — no `du`, no shell.

use std::path::Path;
use log::warn;

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path:  String,
    pub size:  u64,
    pub human: String,
}

/// Format bytes to a human-readable string.
fn fmt_bytes(b: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    if b >= GB      { format!("{:.1} GB", b as f64 / GB as f64) }
    else if b >= MB { format!("{:.1} MB", b as f64 / MB as f64) }
    else if b >= KB { format!("{:.1} KB", b as f64 / KB as f64) }
    else            { format!("{} B",     b) }
}

/// Recursively compute size of a directory (non-blocking is the caller's job).
pub fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    let Ok(rd) = std::fs::read_dir(path) else { return 0; };
    for entry in rd.flatten() {
        let p = entry.path();
        match entry.metadata() {
            Ok(m) if m.is_file() => total += m.len(),
            Ok(m) if m.is_dir()  => total += dir_size(&p),
            _ => {}
        }
    }
    total
}

/// Return the largest top-level directories inside `root`,
/// sorted descending by size.  Scans only one level deep to stay responsive.
pub fn largest_dirs(root: &str, limit: usize) -> Vec<DirEntry> {
    let mut results: Vec<DirEntry> = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        warn!("Cannot read directory: {}", root);
        return results;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() { continue; }
        let size = dir_size(&p);
        results.push(DirEntry {
            path:  p.to_string_lossy().to_string(),
            size,
            human: fmt_bytes(size),
        });
    }
    results.sort_by(|a, b| b.size.cmp(&a.size));
    results.truncate(limit);
    results
}
