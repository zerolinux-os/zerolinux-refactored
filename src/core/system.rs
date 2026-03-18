// core/system.rs — Live system statistics via sysinfo (no shell parsing)

use std::process::Command;
use sysinfo::{Disks, System};
use log::{debug, warn};

/// All statistics displayed in the dashboard.
#[derive(Clone, Debug, Default)]
pub struct SystemStats {
    // Disk (root partition)
    pub disk_used:    String,
    pub disk_free:    String,
    pub disk_total:   String,
    pub disk_percent: f32,
    // Memory
    pub mem_used:    String,
    pub mem_total:   String,
    pub mem_percent: f32,
    // CPU (0–100 %)
    pub cpu_usage: String,
    // Misc
    pub uptime:            String,
    pub kernel:            String,
    pub packages:          String,
    pub orphans:           String,
    pub cache_size:        String,
    pub updates_available: String,
    pub aur_helper:        String,
    pub firewall_status:   String,
    pub last_update:       String,
}

/// Format bytes into a human-readable string (KB / MB / GB).
fn fmt_bytes(b: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    if b >= GB      { format!("{:.1} GB", b as f64 / GB as f64) }
    else if b >= MB { format!("{:.1} MB", b as f64 / MB as f64) }
    else if b >= KB { format!("{:.1} KB", b as f64 / KB as f64) }
    else            { format!("{} B", b) }
}

/// Format seconds into "Xh Ym" uptime string.
fn fmt_uptime(secs: u64) -> String {
    let days  = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins  = (secs % 3600) / 60;
    if days > 0  { format!("{}d {}h {}m", days, hours, mins) }
    else         { format!("{}h {}m", hours, mins) }
}

/// Run a single binary with explicit args — NO shell, NO string injection.
/// Returns stdout trimmed, or empty string on failure.
fn run_cmd(bin: &str, args: &[&str]) -> String {
    match Command::new(bin).args(args).output() {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        Ok(out) => {
            debug!("Command `{} {:?}` exited non-zero: {}",
                bin, args,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            String::new()
        }
        Err(e) => {
            debug!("Failed to run `{}`: {}", bin, e);
            String::new()
        }
    }
}

/// Check whether a binary exists on PATH.
fn which(bin: &str) -> bool {
    Command::new("which").arg(bin).output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect the first available AUR helper.
pub fn detect_aur_helper() -> Option<String> {
    for helper in &["paru", "yay", "pikaur", "trizen"] {
        if which(helper) {
            return Some(helper.to_string());
        }
    }
    None
}

/// Collect a fresh snapshot of all system stats.
/// All values come from sysinfo or explicit Command calls — never `sh -c`.
pub fn collect_stats() -> SystemStats {
    debug!("Collecting system stats");

    // ── sysinfo ──────────────────────────────────────────
    let mut sys = System::new_all();
    sys.refresh_all();

    // CPU — sysinfo needs two samples with a pause in between to compute
    // real usage; a single refresh always returns 0 on the first call.
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_all();

    let cpu_pct: f32 = {
        let cpus = sys.cpus();
        if cpus.is_empty() { 0.0 }
        else { cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32 }
    };

    // Memory
    let mem_total_b  = sys.total_memory();
    let mem_used_b   = sys.used_memory();
    let mem_percent  = if mem_total_b > 0 { mem_used_b as f32 / mem_total_b as f32 } else { 0.0 };

    // Disk — find root partition
    let disks = Disks::new_with_refreshed_list();
    let (disk_total_b, disk_free_b) = disks
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .map(|d| (d.total_space(), d.available_space()))
        .unwrap_or((0, 0));
    let disk_used_b   = disk_total_b.saturating_sub(disk_free_b);
    let disk_percent  = if disk_total_b > 0 { disk_used_b as f32 / disk_total_b as f32 } else { 0.0 };

    let uptime_str = fmt_uptime(System::uptime());
    let kernel_str = System::kernel_version().unwrap_or_else(|| "unknown".into());

    // ── Pacman / AUR (explicit args, no sh -c) ───────────
    let packages = run_cmd("pacman", &["-Q"]);
    let pkg_count = if packages.is_empty() {
        "?".to_string()
    } else {
        packages.lines().count().to_string()
    };

    // Orphans: `pacman -Qtdq`
    let orphan_out = run_cmd("pacman", &["-Qtdq"]);
    let orphan_count = if orphan_out.trim().is_empty() {
        "0".to_string()
    } else {
        orphan_out.lines().count().to_string()
    };

    // Package cache size — walk /var/cache/pacman/pkg
    let cache_size = cache_dir_size("/var/cache/pacman/pkg");

    // Available updates via checkupdates (pacman-contrib, no root)
    let updates_out = run_cmd("checkupdates", &[]);
    let updates_count = if updates_out.trim().is_empty() {
        "0".to_string()
    } else {
        updates_out.lines().count().to_string()
    };

    // AUR helper detection
    let aur_helper = detect_aur_helper()
        .unwrap_or_else(|| "none".to_string());

    // Firewall status — explicit binary, no shell
    let fw = detect_firewall();

    // Last update from pacman log — read file directly, no grep subprocess
    let last_update = read_last_update();

    debug!("Stats collected: cpu={:.1}% mem={:.1}% disk={:.1}%",
        cpu_pct * 100.0, mem_percent * 100.0, disk_percent * 100.0);

    SystemStats {
        disk_used:         fmt_bytes(disk_used_b),
        disk_free:         fmt_bytes(disk_free_b),
        disk_total:        fmt_bytes(disk_total_b),
        disk_percent,
        mem_used:          fmt_bytes(mem_used_b),
        mem_total:         fmt_bytes(mem_total_b),
        mem_percent,
        cpu_usage:         format!("{:.1}%", cpu_pct),
        uptime:            uptime_str,
        kernel:            kernel_str,
        packages:          pkg_count,
        orphans:           orphan_count,
        cache_size:        fmt_bytes(cache_size),
        updates_available: updates_count,
        aur_helper,
        firewall_status:   fw,
        last_update,
    }
}

/// Walk a directory and return total size in bytes without spawning du.
fn cache_dir_size(path: &str) -> u64 {
    use std::fs;
    let mut total = 0u64;
    let Ok(rd) = fs::read_dir(path) else { return 0; };
    for entry in rd.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() { total += meta.len(); }
        }
    }
    total
}

/// Detect active firewall without shell glue.
fn detect_firewall() -> String {
    // Try ufw
    match Command::new("ufw").arg("status").output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains("Status: active") {
                return "UFW Active".to_string();
            }
        }
        Err(_) => {}
    }
    // Try firewalld
    match Command::new("firewall-cmd").arg("--state").output() {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.trim() == "running" {
                return "firewalld Active".to_string();
            }
        }
        _ => {}
    }
    "Not configured".to_string()
}

/// Read last pacman upgrade date directly from /var/log/pacman.log.
fn read_last_update() -> String {
    use std::fs;
    use std::io::{BufRead, BufReader};

    let Ok(file) = fs::File::open("/var/log/pacman.log") else {
        warn!("Cannot open /var/log/pacman.log");
        return "unknown".to_string();
    };
    let reader = BufReader::new(file);
    let mut last_date = "unknown".to_string();
    for line in reader.lines().flatten() {
        // Lines look like: [2024-05-01T10:23:45+0000] [ALPM] upgraded foo (1.0 -> 2.0)
        if line.contains("[ALPM]") && (line.contains("upgraded") || line.contains("installed")) {
            if let Some(date) = line.get(1..11) {
                last_date = date.to_string();
            }
        }
    }
    last_date
}
