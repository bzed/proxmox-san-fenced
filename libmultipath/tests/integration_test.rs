//! Integration tests for libmultipath.
//!
//! These tests verify communication with a mock multipathd daemon.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.

use std::env;
use std::io::{Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
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

fn run_custom_mock_server<F>(test_name: &str, mut handler: F) -> String
where
    F: FnMut(UnixStream) + Send + 'static,
{
    let socket_name = format!("/tmp/test-custom-mock-{}-{}", test_name, std::process::id());
    let socket_name_clone = socket_name.clone();

    std::thread::spawn(move || {
        let addr = SocketAddr::from_abstract_name(socket_name_clone.as_bytes()).unwrap();
        let listener = UnixListener::bind_addr(&addr).unwrap();
        if let Ok((stream, _)) = listener.accept() {
            handler(stream);
        }
    });

    socket_name
}

fn read_command(mut stream: &UnixStream) -> String {
    let mut len_bytes = [0u8; 8];
    if stream.read_exact(&mut len_bytes).is_err() {
        return String::new();
    }
    let cmd_len = u64::from_le_bytes(len_bytes) as usize;
    let mut cmd_bytes = vec![0u8; cmd_len];
    if stream.read_exact(&mut cmd_bytes).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&cmd_bytes).into_owned()
}

#[test]
fn test_mock_server_infinite_length() {
    let socket_path = run_custom_mock_server("infinite_len", |mut stream| {
        let _cmd = read_command(&stream);
        // Send a length that exceeds MAX_REPLY_LEN (32 MB)
        let len = (libmultipath::MAX_REPLY_LEN + 1) as u64;
        let len_bytes = len.to_le_bytes();
        stream.write_all(&len_bytes).ok();
    });

    std::thread::sleep(Duration::from_millis(50));

    let result = libmultipath::send_multipath_command_to_socket(&socket_path, "show maps json");
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("Invalid reply length"));
}

#[test]
fn test_mock_server_infinite_stream() {
    let socket_path = run_custom_mock_server("infinite_stream", |mut stream| {
        let _cmd = read_command(&stream);
        // Claim the response is 10 bytes
        let len = 10u64;
        let len_bytes = len.to_le_bytes();
        stream.write_all(&len_bytes).ok();
        // Write 1 MB of 'A's (much more than 10 bytes)
        let data = vec![b'A'; 1024 * 1024];
        stream.write_all(&data).ok();
    });

    std::thread::sleep(Duration::from_millis(50));

    let result = libmultipath::send_multipath_command_to_socket(&socket_path, "show maps json");
    assert!(result.is_ok());
    let reply = result.unwrap();
    // It should have read exactly 10 bytes, excluding the null byte
    assert_eq!(reply, "AAAAAAAAA");
}

#[test]
fn test_mock_server_binary_garbage_invalid_utf8() {
    let socket_path = run_custom_mock_server("binary_garbage_utf8", |mut stream| {
        let _cmd = read_command(&stream);
        let len = 4u64;
        let len_bytes = len.to_le_bytes();
        stream.write_all(&len_bytes).ok();
        // Send invalid UTF-8 bytes (null-terminated at the end so it truncates up to null, but first 3 bytes are invalid UTF-8)
        let data = [0xFFu8, 0xFEu8, 0xFDu8, 0x00u8];
        stream.write_all(&data).ok();
    });

    std::thread::sleep(Duration::from_millis(50));

    let result = libmultipath::send_multipath_command_to_socket(&socket_path, "show maps json");
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("Invalid UTF-8"));
}
