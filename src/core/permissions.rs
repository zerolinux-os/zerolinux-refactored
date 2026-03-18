// core/permissions.rs — Privilege handling without terminal-based sudo.
//
// Strategy:
//   1. Already root → proceed.
//   2. pkexec available → re-launch self via `pkexec env DISPLAY=... <exe>`
//      so the GUI can open on the current session.
//   3. Fallback: sudo -E (preserves environment).
//   4. Nothing works → clear error, exit 1.

use std::process::Command;
use log::{info, warn, error};

/// Returns true if the process is running as root (euid 0).
pub fn is_root() -> bool {
    // SAFETY: geteuid() is a trivial syscall with no side effects.
    unsafe { libc::geteuid() == 0 }
}

/// Ensure we are root before the UI starts.
/// If not root, re-launch self with elevated privileges and exit the
/// current (unprivileged) process.
pub fn ensure_root() {
    if is_root() {
        info!("Running as root — no privilege escalation needed");
        return;
    }

    info!("Not root; attempting privilege escalation");

    let exe = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("/usr/bin/zerolinux-cleaner"));

    // Collect display/session environment that the GUI needs.
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let wayland  = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let xauth    = std::env::var("XAUTHORITY").unwrap_or_default();
    let runtime  = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let dbus     = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();

    let extra: Vec<String> = std::env::args().skip(1).collect();

    // ── Try pkexec first (polkit GUI password dialog) ─────────────────────────
    if command_exists("pkexec") {
        info!("Trying pkexec");
        let status = Command::new("pkexec")
            .arg("env")
            .arg(format!("DISPLAY={}", display))
            .arg(format!("WAYLAND_DISPLAY={}", wayland))
            .arg(format!("XAUTHORITY={}", xauth))
            .arg(format!("XDG_RUNTIME_DIR={}", runtime))
            .arg(format!("DBUS_SESSION_BUS_ADDRESS={}", dbus))
            .arg(&exe)
            .args(&extra)
            .status();

        match status {
            Ok(s) => {
                info!("pkexec finished with: {}", s);
                std::process::exit(s.code().unwrap_or(0));
            }
            Err(e) => {
                warn!("pkexec failed: {} — falling back to sudo", e);
            }
        }
    } else {
        warn!("pkexec not found — falling back to sudo");
    }

    // ── Fallback: sudo -E (preserves the calling environment) ────────────────
    if command_exists("sudo") {
        info!("Trying sudo -E");
        let status = Command::new("sudo")
            .arg("-E")
            .arg(&exe)
            .args(&extra)
            .status();

        match status {
            Ok(s) => {
                info!("sudo finished with: {}", s);
                std::process::exit(s.code().unwrap_or(0));
            }
            Err(e) => {
                error!("sudo also failed: {}", e);
            }
        }
    }

    // ── Nothing worked ────────────────────────────────────────────────────────
    eprintln!(
        "\nZeroLinux Cleaner requires root privileges.\n\
         Please run:  sudo -E ./zerolinux-cleaner\n"
    );
    std::process::exit(1);
}

/// Check if a binary exists on PATH without using `sh -c`.
fn command_exists(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

