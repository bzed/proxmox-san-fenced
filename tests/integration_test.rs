//! Workspace-level integration tests for pve-san-fenced daemon
//!
//! These tests run the pve-san-fenced daemon binary against the mpath-mockd
//! and pvesh-mock tools.

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

    let child = Command::new(mockd_bin)
        .arg("--socket")
        .arg(&ctx.socket_path)
        .arg("--test-data-dir")
        .arg(test_data_dir)
        .arg("--file-map")
        .arg(file_map)
        .arg("--verbose") // Enable verbose logging in the mock daemon
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start mpath-mockd");

    ctx.mock_daemon = Some(child);

    // Give the mock daemon a brief moment to bind to the socket
    std::thread::sleep(Duration::from_millis(200));
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
}
