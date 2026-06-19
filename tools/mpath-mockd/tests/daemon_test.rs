//! Tests for mpath-mockd daemon
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
    // CARGO_MANIFEST_DIR is the directory containing mpath-mockd's Cargo.toml
    // which is /home/bzed/workspace/conova/vibe/pve-san-fenced/tools/mpath-mockd
    // We need to go up two levels to get the workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .to_path_buf()
}

/// Get the path to the mpath-mockd binary
fn daemon_path() -> PathBuf {
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
fn start_test_daemon(test_name: &str) -> (Child, String) {
    let socket_name = format!("@/tmp/test-mpath-mockd-{}-{}", test_name, std::process::id());
    let daemon = Command::new(daemon_path())
        .arg("--socket")
        .arg(&socket_name)
        .arg("--test-data-dir")
        .arg(test_data_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start test daemon");

    (daemon, socket_name)
}

/// Waits for the daemon to start by testing connectivity
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
fn test_daemon_starts() {
    let (mut daemon, socket_path) = start_test_daemon("test_daemon_starts");

    assert!(wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_ok(), "Daemon should start");

    daemon.kill().ok();
    daemon.wait().ok();
}

#[test]
fn test_daemon_responds_to_command() {
    let (mut daemon, socket_path) = start_test_daemon("test_daemon_responds");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Daemon did not start in time");
    }

    // Use mpath-query to test the daemon
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
            assert!(stdout.contains("major_version"), "Response should contain major_version");
            assert!(stdout.contains("maps"), "Response should contain maps");
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_daemon_handles_unknown_command() {
    let (mut daemon, socket_path) = start_test_daemon("test_daemon_unknown");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Daemon did not start in time");
    }

    // Use mpath-query to test with unknown command
    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&socket_path)
        .arg("-c")
        .arg("unknown command")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Should return some response (possibly error or default)
            assert!(!stdout.is_empty(), "Response should not be empty");
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_daemon_handles_multiple_commands() {
    let (mut daemon, socket_path) = start_test_daemon("test_daemon_multi");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Daemon did not start in time");
    }

    // Test multiple commands
    let commands = ["show maps json", "show status", "list maps"];
    for cmd in commands.iter() {
        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&socket_path)
            .arg("-c")
            .arg(cmd)
            .arg("-o")
            .arg("/dev/null")
            .status();

        assert!(result.is_ok(), "Failed to run query for command: {}", cmd);
        assert!(result.unwrap().success(), "Query failed for command: {}", cmd);
    }

    daemon.kill().ok();
    daemon.wait().ok();
}

#[test]
fn test_daemon_custom_socket() {
    let custom_socket = format!("@/tmp/test-mpath-mockd-custom-{}", std::process::id());
    let mut daemon = Command::new(daemon_path())
        .arg("--socket")
        .arg(&custom_socket)
        .arg("--test-data-dir")
        .arg(test_data_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start daemon with custom socket");

    // Wait for daemon to start
    let start = Instant::now();
    let mut ready = false;
    while start.elapsed() < Duration::from_secs(2) {
        if daemon.try_wait().is_ok_and(|o| o.is_some()) {
            panic!("Daemon exited");
        }

        // Try to connect using mpath-query
        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&custom_socket)
            .arg("-c")
            .arg("show maps json")
            .arg("-o")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Ok(status) = result {
            if status.success() {
                ready = true;
                break;
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(ready, "Daemon with custom socket should be ready");

    daemon.kill().ok();
    daemon.wait().ok();
}

#[test]
fn test_daemon_default_file_for_show_maps_json() {
    // Start daemon with explicit file-map to ensure we get all_active_running.json first
    let custom_socket = format!("@/tmp/test-mpath-mockd-default-{}", std::process::id());
    let mut daemon = Command::new(daemon_path())
        .arg("--socket")
        .arg(&custom_socket)
        .arg("--test-data-dir")
        .arg(test_data_dir())
        .arg("--file-map")
        .arg("show maps json=show_maps_json/all_active_running.json")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start daemon");

    // Wait for daemon to start
    let start = Instant::now();
    let mut ready = false;
    while start.elapsed() < Duration::from_secs(2) {
        if daemon.try_wait().is_ok_and(|o| o.is_some()) {
            panic!("Daemon exited");
        }

        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&custom_socket)
            .arg("-c")
            .arg("show maps json")
            .arg("-o")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Ok(status) = result {
            if status.success() {
                ready = true;
                break;
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(ready, "Daemon should be ready");

    // Query for show maps json - should return all_active_running.json
    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&custom_socket)
        .arg("-c")
        .arg("show maps json")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // all_active_running.json has active paths, not failed ones
            assert!(stdout.contains("\"paths\" : 16"), "Default response should have 16 paths from all_active_running.json");
            assert!(stdout.contains("\"dm_st\" : \"active\""), "Default response should have active paths");
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_daemon_custom_file_mapping() {
    let custom_socket = format!("@/tmp/test-mpath-mockd-custom-map-{}", std::process::id());
    let mut daemon = Command::new(daemon_path())
        .arg("--socket")
        .arg(&custom_socket)
        .arg("--test-data-dir")
        .arg(test_data_dir())
        .arg("--file-map")
        .arg("show maps json=show_maps_json/failed_all_timeout.json")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start daemon with custom file mapping");

    // Wait for daemon to start
    let start = Instant::now();
    let mut ready = false;
    while start.elapsed() < Duration::from_secs(2) {
        if daemon.try_wait().is_ok_and(|o| o.is_some()) {
            panic!("Daemon exited");
        }

        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&custom_socket)
            .arg("-c")
            .arg("show maps json")
            .arg("-o")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Ok(status) = result {
            if status.success() {
                ready = true;
                break;
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(ready, "Daemon with custom file mapping should be ready");

    // Query for show maps json - should return failed_all_timeout.json
    let result = Command::new(query_path())
        .arg("--socket")
        .arg(&custom_socket)
        .arg("-c")
        .arg("show maps json")
        .output();

    daemon.kill().ok();
    daemon.wait().ok();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // failed_all_timeout.json has 0 paths and all are failed/timeout
            assert!(stdout.contains("\"paths\" : 0"), "Custom response should have 0 paths from failed_all_timeout.json");
            assert!(stdout.contains("\"chk_st\" : \"i/o timeout\""), "Custom response should have timeout status");
        }
        Ok(output) => panic!("Query failed: {}", String::from_utf8_lossy(&output.stderr)),
        Err(e) => panic!("Failed to run query: {}", e),
    }
}

#[test]
fn test_daemon_all_commands() {
    let (mut daemon, socket_path) = start_test_daemon("test_daemon_all_commands");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Daemon did not start in time");
    }

    // Test all known commands
    let test_cases = vec![
        ("show maps json", vec!["major_version", "maps"]),
        ("show topology", vec!["create:", "mpatha"]),
        ("list maps", vec!["name", "sysfs", "uuid"]),
        ("show status", vec!["paths:", "busy"]),
        ("show config", vec!["defaults", "blacklist"]),
    ];

    for (command, expected_contents) in test_cases {
        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&socket_path)
            .arg("-c")
            .arg(command)
            .output();

        match result {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for expected in expected_contents {
                    assert!(stdout.contains(expected),
                        "Response for '{}' should contain '{}'", command, expected);
                }
            }
            Ok(output) => panic!("Query failed for '{}': {}", command, String::from_utf8_lossy(&output.stderr)),
            Err(e) => panic!("Failed to run query for '{}': {}", command, e),
        }
    }

    daemon.kill().ok();
    daemon.wait().ok();
}

#[test]
fn test_daemon_cycles_through_files() {
    // Use --file-map to specify two files that should be cycled
    let custom_socket = format!("@/tmp/test-mpath-mockd-cycle-{}", std::process::id());
    let mut daemon = Command::new(daemon_path())
        .arg("--socket")
        .arg(&custom_socket)
        .arg("--test-data-dir")
        .arg(test_data_dir())
        .arg("--file-map")
        .arg("show maps json=show_maps_json/all_active_running.json,show_maps_json/failed_all_timeout.json")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start daemon with cycling");

    // Wait for daemon to start
    let start = Instant::now();
    let mut ready = false;
    while start.elapsed() < Duration::from_secs(2) {
        if daemon.try_wait().is_ok_and(|o| o.is_some()) {
            panic!("Daemon exited");
        }

        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&custom_socket)
            .arg("-c")
            .arg("show maps json")
            .arg("-o")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Ok(status) = result {
            if status.success() {
                ready = true;
                break;
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(ready, "Daemon with cycling should be ready");

    // Make multiple queries and verify they cycle through the files
    let results: Vec<String> = (0..4).map(|_| {
        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&custom_socket)
            .arg("-c")
            .arg("show maps json")
            .output()
            .expect("Failed to run query");

        assert!(result.status.success(), "Query should succeed");
        String::from_utf8_lossy(&result.stdout).into_owned()
    }).collect();

    daemon.kill().ok();
    daemon.wait().ok();

    // Verify we got both types of responses (cycling between the two files)
    // all_active_running.json has "paths" : 16
    // failed_all_timeout.json has "paths" : 0
    let active_count = results.iter().filter(|s| s.contains("\"paths\" : 16")).count();
    let failed_count = results.iter().filter(|s| s.contains("\"paths\" : 0")).count();

    // With 4 queries and 2 files, we should get 2 of each (round-robin)
    assert_eq!(active_count, 2, "Should get 2 active responses in 4 queries");
    assert_eq!(failed_count, 2, "Should get 2 failed responses in 4 queries");
}

#[test]
fn test_daemon_auto_loads_multiple_files() {
    // Without specifying --file-map, the daemon should auto-load all files from the subdirectory
    let (mut daemon, socket_path) = start_test_daemon("test_daemon_auto_load");

    if wait_for_daemon(&mut daemon, &socket_path, Duration::from_secs(2)).is_err() {
        daemon.kill().ok();
        panic!("Daemon did not start in time");
    }

    // Make multiple queries to show maps json
    let results: Vec<String> = (0..4).map(|_| {
        let result = Command::new(query_path())
            .arg("--socket")
            .arg(&socket_path)
            .arg("-c")
            .arg("show maps json")
            .output()
            .expect("Failed to run query");

        assert!(result.status.success(), "Query should succeed");
        String::from_utf8_lossy(&result.stdout).into_owned()
    }).collect();

    daemon.kill().ok();
    daemon.wait().ok();

    // With 2 files in show_maps_json/ (all_active_running.json and failed_all_timeout.json)
    // we should cycle through both
    let active_count = results.iter().filter(|s| s.contains("\"paths\" : 16")).count();
    let failed_count = results.iter().filter(|s| s.contains("\"paths\" : 0")).count();

    // With 4 queries and 2 files, we should get 2 of each
    assert!(active_count >= 1 && failed_count >= 1,
        "Should cycle through multiple files, got {} active and {} failed", active_count, failed_count);
}
