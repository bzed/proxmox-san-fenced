//! Workspace-level integration tests for pve-san-fenced daemon
//!
//! These tests run the pve-san-fenced daemon binary against the mpath-mockd
//! and pvesh-mock tools.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct TestContext {
    mock_daemon: Option<Child>,
    target_daemon: Option<Child>,
    temp_dir: PathBuf,
    socket_path: String,
}

impl TestContext {
    fn new(test_name: &str, node_name: &str) -> Self {
        // Create a unique temporary directory for this test case
        let temp_dir = env::temp_dir().join(format!(
            "pve-san-fenced-test-{}-{}",
            test_name,
            std::process::id()
        ));
        fs::create_dir_all(&temp_dir).unwrap();

        // Create the mock nodes directory required for validation
        let nodes_dir = temp_dir.join("nodes");
        fs::create_dir_all(nodes_dir.join(node_name)).unwrap();

        // Use a unique Unix domain socket path in the abstract namespace
        let socket_path = format!(
            "@/tmp/test-pve-san-fenced-{}-{}",
            test_name,
            std::process::id()
        );

        Self {
            mock_daemon: None,
            target_daemon: None,
            temp_dir,
            socket_path,
        }
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        if let Some(mut child) = self.target_daemon.take() {
            child.kill().ok();
            child.wait().ok();
        }
        if let Some(mut child) = self.mock_daemon.take() {
            child.kill().ok();
            child.wait().ok();
        }
        fs::remove_dir_all(&self.temp_dir).ok();
    }
}

/// Helper to start the mock daemon
fn start_mockd(ctx: &mut TestContext, file_map: &str) {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mockd_bin = workspace.join("target/debug/mpath-mockd");
    let test_data_dir = workspace.join("test-data/multipathd/show_maps_json");

    // Write a mock config file to temp_dir that passes validation with no warnings
    let mock_config_path = ctx.temp_dir.join("mock_show_config.txt");
    let mock_config_content = r#"
defaults {
    polling_interval 5
    no_path_retry "queue"
    fast_io_fail_tmo 5
    dev_loss_tmo "infinity"
}
"#;
    fs::write(&mock_config_path, mock_config_content).unwrap();

    let child = Command::new(mockd_bin)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--test-data-dir")
        .arg(test_data_dir)
        .arg("--file-map")
        .arg(file_map)
        .arg("--file-map")
        .arg(format!("show config={}", mock_config_path.to_str().unwrap()))
        .arg("--verbose") // Enable verbose logging in the mock daemon
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start mpath-mockd");

    ctx.mock_daemon = Some(child);

    // Give the mock daemon a brief moment to bind to the socket
    std::thread::sleep(Duration::from_millis(200));
}

/// Helper to poll and assert the content of the status file via daemon CLI status-query
fn assert_status_file(status_file_path: &std::path::Path, expected_prefix: &str) {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fencer_bin = workspace.join("target/debug/pve-san-fenced");

    let expected_code = match expected_prefix {
        "OK" => 0,
        "WARNING" => 1,
        "CRITICAL" => 2,
        _ => 3,
    };

    let start = std::time::Instant::now();
    let mut last_code = None;
    let mut last_stdout = String::new();

    while start.elapsed() < Duration::from_secs(3) {
        let output = Command::new(&fencer_bin)
            .arg("--status")
            .arg("--status-file")
            .arg(status_file_path)
            .output()
            .expect("Failed to run fencer in status-query mode");

        let code = output.status.code();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

        last_code = code;
        last_stdout = stdout.clone();

        if code == Some(expected_code) && stdout.starts_with(expected_prefix) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "Expected status-query to return exit code {:?}, starts_with '{}', but got code {:?}, stdout: '{}'",
        expected_code, expected_prefix, last_code, last_stdout
    );
}

/// Helper to start the pve-san-fenced daemon
fn start_fencer(ctx: &mut TestContext, node_name: &str, extra_args: &[&str]) {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fencer_bin = workspace.join("target/debug/pve-san-fenced");
    let pvesh_mock_bin = workspace.join("target/debug/pvesh-mock");
    let test_data_dir = workspace.join("test-data/pvesh");

    let nodes_dir = ctx.temp_dir.join("nodes");

    let mut cmd = Command::new(fencer_bin);
    cmd.arg("--node-name")
        .arg(node_name)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--pvesh-command")
        .arg(pvesh_mock_bin)
        .arg("--poll-interval")
        .arg("1")
        .arg("--discovery-interval")
        .arg("10")
        .arg("--max-failures")
        .arg("3");

    // Always ensure we have a status file inside temp_dir if not overridden
    let has_status_file = extra_args.iter().any(|arg| arg.starts_with("--status-file"));
    let status_path = ctx.temp_dir.join("pve-san-fenced.status");
    if !has_status_file {
        cmd.arg("--status-file").arg(&status_path);
    }

    for arg in extra_args {
        cmd.arg(arg);
    }

    cmd.env("PVE_SAN_TEST_DATA_DIR", test_data_dir)
        .env("PVE_SAN_SYS_NODES_DIR", nodes_dir)
        .env("PVE_SAN_FENCE_DRY_RUN", "1")
        .env("RUST_LOG", "debug") // Set log level to debug for the fencer under test!
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().expect("Failed to start pve-san-fenced");
    ctx.target_daemon = Some(child);
}

#[test]
fn test_integration_stable_healthy_paths() {
    let mut ctx = TestContext::new("stable_healthy", "pve001");

    // Start mock daemon with maps always reporting healthy paths
    start_mockd(&mut ctx, "show maps json=all_active_running.json");

    // Start fencer daemon
    start_fencer(&mut ctx, "pve001", &[]);

    // Run for 4 seconds to allow multiple check intervals
    std::thread::sleep(Duration::from_secs(4));

    // Verify it is still running
    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");
    if fencer.try_wait().unwrap().is_some() {
        let output = fencer.wait_with_output().unwrap();
        panic!(
            "Fencer exited prematurely! Logs:\nSTDOUT:\n{}\nSTDERR:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Terminate fencer and read output
    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let full_logs = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("=== LOGS ===\n{full_logs}\n=============");

    // Assert that no fencing trigger was attempted
    assert!(full_logs.contains("Starting PVE SAN fencing daemon on node: pve001"));
    assert!(!full_logs.contains("Consecutive storage failure"));
    assert!(!full_logs.contains("SAN FENCER: Total persistent storage loss detected"));

    // Verify status file is OK
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "OK");
}

#[test]
fn test_integration_sustained_failure_fencing() {
    let mut ctx = TestContext::new("sustained_failure", "pve001");

    // Start mock daemon with maps always reporting failing paths
    start_mockd(&mut ctx, "show maps json=failed_all_timeout.json");

    // Start fencer daemon
    start_fencer(&mut ctx, "pve001", &[]);

    // The daemon should fence after 3 failures (max-failures 3, poll-interval 1s)
    let start = std::time::Instant::now();
    let mut exit_status = None;
    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");

    while start.elapsed() < Duration::from_secs(10) {
        if let Some(status) = fencer.try_wait().unwrap() {
            exit_status = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if exit_status.is_none() {
        fencer.kill().ok();
        let output = fencer.wait_with_output().unwrap();
        let mock_out = if let Some(mut mock) = ctx.mock_daemon.take() {
            mock.kill().ok();
            let out = mock.wait_with_output().unwrap();
            format!(
                "STDOUT:\n{}\nSTDERR:\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        } else {
            "No mock daemon".to_string();
            "".to_string()
        };
        panic!(
            "Fencer failed to exit within 10 seconds under sustained failure!\nFencer Logs:\nSTDOUT:\n{}\nSTDERR:\n{}\nMock Daemon Logs:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            mock_out
        );
    }

    let status = exit_status.unwrap();
    assert!(
        status.success(),
        "Fencer daemon exited with failure code instead of clean dry-run exit: {:?}",
        status
    );

    let output = fencer.wait_with_output().unwrap();
    let full_logs = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("=== LOGS ===\n{full_logs}\n=============");

    // Verify correct progression of failures and final dry-run exit
    assert!(full_logs.contains("Consecutive storage failure: 1/3"));
    assert!(full_logs.contains("Consecutive storage failure: 2/3"));
    assert!(full_logs.contains("Consecutive storage failure: 3/3"));
    assert!(full_logs.contains("SAN FENCER: Total persistent storage loss detected"));
    assert!(full_logs.contains("SAN FENCER: DRY RUN: Fencing triggered. Exiting daemon."));

    // Verify status file is CRITICAL
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "CRITICAL");
}

#[test]
fn test_integration_transient_failure_recovery() {
    let mut ctx = TestContext::new("transient_failure", "pve001");

    // Cycle through maps mapping healthy -> failed -> healthy -> failed...
    start_mockd(
        &mut ctx,
        "show maps json=all_active_running.json,failed_all_timeout.json,all_active_running.json",
    );

    // Start fencer daemon
    start_fencer(&mut ctx, "pve001", &[]);

    // Sleep for 6 seconds to witness multiple cycles
    std::thread::sleep(Duration::from_secs(6));

    // Verify it is still running
    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");
    if fencer.try_wait().unwrap().is_some() {
        let output = fencer.wait_with_output().unwrap();
        panic!(
            "Fencer exited under transient failures! Logs:\nSTDOUT:\n{}\nSTDERR:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Terminate fencer and read output
    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let full_logs = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("=== LOGS ===\n{full_logs}\n=============");

    // Assert failures were tracked but reset upon recovery
    assert!(full_logs.contains("Consecutive storage failure: 1/3"));
    assert!(full_logs.contains("Storage connectivity restored. Resetting failure counter."));
    assert!(!full_logs.contains("Consecutive storage failure: 2/3"));
    assert!(!full_logs.contains("Consecutive storage failure: 3/3"));
    assert!(!full_logs.contains("SAN FENCER: Total persistent storage loss detected"));

    // Verify status file is OK
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "OK");
}

#[test]
fn test_integration_ghost_paths_fencing() {
    let mut ctx = TestContext::new("ghost_paths_fencing", "pve001");

    // Start mock daemon with maps always reporting ghost paths
    start_mockd(&mut ctx, "show maps json=failed_ghost_only.json");

    // Start fencer daemon
    start_fencer(&mut ctx, "pve001", &[]);

    // The daemon should fence after 3 failures (max-failures 3, poll-interval 1s)
    let start = std::time::Instant::now();
    let mut exit_status = None;
    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");

    while start.elapsed() < Duration::from_secs(10) {
        if let Some(status) = fencer.try_wait().unwrap() {
            exit_status = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if exit_status.is_none() {
        fencer.kill().ok();
        let output = fencer.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "Fencer failed to exit within 10 seconds under sustained ghost path failure!\nFencer Logs:\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
        );
    }

    let status = exit_status.unwrap();
    assert!(
        status.success(),
        "Fencer daemon exited with failure code instead of clean dry-run exit: {status:?}"
    );

    let output = fencer.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let full_logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");

    assert!(full_logs.contains("Consecutive storage failure: 1/3"));
    assert!(full_logs.contains("Consecutive storage failure: 2/3"));
    assert!(full_logs.contains("Consecutive storage failure: 3/3"));
    assert!(full_logs.contains("SAN FENCER: Total persistent storage loss detected"));
    assert!(full_logs.contains("SAN FENCER: DRY RUN: Fencing triggered. Exiting daemon."));

    // Verify status file is CRITICAL
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "CRITICAL");
}

#[test]
fn test_integration_some_undef_some_active_stable() {
    let mut ctx = TestContext::new("some_undef_some_active", "pve001");

    // Start mock daemon with some undef paths (which are treated as alive)
    start_mockd(&mut ctx, "show maps json=some_undef_some_active.json");

    // Start fencer daemon
    start_fencer(&mut ctx, "pve001", &[]);

    // Run for 4 seconds to verify fencer remains stable and does not fence
    std::thread::sleep(Duration::from_secs(4));

    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");
    if fencer.try_wait().unwrap().is_some() {
        let output = fencer.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "Fencer exited prematurely under stable undef/active mix! Logs:\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
        );
    }

    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let full_logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");
    assert!(!full_logs.contains("Consecutive storage failure"));
    assert!(!full_logs.contains("SAN FENCER: Total persistent storage loss detected"));

    // Verify status file is OK
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "OK");
}

#[test]
fn test_integration_disabled_pg_active_path_stable() {
    let mut ctx = TestContext::new("disabled_pg_active_path", "pve001");

    // Start mock daemon with a disabled pg but containing active paths
    start_mockd(&mut ctx, "show maps json=disabled_pg_active_path.json");

    // Start fencer daemon
    start_fencer(&mut ctx, "pve001", &[]);

    // Run for 4 seconds to verify fencer remains stable and does not fence
    std::thread::sleep(Duration::from_secs(4));

    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");
    if fencer.try_wait().unwrap().is_some() {
        let output = fencer.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "Fencer exited prematurely under disabled pg with active paths! Logs:\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
        );
    }

    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let full_logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");
    assert!(!full_logs.contains("Consecutive storage failure"));
    assert!(!full_logs.contains("SAN FENCER: Total persistent storage loss detected"));

    // Verify status file is OK
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "OK");
}

#[test]
fn test_integration_invalid_sysrq_chars() {
    let mut ctx = TestContext::new("invalid_sysrq_chars", "pve001");

    // Start mock daemon to avoid query connection failure logging
    start_mockd(&mut ctx, "show maps json=all_active_running.json");

    // Start fencer daemon with an invalid sysrq-char 'x'
    start_fencer(&mut ctx, "pve001", &["--sysrq-char", "s,b,x"]);

    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");

    let start = std::time::Instant::now();
    let mut exit_status = None;
    while start.elapsed() < Duration::from_secs(5) {
        if let Some(status) = fencer.try_wait().unwrap() {
            exit_status = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let status = exit_status.expect("Fencer did not exit after invalid configuration");
    assert_eq!(
        status.code(),
        Some(1),
        "Expected exit status 1 for invalid configuration, got: {status:?}"
    );

    let output = fencer.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");

    assert!(
        full_logs.contains(
            "Configuration error: Invalid SysRq character 'x' specified in configuration"
        ),
        "Logs did not contain expected error: {full_logs}"
    );

    // Verify status file is CRITICAL due to startup configuration failure
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "CRITICAL");
}

#[test]
fn test_integration_debug_log_mode() {
    let mut ctx = TestContext::new("debug_log_mode", "pve001");

    // Start mock daemon with maps always reporting healthy paths
    start_mockd(&mut ctx, "show maps json=all_active_running.json");

    // Start fencer daemon with debug log mode enabled using env variable PVE_SAN_DEBUG=true
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fencer_bin = workspace.join("target/debug/pve-san-fenced");
    let pvesh_mock_bin = workspace.join("target/debug/pvesh-mock");
    let test_data_dir = workspace.join("test-data/pvesh");
    let nodes_dir = ctx.temp_dir.join("nodes");

    let status_path = ctx.temp_dir.join("pve-san-fenced.status");
    let mut cmd = Command::new(fencer_bin);
    cmd.arg("--node-name")
        .arg("pve001")
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--pvesh-command")
        .arg(pvesh_mock_bin)
        .arg("--poll-interval")
        .arg("1")
        .arg("--discovery-interval")
        .arg("1")
        .arg("--max-failures")
        .arg("3")
        .arg("--status-file")
        .arg(&status_path)
        .env("PVE_SAN_TEST_DATA_DIR", test_data_dir)
        .env("PVE_SAN_SYS_NODES_DIR", nodes_dir)
        .env("PVE_SAN_FENCE_DRY_RUN", "1")
        .env("PVE_SAN_DEBUG", "true")
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd
        .spawn()
        .expect("Failed to start pve-san-fenced with PVE_SAN_DEBUG");
    ctx.target_daemon = Some(child);

    // Wait for the discovery run to happen and log
    std::thread::sleep(Duration::from_secs(2));

    // Stop and get output
    let mut fencer = ctx.target_daemon.take().unwrap();
    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");

    assert!(
        logs.contains("Discovered VM:") && logs.contains("state:"),
        "Logs did not contain the debug discovery output:\n{logs}"
    );

    // Verify status file is OK
    assert_status_file(&status_path, "OK");
}

#[test]
fn test_integration_hanging_multipathd() {
    let mut ctx = TestContext::new("hanging_multipathd", "pve001");

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mockd_bin = workspace.join("target/debug/mpath-mockd");
    let test_data_dir = workspace.join("test-data/multipathd/show_maps_json");

    let child = Command::new(mockd_bin)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--test-data-dir")
        .arg(test_data_dir)
        .arg("--hang")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start mpath-mockd in hang mode");

    ctx.mock_daemon = Some(child);

    std::thread::sleep(Duration::from_millis(200));

    let fencer_bin = workspace.join("target/debug/pve-san-fenced");
    let pvesh_mock_bin = workspace.join("target/debug/pvesh-mock");
    let pvesh_test_data = workspace.join("test-data/pvesh");
    let nodes_dir = ctx.temp_dir.join("nodes");

    let status_path = ctx.temp_dir.join("pve-san-fenced.status");
    let mut cmd = Command::new(fencer_bin);
    cmd.arg("--node-name")
        .arg("pve001")
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--pvesh-command")
        .arg(pvesh_mock_bin)
        .arg("--poll-interval")
        .arg("1")
        .arg("--discovery-interval")
        .arg("10")
        .arg("--max-failures")
        .arg("3")
        .arg("--status-file")
        .arg(&status_path)
        .env("PVE_SAN_TEST_DATA_DIR", pvesh_test_data)
        .env("PVE_SAN_SYS_NODES_DIR", nodes_dir)
        .env("PVE_SAN_FENCE_DRY_RUN", "1")
        .env("RUST_LOG", "debug")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().expect("Failed to start pve-san-fenced");
    ctx.target_daemon = Some(child);

    std::thread::sleep(Duration::from_secs(6));

    let mut fencer = ctx.target_daemon.take().unwrap();
    assert!(
        fencer.try_wait().unwrap().is_none(),
        "Fencer daemon exited prematurely during socket hang"
    );

    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");

    assert!(
        full_logs.contains("Failed to query multipathd"),
        "Logs did not warn about failed multipathd query during hang:\n{full_logs}"
    );
    assert!(
        full_logs.contains("Timeout waiting for reply"),
        "Logs did not contain timeout details:\n{full_logs}"
    );
    assert!(
        !full_logs.contains("TEST MODE: Fencing decision reached"),
        "Fencing should NOT have been triggered during daemon query timeouts:\n{full_logs}"
    );

    // Verify status file is WARNING due to hanging multipathd query
    assert_status_file(&status_path, "WARNING");
}

#[test]
fn test_integration_unresponsive_multipathd_connection_timeout() {
    let mut ctx = TestContext::new("unresponsive_multipathd", "pve001");

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mockd_bin = workspace.join("target/debug/mpath-mockd");

    let child = Command::new(mockd_bin)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--unresponsive")
        .arg("--verbose")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start mpath-mockd in unresponsive mode");

    ctx.mock_daemon = Some(child);

    std::thread::sleep(Duration::from_millis(200));

    let fencer_bin = workspace.join("target/debug/pve-san-fenced");
    let pvesh_mock_bin = workspace.join("target/debug/pvesh-mock");
    let pvesh_test_data = workspace.join("test-data/pvesh");
    let nodes_dir = ctx.temp_dir.join("nodes");

    let status_path = ctx.temp_dir.join("pve-san-fenced.status");
    let mut cmd = Command::new(fencer_bin);
    cmd.arg("--node-name")
        .arg("pve001")
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--pvesh-command")
        .arg(pvesh_mock_bin)
        .arg("--poll-interval")
        .arg("1")
        .arg("--discovery-interval")
        .arg("10")
        .arg("--max-failures")
        .arg("3")
        .arg("--status-file")
        .arg(&status_path)
        .env("PVE_SAN_TEST_DATA_DIR", pvesh_test_data)
        .env("PVE_SAN_SYS_NODES_DIR", nodes_dir)
        .env("PVE_SAN_FENCE_DRY_RUN", "1")
        .env("RUST_LOG", "debug")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().expect("Failed to start pve-san-fenced");
    ctx.target_daemon = Some(child);

    // Wait for at least one poll cycle to occur (1 second poll interval)
    // The connection should timeout after DEFAULT_CONNECT_TIMEOUT_MS (2000ms)
    // Also need time for discovery to run first
    std::thread::sleep(Duration::from_secs(5));

    let mut fencer = ctx.target_daemon.take().unwrap();
    // The fencer should still be running because connection timeout is handled gracefully
    assert!(
        fencer.try_wait().unwrap().is_none(),
        "Fencer daemon exited prematurely during unresponsive multipathd test"
    );

    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");

    // Verify that connection timeout errors are logged
    assert!(
        full_logs.contains("Connection") && full_logs.contains("timed out") ||
        full_logs.contains("Failed to query multipathd"),
        "Logs did not contain connection timeout error:\n{full_logs}"
    );
    
    // Verify that fencing was NOT triggered (connection errors should not cause fencing)
    assert!(
        !full_logs.contains("SAN FENCER: Total persistent storage loss detected"),
        "Fencing should NOT have been triggered during connection timeouts:\n{full_logs}"
    );

    // Verify status file is WARNING due to unresponsive multipathd connection timeout
    assert_status_file(&status_path, "WARNING");
}

#[test]
fn test_integration_partial_failure_fencing() {
    let mut ctx = TestContext::new("partial_failure_fencing", "pve001");

    // Start mock daemon with maps where mpathb is failed but mpatha is active
    start_mockd(&mut ctx, "show maps json=mpatha_active_mpathb_failed.json");

    // Start fencer daemon
    start_fencer(&mut ctx, "pve001", &[]);

    // The daemon should fence after 3 failures (max-failures 3, poll-interval 1s)
    let start = std::time::Instant::now();
    let mut exit_status = None;
    let mut fencer = ctx
        .target_daemon
        .take()
        .expect("Fencer process not tracked");

    while start.elapsed() < Duration::from_secs(10) {
        if let Some(status) = fencer.try_wait().unwrap() {
            exit_status = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if exit_status.is_none() {
        fencer.kill().ok();
        let output = fencer.wait_with_output().unwrap();
        panic!(
            "Fencer failed to exit within 10 seconds under partial multipath failure!\nFencer Logs:\nSTDOUT:\n{}\nSTDERR:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let status = exit_status.unwrap();
    assert!(
        status.success(),
        "Fencer daemon exited with failure code instead of clean dry-run exit: {:?}",
        status
    );

    let output = fencer.wait_with_output().unwrap();
    let full_logs = format!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("=== LOGS ===\n{full_logs}\n=============");

    // Verify correct progression of failures and final dry-run exit
    assert!(full_logs.contains("Consecutive storage failure: 1/3"));
    assert!(full_logs.contains("Consecutive storage failure: 2/3"));
    assert!(full_logs.contains("Consecutive storage failure: 3/3"));
    assert!(full_logs.contains("SAN FENCER: Total persistent storage loss detected"));
    assert!(full_logs.contains("SAN FENCER: DRY RUN: Fencing triggered. Exiting daemon."));

    // Verify status file is CRITICAL
    assert_status_file(&ctx.temp_dir.join("pve-san-fenced.status"), "CRITICAL");
}

    // Test that discovery backoff works correctly when discovery encounters errors
#[test]
fn test_integration_discovery_backoff() {
    let mut ctx = TestContext::new("discovery_backoff", "pve001");

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Start mpath-mockd normally (not needed for this test but for completeness)
    let mockd_bin = workspace.join("target/debug/mpath-mockd");
    let test_data_dir = workspace.join("test-data/multipathd/show_maps_json");

    let child = Command::new(&mockd_bin)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--test-data-dir")
        .arg(test_data_dir)
        .arg("--file-map")
        .arg("show maps json=all_active_running.json")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start mpath-mockd");

    ctx.mock_daemon = Some(child);
    std::thread::sleep(Duration::from_millis(200));

    // Create a fake qemu-server as a FILE (not a directory) to make LocalFiles mode fail
    // The path is constructed as PVE_SAN_TEST_DATA_DIR.parent() / "pve/local/qemu-server"
    let fake_qemu_dir = ctx.temp_dir.parent().unwrap().join("pve/local/qemu-server");
    fs::create_dir_all(fake_qemu_dir.parent().unwrap()).unwrap();
    fs::write(&fake_qemu_dir, "not a directory").unwrap();

    // Start fencer daemon with short discovery interval and backoff settings
    let fencer_bin = workspace.join("target/debug/pve-san-fenced");
    let nodes_dir = ctx.temp_dir.join("nodes");

    let status_path = ctx.temp_dir.join("pve-san-fenced.status");
    let mut cmd = Command::new(&fencer_bin);
    cmd.arg("--node-name")
        .arg("pve001")
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--pvesh-command")
        .arg("pvesh")
        .arg("--poll-interval")
        .arg("1")
        .arg("--discovery-interval")
        .arg("1")
        .arg("--max-failures")
        .arg("10")
        .arg("--discovery-max-retries")
        .arg("2")
        .arg("--discovery-backoff-base")
        .arg("1")
        .arg("--discovery-backoff-max")
        .arg("3")
        .arg("--status-file")
        .arg(&status_path)
        .env("PVE_SAN_TEST_DATA_DIR", ctx.temp_dir.clone())  // Point to temp dir with fake file
        .env("PVE_SAN_SYS_NODES_DIR", nodes_dir)
        .env("PVE_SAN_FENCE_DRY_RUN", "1")
        .env("RUST_LOG", "debug")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().expect("Failed to start pve-san-fenced");
    ctx.target_daemon = Some(child);

    // Run for enough time to trigger multiple discovery failures and backoff
    std::thread::sleep(Duration::from_secs(10));

    let mut fencer = ctx.target_daemon.take().unwrap();
    // The fencer should still be running - discovery failures should not cause fencing
    assert!(
        fencer.try_wait().unwrap().is_none(),
        "Fencer daemon exited prematurely during discovery backoff test"
    );

    fencer.kill().ok();
    let output = fencer.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let full_logs = format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}");

    println!("=== DISCOVERY BACKOFF TEST LOGS ===\n{full_logs}\n===============================");

    // Verify that discovery errors are logged
    assert!(
        full_logs.contains("Error discovering active multipath devices"),
        "Logs did not contain discovery errors:\n{full_logs}"
    );

    // Verify that backoff warnings are logged
    assert!(
        full_logs.contains("backing off for") && full_logs.contains("consecutive failures"),
        "Logs did not contain backoff warnings:\n{full_logs}"
    );

    // Verify that fencing was NOT triggered (discovery errors should not cause fencing)
    assert!(
        !full_logs.contains("SAN FENCER: Total persistent storage loss detected"),
        "Fencing should NOT have been triggered during discovery failures:\n{full_logs}"
    );

    // Verify status file is WARNING due to discovery failure backoffs
    assert_status_file(&status_path, "WARNING");
}

#[test]
fn test_integration_status_reporting_transitions() {
    let mut ctx = TestContext::new("status_reporting", "pve001");
    let status_file_path = ctx.temp_dir.join("pve-san-fenced.status");

    // Start mock daemon with healthy -> failed -> healthy maps
    start_mockd(
        &mut ctx,
        "show maps json=all_active_running.json,failed_all_timeout.json,all_active_running.json",
    );

    // Start fencer daemon with custom status-file argument
    start_fencer(
        &mut ctx,
        "pve001",
        &[
            "--status-file",
            status_file_path.to_str().unwrap(),
        ],
    );

    // Helper to poll status file
    let wait_for_status = |predicate: fn(&str) -> bool, timeout: Duration| -> String {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if let Ok(content) = fs::read_to_string(&status_file_path) {
                if predicate(&content) {
                    return content;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        fs::read_to_string(&status_file_path).unwrap_or_default()
    };

    // 1. Wait for initial OK status
    let content = wait_for_status(|c| c.starts_with("OK -"), Duration::from_secs(3));
    assert!(content.starts_with("OK -"), "Expected OK status, got: {content}");

    // 2. Wait for transition to WARNING (poll failures)
    let content = wait_for_status(|c| c.starts_with("WARNING -"), Duration::from_secs(3));
    assert!(content.starts_with("WARNING -"), "Expected WARNING status, got: {content}");
    assert!(content.contains("Consecutive storage failure"), "Expected storage failure info in: {content}");

    // 3. Wait for transition back to OK (upon recovery)
    let content = wait_for_status(|c| c.starts_with("OK -"), Duration::from_secs(5));
    assert!(content.starts_with("OK -"), "Expected recovered OK status, got: {content}");
}

#[test]
fn test_integration_status_cli_check() {
    let ctx = TestContext::new("status_cli_check", "pve001");
    let status_file_path = ctx.temp_dir.join("pve-san-fenced.status");

    // 1. Check with non-existent status file (should exit 3)
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fencer_bin = workspace.join("target/debug/pve-san-fenced");

    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer binary for status query");

    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("UNKNOWN - Failed to read status file"));

    // 2. Write OK status to file and check (should exit 0)
    fs::write(&status_file_path, "OK - Daemon is happy\n").unwrap();
    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer binary for status query");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "OK - Daemon is happy");

    // 3. Write WARNING status to file and check (should exit 1)
    fs::write(&status_file_path, "WARNING - Stale data\n").unwrap();
    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer binary for status query");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "WARNING - Stale data");

    // 4. Write CRITICAL status to file and check (should exit 2)
    fs::write(&status_file_path, "CRITICAL - Reboot failed\n").unwrap();
    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer binary for status query");

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "CRITICAL - Reboot failed");

    // 5. Write badly formatted status to file and check (should exit 3)
    fs::write(&status_file_path, "INVALID STATUS FORMAT\n").unwrap();
    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer binary for status query");

    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("UNKNOWN - Badly formatted status file"));

    // 6. Write empty status file (should exit 3)
    fs::write(&status_file_path, "").unwrap();
    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer binary for status query");

    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("UNKNOWN - Status file is empty"));

    // 7. Outdated status file (should exit 3)
    fs::write(&status_file_path, "OK - Daemon is happy\n").unwrap();
    let touch_status = Command::new("touch")
        .arg("-d")
        .arg("1 hour ago")
        .arg(&status_file_path)
        .status()
        .expect("Failed to run touch command");
    assert!(touch_status.success());

    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer binary for status query");

    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("UNKNOWN - Status file is outdated"));
}

#[test]
fn test_integration_status_warning_updates() {
    let mut ctx = TestContext::new("status_warning_updates", "pve001");
    let status_file_path = ctx.temp_dir.join("pve-san-fenced.status");

    // Write a custom mock config that triggers a dev_loss_tmo warning
    let warning_config_path = ctx.temp_dir.join("mock_warning_config.txt");
    let warning_config_content = r#"
defaults {
    polling_interval 5
    no_path_retry "queue"
    fast_io_fail_tmo 5
}
"#;
    fs::write(&warning_config_path, warning_config_content).unwrap();

    // Start mock daemon manually with the warning config mapped
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mockd_bin = workspace.join("target/debug/mpath-mockd");
    let test_data_dir = workspace.join("test-data/multipathd/show_maps_json");

    let child = Command::new(mockd_bin)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--test-data-dir")
        .arg(test_data_dir)
        .arg("--file-map")
        .arg("show maps json=all_active_running.json,failed_all_timeout.json")
        .arg("--file-map")
        .arg(format!("show config={}", warning_config_path.to_str().unwrap()))
        .arg("--verbose")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start mpath-mockd");

    ctx.mock_daemon = Some(child);
    std::thread::sleep(Duration::from_millis(200));

    // Start fencer daemon
    start_fencer(
        &mut ctx,
        "pve001",
        &[
            "--status-file",
            status_file_path.to_str().unwrap(),
        ],
    );

    // Helper to query status from CLI
    let query_status = || -> (Option<i32>, String) {
        let fencer_bin = workspace.join("target/debug/pve-san-fenced");
        let output = Command::new(&fencer_bin)
            .arg("--status")
            .arg("--status-file")
            .arg(&status_file_path)
            .output()
            .expect("Failed to run fencer in status-query mode");
        (output.status.code(), String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    // 1. Initial status check: must report the dev_loss_tmo warning
    let start = std::time::Instant::now();
    let mut initial_ok = false;
    while start.elapsed() < Duration::from_secs(3) {
        let (code, stdout) = query_status();
        if code == Some(1) && stdout.contains("dev_loss_tmo is not configured") {
            initial_ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(initial_ok, "Initial warning status not written or incorrect");

    // 2. Wait for at least 3 times the poll interval (poll interval is 1s, so 3 seconds)
    // where maps cycle to failed
    std::thread::sleep(Duration::from_secs(3));

    // The status should update to include the consecutive storage failure warning
    let (code, stdout) = query_status();
    assert_eq!(code, Some(1));
    assert!(stdout.contains("Consecutive storage failure"), "Expected consecutive failure in: {stdout}");
    assert!(stdout.contains("dev_loss_tmo is not configured"), "Expected dev_loss_tmo warning in: {stdout}");
}

#[test]
fn test_integration_status_warning_no_failures() {
    let mut ctx = TestContext::new("status_warning_no_failures", "pve001");
    let status_file_path = ctx.temp_dir.join("pve-san-fenced.status");

    // Write a custom mock config that triggers a dev_loss_tmo warning
    let warning_config_path = ctx.temp_dir.join("mock_warning_config.txt");
    let warning_config_content = r#"
defaults {
    polling_interval 5
    no_path_retry "queue"
    fast_io_fail_tmo 5
}
"#;
    fs::write(&warning_config_path, warning_config_content).unwrap();

    // Start mock daemon with healthy maps only
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mockd_bin = workspace.join("target/debug/mpath-mockd");
    let test_data_dir = workspace.join("test-data/multipathd/show_maps_json");

    let child = Command::new(mockd_bin)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--test-data-dir")
        .arg(test_data_dir)
        .arg("--file-map")
        .arg("show maps json=all_active_running.json")
        .arg("--file-map")
        .arg(format!("show config={}", warning_config_path.to_str().unwrap()))
        .arg("--verbose")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start mpath-mockd");

    ctx.mock_daemon = Some(child);
    std::thread::sleep(Duration::from_millis(200));

    // Start fencer daemon
    start_fencer(
        &mut ctx,
        "pve001",
        &[
            "--status-file",
            status_file_path.to_str().unwrap(),
        ],
    );

    // Wait for at least 3 times the poll interval (poll interval is 1s, so 3 seconds)
    std::thread::sleep(Duration::from_secs(3));

    // Query status from CLI
    let fencer_bin = workspace.join("target/debug/pve-san-fenced");
    let output = Command::new(&fencer_bin)
        .arg("--status")
        .arg("--status-file")
        .arg(&status_file_path)
        .output()
        .expect("Failed to run fencer in status-query mode");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("WARNING -"));
    assert!(stdout.contains("dev_loss_tmo is not configured"));
    assert!(!stdout.contains("Consecutive storage failure"));
}
