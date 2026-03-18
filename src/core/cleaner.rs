// core/cleaner.rs — Cleaning operations.
//
// Rules:
//   • Every Command call uses an explicit binary + arg list — no `sh -c`.
//   • All errors are propagated via Result; nothing is silently swallowed.
//   • The caller (main.rs) decides how to present errors in the UI.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use log::{info, warn, error};

use crate::ui::terminal::{Term, TermHandle};

pub type CleanResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

// ─── Low-level helpers ───────────────────────────────────────────────────────

/// Spawn a binary with explicit args, stream every stdout/stderr line to `term`,
/// and return Ok(()) on success or Err with the exit-code description on failure.
pub fn run_streamed(
    bin: &str,
    args: &[&str],
    term: &Term,
    handle: &TermHandle,
    progress: f32,
) -> CleanResult {
    term.cmd(&format!("$ {} {}", bin, args.join(" ")));
    term.push_to_ui(handle, progress);

    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start `{}`: {}", bin, e))?;

    // Stream stdout
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().flatten() {
            let line = line.trim().to_string();
            if !line.is_empty() {
                term.out(&line);
                term.push_to_ui(handle, progress);
            }
        }
    }
    // Stream stderr (non-fatal; classify by keyword)
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().flatten() {
            let line = line.trim().to_string();
            if !line.is_empty() {
                if line.to_lowercase().contains("error") {
                    term.err(&line);
                } else {
                    term.out(&line);
                }
                term.push_to_ui(handle, progress);
            }
        }
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{}` exited with status: {}", bin, status).into())
    }
}

/// Collect orphan package names via `pacman -Qtdq`.
/// Returns an empty Vec if there are none.
fn list_orphans() -> Vec<String> {
    match Command::new("pacman").args(["-Qtdq"]).output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                // Validate: only allow package name characters
                .filter(|s| s.chars().all(|c| c.is_alphanumeric() || "-_.+@".contains(c)))
                .map(String::from)
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Recursively remove a directory, only if it starts with a known-safe prefix.
fn safe_remove_dir(path: &str, term: &Term) {
    let safe_prefixes = ["/tmp/", "/var/tmp/", "/home/", "/root/", "/var/cache/"];
    if !safe_prefixes.iter().any(|p| path.starts_with(p)) {
        warn!("Refusing to delete outside safe prefix: {}", path);
        term.err(&format!("Skipped unsafe path: {}", path));
        return;
    }
    match std::fs::remove_dir_all(path) {
        Ok(_)  => info!("Removed: {}", path),
        Err(e) => warn!("Could not remove {}: {}", path, e),
    }
}

// ─── Public cleaning operations ──────────────────────────────────────────────

/// Quick clean: orphans → package cache → /tmp.
pub fn quick_clean(term: &Term, handle: &TermHandle) -> CleanResult {
    info!("Starting quick clean");
    term.info("══ Quick Clean ══════════════════════");
    term.push_to_ui(handle, 0.05);

    // 1. Orphan packages
    term.info("Checking for orphan packages...");
    term.push_to_ui(handle, 0.10);
    let orphans = list_orphans();
    if orphans.is_empty() {
        term.ok("No orphan packages found.");
    } else {
        term.info(&format!("Found {} orphan(s) — removing...", orphans.len()));
        // Build arg list: pacman -Rns -- pkg1 pkg2 ...
        let mut args = vec!["-Rns", "--noconfirm", "--"];
        let pkg_refs: Vec<&str> = orphans.iter().map(String::as_str).collect();
        args.extend_from_slice(&pkg_refs);
        match run_streamed("pacman", &args, term, handle, 0.20) {
            Ok(_)  => term.ok(&format!("Removed {} orphan(s).", orphans.len())),
            Err(e) => { error!("{}", e); term.err(&e.to_string()); }
        }
    }
    term.push_to_ui(handle, 0.30);

    // 2. Package cache — keep 1 version, remove uninstalled
    term.info("Cleaning package cache (keeping 1 version)...");
    match run_streamed("paccache", &["-rk1"], term, handle, 0.45) {
        Ok(_)  => {},
        Err(e) => { warn!("paccache -rk1: {}", e); term.err(&e.to_string()); }
    }
    match run_streamed("paccache", &["-ruk0"], term, handle, 0.55) {
        Ok(_)  => {},
        Err(e) => { warn!("paccache -ruk0: {}", e); term.err(&e.to_string()); }
    }
    term.ok("Package cache cleaned.");
    term.push_to_ui(handle, 0.60);

    // 3. /tmp — remove contents, not the directory itself
    term.info("Clearing /tmp...");
    clear_tmp(term);
    term.ok("Temporary files cleared.");
    term.push_to_ui(handle, 1.0);

    info!("Quick clean complete");
    Ok(())
}

/// Full clean: everything in quick_clean plus journal, browser/dev caches.
pub fn full_clean(term: &Term, handle: &TermHandle) -> CleanResult {
    quick_clean(term, handle)?;

    info!("Starting full clean extras");
    term.info("══ Full Clean (extended) ════════════");
    term.push_to_ui(handle, 0.05);

    // 4. Systemd journal — vacuum to 7 days
    term.info("Vacuuming systemd journal (keep 7 days)...");
    match run_streamed("journalctl", &["--vacuum-time=7d"], term, handle, 0.20) {
        Ok(_)  => term.ok("Journal cleaned."),
        Err(e) => { warn!("{}", e); term.err(&e.to_string()); }
    }

    // 5. Browser caches (Firefox, Chromium) — use std::fs, not rm -rf shell
    term.info("Cleaning browser caches...");
    clean_browser_caches(term);
    term.ok("Browser caches cleared.");
    term.push_to_ui(handle, 0.45);

    // 6. Python bytecode
    term.info("Cleaning Python bytecode caches...");
    clean_pycache(term);
    term.ok("Python caches cleared.");
    term.push_to_ui(handle, 0.60);

    // 7. Thumbnails
    term.info("Clearing thumbnail caches...");
    clean_thumbnails(term);
    term.ok("Thumbnails cleared.");
    term.push_to_ui(handle, 0.72);

    // 8. systemd reset-failed (non-fatal)
    let _ = run_streamed("systemctl", &["reset-failed"], term, handle, 0.80);
    term.ok("systemd reset-failed done.");

    // 9. AUR helper cache
    if let Some(aur) = crate::core::system::detect_aur_helper() {
        term.info(&format!("Cleaning {} cache...", aur));
        match run_streamed(&aur, &["-Sc", "--noconfirm"], term, handle, 0.92) {
            Ok(_)  => term.ok(&format!("{} cache cleaned.", aur)),
            Err(e) => { warn!("{}", e); term.err(&e.to_string()); }
        }
    }

    term.push_to_ui(handle, 1.0);
    info!("Full clean complete");
    Ok(())
}

// ─── Private helpers ─────────────────────────────────────────────────────────

/// Remove everything inside /tmp using std::fs (no shell).
fn clear_tmp(_term: &Term) {
    for dir in &["/tmp", "/var/tmp"] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let path_str = path.to_string_lossy();
                let result = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(e) = result {
                    // Non-fatal — /tmp may have files locked by running processes
                    warn!("Could not remove {}: {}", path_str, e);
                }
            }
        }
    }
}

/// Walk home directories and remove Firefox/Chromium cache dirs.
fn clean_browser_caches(term: &Term) {
    let targets = [
        // Firefox cache
        ".cache/mozilla/firefox",
        ".mozilla/firefox",          // profiles/*/Cache2
        // Chromium / Chrome
        ".cache/chromium",
        ".cache/google-chrome",
    ];
    if let Ok(home_dirs) = std::fs::read_dir("/home") {
        for user_entry in home_dirs.flatten() {
            let base = user_entry.path();
            for rel in &targets {
                let candidate = base.join(rel);
                if candidate.exists() {
                    safe_remove_dir(&candidate.to_string_lossy(), term);
                }
            }
        }
    }
}

/// Remove __pycache__ directories under /home.
fn clean_pycache(term: &Term) {
    walk_and_remove_named("/home", "__pycache__", 8, term);
}

/// Remove thumbnail cache directories under /home.
fn clean_thumbnails(term: &Term) {
    walk_and_remove_named("/home", "thumbnails", 6, term);
}

/// Recursively walk `root`, removing directories named `target_name`
/// up to `max_depth` levels deep. Safe-prefix checked inside safe_remove_dir.
fn walk_and_remove_named(root: &str, target_name: &str, max_depth: usize, term: &Term) {
    fn recurse(
        path: &std::path::Path,
        target: &str,
        depth: usize,
        max: usize,
        term: &Term,
    ) {
        if depth > max { return; }
        let Ok(entries) = std::fs::read_dir(path) else { return; };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().map(|n| n == target).unwrap_or(false) {
                    safe_remove_dir(&p.to_string_lossy(), term);
                } else {
                    recurse(&p, target, depth + 1, max, term);
                }
            }
        }
    }
    recurse(std::path::Path::new(root), target_name, 0, max_depth, term);
}
