//! Status reporting module for pve-san-fenced daemon.
//!
//! Provides a thread-safe `StatusTracker` to track daemon warnings, errors,
//! and critical fencing decisions, formatting them in a Nagios-compatible format.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::sync::{Arc, OnceLock, RwLock};

/// Nagios-compatible status levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatusLevel {
    /// Service is operating normally
    Ok,
    /// Warning condition detected
    Warning,
    /// Critical failure or fencing decision reached
    Critical,
}

impl fmt::Display for StatusLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusLevel::Ok => write!(f, "OK"),
            StatusLevel::Warning => write!(f, "WARNING"),
            StatusLevel::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// Thread-safe tracker to manage and report daemon status
pub struct StatusTracker {
    status_file: RwLock<Option<String>>,
    active_issues: RwLock<HashMap<String, (StatusLevel, String)>>,
}

impl Default for StatusTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusTracker {
    /// Create a new `StatusTracker` with no status file configured.
    pub fn new() -> Self {
        Self {
            status_file: RwLock::new(None),
            active_issues: RwLock::new(HashMap::new()),
        }
    }

    /// Set the destination path for the status file.
    pub fn set_status_file(&self, path: Option<String>) {
        {
            let mut guard = self.status_file.write().unwrap();
            *guard = path;
        }
        self.write_status_file();
    }

    /// Retrieve the currently configured status file path.
    pub fn status_file(&self) -> Option<String> {
        self.status_file.read().unwrap().clone()
    }

    /// Record an active issue with a specific severity level.
    pub fn set_issue(&self, key: &str, level: StatusLevel, message: String) {
        {
            let mut guard = self.active_issues.write().unwrap();
            guard.insert(key.to_string(), (level, message));
        }
        self.write_status_file();
    }

    /// Clear a previously recorded issue.
    pub fn clear_issue(&self, key: &str) {
        let changed = {
            let mut guard = self.active_issues.write().unwrap();
            guard.remove(key).is_some()
        };
        if changed {
            self.write_status_file();
        }
    }

    /// Clear all active issues matching a given prefix.
    pub fn clear_issues_with_prefix(&self, prefix: &str) {
        let changed = {
            let mut guard = self.active_issues.write().unwrap();
            let before = guard.len();
            guard.retain(|key, _| !key.starts_with(prefix));
            guard.len() != before
        };
        if changed {
            self.write_status_file();
        }
    }

    /// Write the aggregated status line to the status file.
    fn write_status_file(&self) {
        let file_path_guard = self.status_file.read().unwrap();
        let file_path = match &*file_path_guard {
            Some(path) => path,
            None => return,
        };

        let issues = self.active_issues.read().unwrap();
        let (level, message) = if issues.is_empty() {
            (StatusLevel::Ok, "Daemon is running normally".to_string())
        } else {
            let max_level = issues
                .values()
                .map(|(lvl, _)| *lvl)
                .max()
                .unwrap_or(StatusLevel::Ok);

            let mut msgs: Vec<&str> = issues
                .values()
                .filter(|(lvl, _)| *lvl == max_level)
                .map(|(_, msg)| msg.as_str())
                .collect();
            msgs.sort();

            (max_level, msgs.join("; "))
        };

        let content = format!("{level} - {message}\n");
        if let Err(e) = fs::write(file_path, content) {
            log::error!("Failed to write status file '{file_path}': {e}");
        }
    }
}

static TRACKER: OnceLock<Arc<StatusTracker>> = OnceLock::new();

/// Retrieve the global instance of the `StatusTracker`.
pub fn get_status_tracker() -> &'static Arc<StatusTracker> {
    TRACKER.get_or_init(|| Arc::new(StatusTracker::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    #[test]
    fn test_status_tracker_basic() {
        let tracker = StatusTracker::new();

        // Before status file is configured, operations should not panic or fail
        tracker.set_issue("issue1", StatusLevel::Warning, "Warning message".to_string());
        tracker.clear_issue("issue1");

        // Now configure a temp file
        let pid = std::process::id();
        let temp_dir = env::temp_dir().join(format!("status-test-{pid}"));
        fs::create_dir_all(&temp_dir).unwrap();
        let status_file_path = temp_dir.join("pve-san-fenced.status");

        tracker.set_status_file(Some(status_file_path.to_str().unwrap().to_string()));

        // Initial state should be OK
        let content = fs::read_to_string(&status_file_path).unwrap();
        assert_eq!(content, "OK - Daemon is running normally\n");

        // Set a warning issue
        tracker.set_issue("test_warn", StatusLevel::Warning, "First warning".to_string());
        let content = fs::read_to_string(&status_file_path).unwrap();
        assert_eq!(content, "WARNING - First warning\n");

        // Set another warning issue
        tracker.set_issue("another_warn", StatusLevel::Warning, "Second warning".to_string());
        let content = fs::read_to_string(&status_file_path).unwrap();
        // Since we sort the messages, it should be "First warning; Second warning"
        assert_eq!(content, "WARNING - First warning; Second warning\n");

        // Set a critical issue
        tracker.set_issue("critical_fail", StatusLevel::Critical, "Critical error occurred".to_string());
        let content = fs::read_to_string(&status_file_path).unwrap();
        assert_eq!(content, "CRITICAL - Critical error occurred\n");

        // Clear critical issue, should go back to WARNING
        tracker.clear_issue("critical_fail");
        let content = fs::read_to_string(&status_file_path).unwrap();
        assert_eq!(content, "WARNING - First warning; Second warning\n");

        // Clear prefix warnings
        tracker.clear_issues_with_prefix("test_");
        let content = fs::read_to_string(&status_file_path).unwrap();
        assert_eq!(content, "WARNING - Second warning\n");

        // Clear remaining
        tracker.clear_issue("another_warn");
        let content = fs::read_to_string(&status_file_path).unwrap();
        assert_eq!(content, "OK - Daemon is running normally\n");

        // Clean up
        fs::remove_dir_all(&temp_dir).ok();
    }
}

