// ui/terminal.rs — In-memory terminal log, pushed to the Slint UI.
//
// Uses std::sync::Mutex for interior mutability (no unsafe).
// The Weak<MainWindow> handle is cloned cheaply and used only
// from the background thread via slint::invoke_from_event_loop.

use std::sync::{Arc, Mutex};
use log::debug;

/// A single line in the operation output terminal.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub text:  String,
    pub level: String, // "info" | "success" | "error" | "cmd" | "output"
}

/// Shared log buffer — cheaply cloneable, safe across threads.
#[derive(Clone)]
pub struct Term {
    entries: Arc<Mutex<Vec<LogEntry>>>,
}

/// Opaque handle to the Slint UI — passed by reference to avoid Clone bounds.
pub type TermHandle = slint::Weak<crate::MainWindow>;

impl Term {
    pub fn new() -> Self {
        Self { entries: Arc::new(Mutex::new(Vec::new())) }
    }

    fn add(&self, text: &str, level: &str) {
        debug!("[{}] {}", level, text);
        if let Ok(mut v) = self.entries.lock() {
            v.push(LogEntry { text: text.into(), level: level.into() });
        }
    }

    pub fn info(&self, t: &str)   { self.add(t, "info"); }
    pub fn ok(&self,   t: &str)   { self.add(t, "success"); }
    pub fn err(&self,  t: &str)   { self.add(t, "error"); }
    pub fn cmd(&self,  t: &str)   { self.add(t, "cmd"); }
    pub fn out(&self,  t: &str)   { self.add(t, "output"); }

    /// Send current entries + progress to the Slint UI (always from any thread).
    pub fn push_to_ui(&self, handle: &TermHandle, progress: f32) {
        // Collect while the lock is held, then release before the async dispatch
        let entries: Vec<crate::TermEntry> = self.entries
            .lock()
            .map(|v| {
                v.iter().map(|e| crate::TermEntry {
                    text:  e.text.clone().into(),
                    level: e.level.clone().into(),
                }).collect()
            })
            .unwrap_or_default();

        let h = handle.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = h.upgrade() {
                ui.set_term_entries(entries.as_slice().into());
                ui.set_progress(progress);
            }
        });
    }

    /// Snapshot all entries (used at operation completion).
    pub fn snapshot(&self) -> Vec<crate::TermEntry> {
        self.entries
            .lock()
            .map(|v| {
                v.iter().map(|e| crate::TermEntry {
                    text:  e.text.clone().into(),
                    level: e.level.clone().into(),
                }).collect()
            })
            .unwrap_or_default()
    }
}

impl Default for Term {
    fn default() -> Self { Self::new() }
}
