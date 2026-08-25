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

/// Helper to dynamically find workspace binaries whether running via `cargo test` (target/debug)
/// or `cargo llvm-cov` (target/llvm-cov-target/debug)
fn get_bin_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("Failed to get current executable path");
    path.pop(); // remove test binary name
    if path.ends_with("deps") {
        path.pop();
    }
    path.join(name)
}


/// Helper to get the workspace root directory
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Get the path to the mpath-mockd binary
fn mockd_path() -> PathBuf {
    get_bin_path("mpath-mockd")
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
    let pid = std::process::id();
    let socket_name = format!("@/tmp/test-custom-mock-{test_name}-{pid}");
    let socket_name_clone = socket_name.clone();

    std::thread::spawn(move || {
        let listener = if let Some(abstract_name) = socket_name_clone.strip_prefix('@') {
            let addr = SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap();
            UnixListener::bind_addr(&addr).unwrap()
        } else {
            let _ = std::fs::remove_file(&socket_name_clone);
            UnixListener::bind(&socket_name_clone).unwrap()
        };
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

#[test]
fn test_filesystem_socket_communication() {
    let pid = std::process::id();
    let socket_path = format!("/tmp/test-libmultipath-fs-{pid}");
    let socket_path_clone = socket_path.clone();

    std::thread::spawn(move || {
        let _ = std::fs::remove_file(&socket_path_clone);
        let listener = UnixListener::bind(&socket_path_clone).unwrap();
        if let Ok((stream, _)) = listener.accept() {
            let mut s = stream;
            let cmd = read_command(&s);
            assert_eq!(cmd.trim_end_matches('\0'), "show maps json");

            let reply = "{\"maps\": []}";
            let reply_bytes = reply.as_bytes();
            let reply_len = (reply_bytes.len() + 1) as u64;
            s.write_all(&reply_len.to_le_bytes()).unwrap();
            s.write_all(reply_bytes).unwrap();
            s.write_all(&[0u8]).unwrap();
        }
    });

    std::thread::sleep(Duration::from_millis(50));

    let conn = libmultipath::MultipathConnection::with_socket(&socket_path);
    assert!(
        conn.is_ok(),
        "Failed to connect to filesystem socket: {:?}",
        conn.err()
    );
    let conn = conn.unwrap();

    let reply = conn.send_command("show maps json", None);
    assert!(reply.is_ok(), "Failed to send command: {:?}", reply.err());
    let reply = reply.unwrap();
    assert_eq!(reply, "{\"maps\": []}");

    let _ = std::fs::remove_file(&socket_path);
}

#[test]
fn test_abstract_socket_communication() {
    let pid = std::process::id();
    let socket_path = format!("@/tmp/test-libmultipath-abs-{pid}");
    let socket_path_clone = socket_path.clone();

    std::thread::spawn(move || {
        let abstract_name = socket_path_clone.strip_prefix('@').unwrap();
        let addr = SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap();
        let listener = UnixListener::bind_addr(&addr).unwrap();
        if let Ok((stream, _)) = listener.accept() {
            let mut s = stream;
            let cmd = read_command(&s);
            assert_eq!(cmd.trim_end_matches('\0'), "show maps json");

            let reply = "{\"maps\": []}";
            let reply_bytes = reply.as_bytes();
            let reply_len = (reply_bytes.len() + 1) as u64;
            s.write_all(&reply_len.to_le_bytes()).unwrap();
            s.write_all(reply_bytes).unwrap();
            s.write_all(&[0u8]).unwrap();
        }
    });

    std::thread::sleep(Duration::from_millis(50));

    let conn = libmultipath::MultipathConnection::with_socket(&socket_path);
    assert!(
        conn.is_ok(),
        "Failed to connect to abstract socket: {:?}",
        conn.err()
    );
    let conn = conn.unwrap();

    let reply = conn.send_command("show maps json", None);
    assert!(reply.is_ok(), "Failed to send command: {:?}", reply.err());
    let reply = reply.unwrap();
    assert_eq!(reply, "{\"maps\": []}");
}

#[test]
fn test_mock_server_hanging_socket() {
    let pid = std::process::id();
    let socket_path = format!("@/tmp/test-libmultipath-hang-{pid}");
    let socket_path_clone = socket_path.clone();

    std::thread::spawn(move || {
        let abstract_name = socket_path_clone.strip_prefix('@').unwrap();
        let addr = SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap();
        let listener = UnixListener::bind_addr(&addr).unwrap();
        if let Ok((stream, _)) = listener.accept() {
            let mut s = stream;
            let _cmd = read_command(&s);
            let len = 100u64;
            s.write_all(&len.to_le_bytes()).unwrap();
            std::thread::sleep(Duration::from_secs(3600));
        }
    });

    std::thread::sleep(Duration::from_millis(50));

    let result = libmultipath::send_multipath_command_to_socket_with_timeout(
        &socket_path,
        "show maps json",
        /*timeout_ms*/ 50,
    );

    assert!(result.is_err());
    let err = result.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn test_send_command_on_fd_invalid() {
    let result = libmultipath::MultipathConnection::send_command_on_fd(-1, "test", None);
    assert!(result.is_err());
    assert_eq!(result.err().unwrap().kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn test_send_command_no_reply() {
    let socket_path = run_custom_mock_server("no_reply", |stream| {
        let cmd = read_command(&stream);
        assert_eq!(cmd.trim_end_matches('\0'), "just do it");
        // Don't write a reply, mimicking fire-and-forget.
    });
    std::thread::sleep(Duration::from_millis(50));
    let conn = libmultipath::MultipathConnection::with_socket(&socket_path).unwrap();
    let res = conn.send_command_no_reply("just do it");
    assert!(res.is_ok());
}

#[test]
fn test_unexpected_eof_on_length() {
    let socket_path = run_custom_mock_server("eof_len", |stream| {
        let _cmd = read_command(&stream);
        // Abruptly close connection instead of sending length
        drop(stream);
    });
    std::thread::sleep(Duration::from_millis(50));
    let res = libmultipath::send_multipath_command_to_socket(&socket_path, "show maps json");
    assert!(res.is_err());
    assert_eq!(res.err().unwrap().kind(), std::io::ErrorKind::ConnectionReset);
}

#[test]
fn test_unexpected_eof_on_data() {
    let socket_path = run_custom_mock_server("eof_data", |mut stream| {
        let _cmd = read_command(&stream);
        let len = 100u64;
        stream.write_all(&len.to_le_bytes()).unwrap();
        // Send fewer bytes than promised, then close
        stream.write_all(b"1234567890").unwrap();
        drop(stream);
    });
    std::thread::sleep(Duration::from_millis(50));
    let res = libmultipath::send_multipath_command_to_socket(&socket_path, "show maps json");
    assert!(res.is_err());
    assert_eq!(res.err().unwrap().kind(), std::io::ErrorKind::ConnectionReset);
}

#[test]
fn test_command_with_null_byte() {
    let socket_path = run_custom_mock_server("null_byte", |_stream| {});
    std::thread::sleep(Duration::from_millis(50));
    let res = libmultipath::send_multipath_command_to_socket(&socket_path, "show\0maps");
    assert!(res.is_err());
    assert_eq!(res.err().unwrap().kind(), std::io::ErrorKind::InvalidInput);
}



#[test]
fn test_send_command_on_fd_valid() {
    use std::os::fd::AsRawFd;
    let socket_path = run_custom_mock_server("valid_fd", |mut stream| {
        let cmd = read_command(&stream);
        assert_eq!(cmd.trim_end_matches('\0'), "test fd");
        let reply = "ok";
        let reply_len = (reply.len() + 1) as u64;
        stream.write_all(&reply_len.to_le_bytes()).unwrap();
        stream.write_all(reply.as_bytes()).unwrap();
        stream.write_all(&[0u8]).unwrap();
    });
    std::thread::sleep(Duration::from_millis(50));
    
    // Connect manually
    let abstract_name = socket_path.strip_prefix('@').unwrap();
    let addr = SocketAddr::from_abstract_name(abstract_name.as_bytes()).unwrap();
    let stream = UnixStream::connect_addr(&addr).unwrap();
    
    let fd = stream.as_raw_fd();
    let res = libmultipath::MultipathConnection::send_command_on_fd(fd, "test fd", None);
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "ok");
}

#[test]
fn test_zero_length_reply() {
    let socket_path = run_custom_mock_server("zero_len", |mut stream| {
        let _cmd = read_command(&stream);
        let len = 0u64;
        stream.write_all(&len.to_le_bytes()).unwrap();
    });
    std::thread::sleep(Duration::from_millis(50));
    let res = libmultipath::send_multipath_command_to_socket(&socket_path, "show maps json");
    assert!(res.is_err());
    let err = res.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("Invalid reply length"));
}

#[test]
fn test_invalid_abstract_socket_address() {
    // 110 characters, exceeding the 108 byte limit for abstract sockets
    let socket_path = format!("@{}", "a".repeat(110));
    let res = libmultipath::MultipathConnection::with_socket(&socket_path);
    assert!(res.is_err());
    let err = res.err().unwrap();
    assert_eq!(err.to_string(), "Invalid socket address");
}

#[test]
fn test_mock_server_hanging_socket_length() {
    let socket_path = run_custom_mock_server("hang_len", |stream| {
        let _cmd = read_command(&stream);
        // Do not write anything, just hang
        std::thread::sleep(Duration::from_secs(3600));
    });

    std::thread::sleep(Duration::from_millis(50));

    let result = libmultipath::send_multipath_command_to_socket_with_timeout(
        &socket_path,
        "show maps json",
        /*timeout_ms*/ 50,
    );

    assert!(result.is_err());
    let err = result.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(err.to_string().contains("Timeout waiting for reply"));
}
