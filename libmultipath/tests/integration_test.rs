//! Integration tests for libmultipath.
//!
//! These tests verify communication with a mock multipathd daemon.

use std::env;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Helper to get the workspace root directory
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Get the path to the mpath-mockd binary
fn mockd_path() -> PathBuf {
    workspace_root().join("target/debug/mpath-mockd")
}

/// Get the path to the test data directory
fn test_data_dir() -> PathBuf {
    workspace_root().join("test-data/multipathd")
}

/// Starts the mock daemon with a unique socket for each test
fn start_mock_daemon(test_name: &str) -> (Child, String) {
    let socket_name = format!(
        "@/tmp/test-libmultipath-{}-{}",
        test_name,
        std::process::id()
    );
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
fn wait_for_daemon(socket_path: &str, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if libmultipath::MultipathConnection::with_socket(socket_path).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(/*ms*/ 100));
    }
    Err("Timeout waiting for daemon".to_string())
}

#[test]
fn test_send_command_success() {
    let (mut daemon, socket_path) = start_mock_daemon("test_success");

    if wait_for_daemon(&socket_path, Duration::from_secs(/*secs*/ 2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let conn = libmultipath::MultipathConnection::with_socket(&socket_path);
    assert!(conn.is_ok(), "Failed to connect: {:?}", conn.err());
    let conn = conn.unwrap();

    let reply = conn.send_command("show maps json", /*timeout_ms*/ None);
    assert!(reply.is_ok(), "Failed to send command: {:?}", reply.err());
    let reply = reply.unwrap();

    assert!(
        reply.contains("maps"),
        "Reply did not contain maps: {}",
        reply
    );

    daemon.kill().ok();
    daemon.wait().ok();
}

#[test]
fn test_send_command_to_socket() {
    let (mut daemon, socket_path) = start_mock_daemon("test_to_socket");

    if wait_for_daemon(&socket_path, Duration::from_secs(/*secs*/ 2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    let reply = libmultipath::send_multipath_command_to_socket(&socket_path, "show maps json");
    assert!(
        reply.is_ok(),
        "Failed to send command to socket: {:?}",
        reply.err()
    );
    let reply = reply.unwrap();
    assert!(reply.contains("maps"));

    daemon.kill().ok();
    daemon.wait().ok();
}

#[test]
fn test_timeout_behavior() {
    let (mut daemon, socket_path) = start_mock_daemon("test_timeout");

    if wait_for_daemon(&socket_path, Duration::from_secs(/*secs*/ 2)).is_err() {
        daemon.kill().ok();
        panic!("Mock daemon did not start in time");
    }

    // A command with a very short timeout of 1ms should either time out or succeed if the response was instant
    let result = libmultipath::send_multipath_command_to_socket_with_timeout(
        &socket_path,
        "show maps json",
        /*timeout_ms*/ 1,
    );

    // If it did timeout, verify it returned TimedOut
    if let Err(e) = result {
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
    }

    daemon.kill().ok();
    daemon.wait().ok();
}
