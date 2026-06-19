//! Integration tests for mpath-query
//!
//! These tests use the mpath-mockd daemon to test the functionality.
//! Note: Tests must be run serially (--test-threads=1) to avoid socket conflicts.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.
//!
//! This program is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
//! GNU Affero General Public License for more details.
//!
//! You should have received a copy of the GNU Affero General Public License
//! along with this program. If not, see <https://www.gnu.org/licenses/>.

use std::env;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Helper to get the workspace root directory
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the directory containing mpath-query's Cargo.toml
    // which is /home/bzed/workspace/conova/vibe/pve-san-fenced/tools/mpath-query
    // We need to go up two levels to get the workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Get the path to the mpath-mockd binary
fn mockd_path() -> PathBuf {
    workspace_root().join("target/debug/mpath-mockd")
}

/// Get the path to the mpath-query binary
fn query_path() -> PathBuf {
    workspace_root().join("target/debug/mpath-query")
}

/// Get the path to the test data directory
fn test_data_dir() -> PathBuf {
    workspace_root().join("test-data/multipathd")
}

/// Starts the mock daemon with a unique socket for each test
fn start_mock_daemon(test_name: &str) -> (Child, String) {
    let socket_name = format!("@/tmp/test-mpath-mock-{}-{}", test_name, std::process::id());
    let daemon = Command::new(mockd_path())
        .arg("--socket")
        .arg(&socket_name)
        .arg("--test-data-dir")
        .arg(test_data_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start mock daemon");

    (daemon, socket_name)
}

/// Waits for the mock daemon to start listening
fn wait_for_daemon(daemon: &mut Child, socket_path: &str, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();

    while start.elapsed() < timeout {
        if daemon.try_wait().is_ok_and(|o| o.is_some()) {
            return Err("Daemon exited".to_string());
        }

        // Try to connect using mpath-query
        let result = Command::new(query_path())
            .arg("--socket")
            .arg(socket_path)
            .arg("-c")
            .arg("show maps json")
            .arg("-o")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Ok(status) = result {
            if status.success() {
                return Ok(());
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    Err("Timeout waiting for daemon".to_string())
}

#[test]
fn test_default_command_to_stdout() {
    let (mut daemon, socket_path) = start_mock_daemon("test_default");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&socket_path)
        .arg("-c")
        .arg("show maps json")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("major_version"));
            assert!(stdout.contains("minor_version"));
            assert!(stdout.contains("maps"));
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_output_to_file() {
    let (mut daemon, socket_path) = start_mock_daemon("test_output_file");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let output_path = "/tmp/test_mpath_query_output.json";
    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&socket_path)
        .arg("-c")
        .arg("show maps json")
        .arg("-o")
        .arg(output_path)
        .status();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(status) if status.success() => {
            let content = std::fs::read_to_string(output_path).expect("Failed to read output file");
            assert!(content.contains("major_version"));
            assert!(content.contains("maps"));
            std::fs::remove_file(output_path).ok();
        }
        Ok(status) => panic!("Query failed with exit code: {:?}", status.code()),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_custom_command() {
    let (mut daemon, socket_path) = start_mock_daemon("test_custom_cmd");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&socket_path)
        .arg("-c")
        .arg("show status")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(!stdout.is_empty());
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_subcommand() {
    let (mut daemon, socket_path) = start_mock_daemon("test_subcommand");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&socket_path)
        .arg("show-maps-json")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("major_version"));
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_list_maps_command() {
    let (mut daemon, socket_path) = start_mock_daemon("test_list_maps");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&socket_path)
        .arg("-c")
        .arg("list maps")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(!stdout.is_empty());
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_verbose_mode() {
    let (mut daemon, socket_path) = start_mock_daemon("test_verbose");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&socket_path)
        .arg("-v")
        .arg("-c")
        .arg("show maps json")
        .arg("-o")
        .arg("/dev/null")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("Connecting to socket"));
            assert!(stderr.contains("Sending command"));
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}
