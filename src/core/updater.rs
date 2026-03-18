// core/updater.rs — System update operations.
//
// All commands use explicit binary + arg list — no `sh -c`.
// AUR updates run as the original (non-root) user via `sudo -u <user>`.

use std::process::Command;
use log::{info, warn, error};

use crate::core::cleaner::run_streamed;
use crate::core::system::detect_aur_helper;
use crate::ui::terminal::{Term, TermHandle};

pub type UpdateResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Determine the non-root user who launched the app (from SUDO_USER env var).
fn original_user() -> Option<String> {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|s| !s.is_empty())
        // Safety: only allow plain usernames (no shell meta-characters)
        .filter(|s| s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-'))
}

/// Check for available updates without installing them.
/// Returns a list of "pkg old -> new" strings.
pub fn check_updates() -> Vec<String> {
    match Command::new("checkupdates").output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        }
        Err(e) => {
            warn!("checkupdates failed: {}", e);
            Vec::new()
        }
    }
}

/// Full system update: pacman → AUR helper (if present) → Flatpak (if present).
pub fn system_update(term: &Term, handle: &TermHandle) -> UpdateResult {
    info!("Starting system update");
    term.info("══ System Update ════════════════════");
    term.push_to_ui(handle, 0.05);

    // 1. Sync package databases
    term.info("Syncing package databases...");
    match run_streamed("pacman", &["-Sy"], term, handle, 0.10) {
        Ok(_)  => term.ok("Databases synced."),
        Err(e) => {
            error!("pacman -Sy failed: {}", e);
            term.err(&format!("Database sync failed: {}", e));
            // Non-fatal — continue anyway
        }
    }

    // 2. Check what needs updating
    let updates = check_updates();
    if updates.is_empty() {
        term.ok("System is already up to date!");
        term.push_to_ui(handle, 1.0);
        return Ok(());
    }

    let total = updates.len();
    term.info(&format!("{} package(s) to upgrade:", total));
    for line in updates.iter().take(30) {
        term.out(&format!("  {}", line));
    }
    if total > 30 {
        term.out(&format!("  … and {} more", total - 30));
    }
    term.push_to_ui(handle, 0.30);

    // 3. Upgrade official packages
    term.info("Upgrading system packages...");
    match run_streamed("pacman", &["-Su", "--noconfirm"], term, handle, 0.65) {
        Ok(_)  => term.ok("System packages upgraded."),
        Err(e) => {
            error!("pacman -Su failed: {}", e);
            term.err(&format!("Upgrade error: {}", e));
        }
    }

    // 4. AUR helper
    if let Some(aur) = detect_aur_helper() {
        term.info(&format!("Updating AUR packages with {}...", aur));

        // AUR helpers must NOT run as root; use original user.
        match original_user() {
            Some(user) => {
                // sudo -u <user> <aur-helper> -Su --noconfirm
                match run_streamed(
                    "sudo",
                    &["-u", &user, &aur, "-Su", "--noconfirm"],
                    term, handle, 0.85,
                ) {
                    Ok(_)  => term.ok(&format!("AUR updated via {}.", aur)),
                    Err(e) => { warn!("{}", e); term.err(&e.to_string()); }
                }
            }
            None => {
                term.err("Cannot determine non-root user for AUR update; skipping.");
                warn!("SUDO_USER not set — skipping AUR update");
            }
        }
    }

    // 5. Flatpak (if installed) — runs fine as root
    if Command::new("which").arg("flatpak").output()
        .map(|o| o.status.success()).unwrap_or(false)
    {
        term.info("Updating Flatpak applications...");
        match run_streamed("flatpak", &["update", "--noninteractive"], term, handle, 0.95) {
            Ok(_)  => term.ok("Flatpak applications updated."),
            Err(e) => { warn!("{}", e); term.err(&e.to_string()); }
        }
    }

    term.push_to_ui(handle, 1.0);
    info!("System update complete");
    Ok(())
}
