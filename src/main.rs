// main.rs — ZeroLinux Cleaner v3.0
//
// Architecture:
//   • Slint UI runs on the main thread (required by most GUI toolkits).
//   • All blocking operations run on a Tokio thread-pool thread via
//     tokio::task::spawn_blocking.
//   • UI updates cross back to the main thread through
//     slint::invoke_from_event_loop — the only safe way to touch Slint from
//     a background thread.
//   • No `sh -c`, no unsafe block (except the single libc::geteuid() in
//     permissions.rs which is documented there).

slint::include_modules!();

mod core;
mod ui;
mod utils;

use core::{cleaner, permissions, system, updater};
use ui::terminal::Term;
use utils::disk_analyzer;

use std::time::Duration;
use log::{error, info};

// ─── Tokio runtime (used only for background tasks) ──────────────────────────

fn spawn_bg<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    // Each operation gets its own blocking thread; never blocks the UI.
    std::thread::spawn(f);
}

// ─── Finish helper: push final state to UI after an operation ────────────────

fn finish(handle: slint::Weak<MainWindow>, term: Term) {
    term.info("─────────────────────────────────────");
    term.ok("Done! Refreshing stats...");

    let entries = term.snapshot();
    let fresh   = system::collect_stats();

    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = handle.upgrade() {
            ui.set_term_entries(entries.as_slice().into());
            ui.set_stats(stats_to_slint(&fresh));
            ui.set_progress(1.0);
            ui.set_busy(false);
        }
    });
}

// ─── Convert domain struct → Slint-generated struct ─────────────────────────

fn stats_to_slint(s: &system::SystemStats) -> StatData {
    StatData {
        disk_used:         s.disk_used.clone().into(),
        disk_free:         s.disk_free.clone().into(),
        disk_total:        s.disk_total.clone().into(),
        disk_percent:      s.disk_percent,
        mem_used:          s.mem_used.clone().into(),
        mem_total:         s.mem_total.clone().into(),
        mem_percent:       s.mem_percent,
        cpu_usage:         s.cpu_usage.clone().into(),
        uptime:            s.uptime.clone().into(),
        kernel:            s.kernel.clone().into(),
        packages:          s.packages.clone().into(),
        orphans:           s.orphans.clone().into(),
        cache_size:        s.cache_size.clone().into(),
        updates_available: s.updates_available.clone().into(),
        aur_helper:        s.aur_helper.clone().into(),
        firewall_status:   s.firewall_status.clone().into(),
        last_update:       s.last_update.clone().into(),
    }
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() -> Result<(), slint::PlatformError> {
    // Initialise env_logger; default to "info" if RUST_LOG is not set.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    info!("ZeroLinux Cleaner v3.0 starting");

    // Escalate privileges via pkexec if not root; exits process on failure.
    permissions::ensure_root();

    let ui = MainWindow::new()?;

    // ── Load initial stats in background ─────────────────────────────────────
    {
        let h = ui.as_weak();
        spawn_bg(move || {
            let s = system::collect_stats();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = h.upgrade() {
                    ui.set_stats(stats_to_slint(&s));
                }
            });
        });
    }

    // ── Auto-refresh stats every 5 s (only when idle) ────────────────────────
    {
        let h = ui.as_weak();
        spawn_bg(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let s = system::collect_stats();
            let h2 = h.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = h2.upgrade() {
                    // Skip update if an operation is in progress
                    if !ui.get_busy() {
                        ui.set_stats(stats_to_slint(&s));
                    }
                }
            });
        });
    }

    // ── Manual refresh button ─────────────────────────────────────────────────
    {
        let h = ui.as_weak();
        ui.on_refresh_stats(move || {
            // Show "Refreshing..." immediately so user gets feedback
            if let Some(ui) = h.upgrade() {
                ui.set_refreshing(true);
            }
            let h2 = h.clone();
            spawn_bg(move || {
                let s = system::collect_stats();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = h2.upgrade() {
                        ui.set_stats(stats_to_slint(&s));
                        ui.set_refreshing(false);
                    }
                });
            });
        });
    }

    // ── Disk Analyzer ─────────────────────────────────────────────────────────
    {
        let h = ui.as_weak();
        ui.on_analyze_disk(move || {
            let h2 = h.clone();
            spawn_bg(move || {
                // Scan the most common large directories
                let mut all: Vec<disk_analyzer::DirEntry> = Vec::new();
                for root in &["/var", "/home", "/usr", "/opt"] {
                    all.extend(disk_analyzer::largest_dirs(root, 5));
                }
                all.sort_by(|a, b| b.size.cmp(&a.size));
                all.truncate(10);

                let entries: Vec<DiskEntry> = all.iter().map(|e| DiskEntry {
                    path:  e.path.clone().into(),
                    human: e.human.clone().into(),
                }).collect();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = h2.upgrade() {
                        ui.set_disk_entries(entries.as_slice().into());
                    }
                });
            });
        });
    }

    // ── Quick Clean ───────────────────────────────────────────────────────────
    {
        let h = ui.as_weak();
        ui.on_do_quick_clean(move || {
            if let Some(ui) = h.upgrade() {
                if ui.get_busy() { return; }
                ui.set_busy(true);
                ui.set_progress(0.0);
                ui.set_term_entries([].as_slice().into());
            }
            let h2 = h.clone();
            spawn_bg(move || {
                let term = Term::new();
                if let Err(e) = cleaner::quick_clean(&term, &h2) {
                    error!("Quick clean error: {}", e);
                    term.err(&format!("Fatal error: {}", e));
                }
                finish(h2, term);
            });
        });
    }

    // ── Full Clean ────────────────────────────────────────────────────────────
    {
        let h = ui.as_weak();
        ui.on_do_full_clean(move || {
            if let Some(ui) = h.upgrade() {
                if ui.get_busy() { return; }
                ui.set_busy(true);
                ui.set_progress(0.0);
                ui.set_term_entries([].as_slice().into());
            }
            let h2 = h.clone();
            spawn_bg(move || {
                let term = Term::new();
                if let Err(e) = cleaner::full_clean(&term, &h2) {
                    error!("Full clean error: {}", e);
                    term.err(&format!("Fatal error: {}", e));
                }
                finish(h2, term);
            });
        });
    }

    // ── System Update ─────────────────────────────────────────────────────────
    {
        let h = ui.as_weak();
        ui.on_do_update(move || {
            if let Some(ui) = h.upgrade() {
                if ui.get_busy() { return; }
                ui.set_busy(true);
                ui.set_progress(0.0);
                ui.set_term_entries([].as_slice().into());
            }
            let h2 = h.clone();
            spawn_bg(move || {
                let term = Term::new();
                if let Err(e) = updater::system_update(&term, &h2) {
                    error!("Update error: {}", e);
                    term.err(&format!("Fatal error: {}", e));
                }
                finish(h2, term);
            });
        });
    }

    // ── One-Click Maintenance (Update + Full Clean) ───────────────────────────
    {
        let h = ui.as_weak();
        ui.on_do_update_clean(move || {
            if let Some(ui) = h.upgrade() {
                if ui.get_busy() { return; }
                ui.set_busy(true);
                ui.set_progress(0.0);
                ui.set_term_entries([].as_slice().into());
            }
            let h2 = h.clone();
            spawn_bg(move || {
                let term = Term::new();
                term.info("══ One-Click Maintenance ════════════");

                // Phase 1: update
                if let Err(e) = updater::system_update(&term, &h2) {
                    error!("Update phase error: {}", e);
                    term.err(&format!("Update error: {}", e));
                }

                // Nudge progress bar to mid-point between phases
                let h3 = h2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = h3.upgrade() { ui.set_progress(0.5); }
                });

                // Phase 2: full clean
                if let Err(e) = cleaner::full_clean(&term, &h2) {
                    error!("Clean phase error: {}", e);
                    term.err(&format!("Clean error: {}", e));
                }

                finish(h2, term);
            });
        });
    }

    // ── Scheduler toggle ──────────────────────────────────────────────────────
    {
        ui.on_toggle_scheduler(move |enabled, interval| {
            // Validate interval against a fixed set — no user string ever reaches a command.
            let cal = match interval.as_str() {
                "daily"   => "daily",
                "monthly" => "monthly",
                _         => "weekly",
            };

            spawn_bg(move || {
                if enabled {
                    let unit = format!(
                        "[Unit]\nDescription=ZeroLinux Auto Cleaner\n\n\
                         [Timer]\nOnCalendar={}\nPersistent=true\n\n\
                         [Install]\nWantedBy=timers.target\n",
                        cal
                    );
                    if let Err(e) = std::fs::write(
                        "/etc/systemd/system/zerolinux-cleaner.timer", &unit
                    ) {
                        error!("Failed to write timer unit: {}", e);
                        return;
                    }
                    // systemctl calls — explicit args, no shell
                    let _ = std::process::Command::new("systemctl")
                        .args(["daemon-reload"]).output();
                    let _ = std::process::Command::new("systemctl")
                        .args(["enable", "--now", "zerolinux-cleaner.timer"]).output();
                    info!("Scheduler enabled: {}", cal);
                } else {
                    let _ = std::process::Command::new("systemctl")
                        .args(["disable", "--now", "zerolinux-cleaner.timer"]).output();
                    info!("Scheduler disabled");
                }
            });
        });
    }

    info!("UI running");
    ui.run()
}
