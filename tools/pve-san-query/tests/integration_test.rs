//! Integration tests for pve-san-query tool
//!
//! These tests use the pvesh-mock tool to simulate pvesh responses.
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
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Helper to get the workspace root directory
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the directory containing pve-san-query's Cargo.toml
    // which is /home/bzed/workspace/conova/vibe/pve-san-fenced/tools/pve-san-query
    // We need to go up two levels to get the workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Helper to get the test data directory
fn test_data_dir() -> PathBuf {
    workspace_root().join("test-data/pvesh")
}

/// Get the path to the pvesh-mock binary
fn pvesh_mock_path() -> PathBuf {
    workspace_root().join("target/debug/pvesh-mock")
}

/// Get the path to the pve-san-query binary
fn pve_san_query_path() -> PathBuf {
    workspace_root().join("target/debug/pve-san-query")
}

static RUN_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Run pve-san-query with pvesh-mock in PATH
fn run_pve_san_query(args: &[&str]) -> Vec<u8> {
    let count = RUN_COUNTER.fetch_add(1, Ordering::SeqCst);
    let unique_temp = env::temp_dir().join(format!(
        "pve-san-query-test-run-{}-{}",
        std::process::id(),
        count
    ));
    fs::create_dir_all(&unique_temp).unwrap();
    let script_path = unique_temp.join("pvesh");

    // Create a wrapper script that calls pvesh-mock
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script_content = format!("#!/bin/sh\nexec {} \"$@\"", pvesh_mock_path().display());
        fs::write(&script_path, script_content).unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        // Set PATH to include temp dir with our mock pvesh
        let path = env::var_os("PATH").unwrap();
        let new_path = format!("{}:{}", unique_temp.display(), path.to_string_lossy());
        let _guard = EnvGuard::new(&["PATH", "PVE_SAN_TEST_DATA_DIR"]);
        env::set_var("PATH", new_path);

        // Set environment variable for pvesh-mock
        env::set_var("PVE_SAN_TEST_DATA_DIR", test_data_dir());

        // Run pve-san-query
        let output = Command::new(pve_san_query_path())
            .args(args)
            .current_dir(workspace_root())
            .output()
            .expect("Failed to run pve-san-query");

        // Clean up
        fs::remove_dir_all(&unique_temp).ok();

        output.stdout
    }

    #[cfg(not(unix))]
    {
        // On non-Unix systems, we can't use a script wrapper
        // Return empty for now
        vec![]
    }
}

#[test]
fn test_pve_san_query_outputs_valid_json() {
    let output = run_pve_san_query(&["--node", "pve001"]);

    let json_output = String::from_utf8(output).expect("Output should be valid UTF-8");
    let _data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Output should be valid JSON");

    // If we got here, the JSON is valid
}

#[test]
fn test_pve_san_query_has_expected_structure() {
    let output = run_pve_san_query(&["--node", "pve001"]);

    let json_output = String::from_utf8(output).expect("Output should be valid UTF-8");
    let data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Output should be valid JSON");

    // Check that we have the expected structure
    assert!(data.get("node").is_some(), "Should have 'node' field");
    assert!(data.get("vms").is_some(), "Should have 'vms' field");

    // Check node value
    assert_eq!(data["node"], "pve001");

    // Check that vms is an array
    assert!(data["vms"].is_array(), "vms should be an array");
}

static FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn test_pve_san_query_with_output_file() {
    let count = FILE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let unique_temp = env::temp_dir().join(format!(
        "pve-san-query-file-test-{}-{}",
        std::process::id(),
        count
    ));
    fs::create_dir_all(&unique_temp).unwrap();
    let output_file = unique_temp.join("pve_san_query_test_output.json");
    let output_file_clone = output_file.clone();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let script_path = unique_temp.join("pvesh");
        let script_content = format!("#!/bin/sh\nexec {} \"$@\"", pvesh_mock_path().display());
        fs::write(&script_path, script_content).unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        // Set PATH to include temp dir with our mock pvesh
        let path = env::var_os("PATH").unwrap();
        let new_path = format!("{}:{}", unique_temp.display(), path.to_string_lossy());
        let _guard = EnvGuard::new(&["PATH", "PVE_SAN_TEST_DATA_DIR"]);
        env::set_var("PATH", new_path);

        // Set environment variable for pvesh-mock
        env::set_var("PVE_SAN_TEST_DATA_DIR", test_data_dir());

        // Run pve-san-query with output file
        let output = Command::new(pve_san_query_path())
            .args([
                "--node",
                "pve001",
                "--output",
                output_file.to_str().unwrap(),
            ])
            .current_dir(workspace_root())
            .output()
            .expect("Failed to run pve-san-query");

        // Check that the command succeeded
        assert!(output.status.success(), "pve-san-query should succeed");

        // Check that the output file was created
        assert!(output_file_clone.exists(), "Output file should exist");

        // Read and verify the output file
        let data =
            fs::read_to_string(&output_file_clone).expect("Should be able to read output file");
        let _json: serde_json::Value =
            serde_json::from_str(&data).expect("Output file should contain valid JSON");

        // Clean up the output file
        fs::remove_file(&output_file_clone).ok();

        // Clean up the unique directory
        fs::remove_dir_all(&unique_temp).ok();
    }

    #[cfg(not(unix))]
    {
        // Skip on non-Unix
        return;
    }
}

#[test]
fn test_pve_san_query_pretty_output() {
    let output = run_pve_san_query(&["--node", "pve001", "--pretty"]);

    let json_output = String::from_utf8(output).expect("Output should be valid UTF-8");

    // The output should be pretty-printed (contain newlines and indentation)
    assert!(
        json_output.contains('\n'),
        "Pretty output should contain newlines"
    );
    assert!(
        json_output.contains("  "),
        "Pretty output should contain indentation"
    );

    // Should still be valid JSON
    let _data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Pretty output should still be valid JSON");
}

#[test]
fn test_pve_san_query_mode_pvesh() {
    let output = run_pve_san_query(&["--node", "pve001", "--mode", "pvesh"]);
    let json_output = String::from_utf8(output).expect("Output should be valid UTF-8");
    let data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Output should be valid JSON");

    let vms = data["vms"].as_array().expect("vms should be an array");
    let mut vmids: Vec<u64> = vms.iter().map(|vm| vm["vmid"].as_u64().unwrap()).collect();
    vmids.sort_unstable();

    let mut expected = vec![
        132, 145, 141, 105, 144, 147, 131, 122, 114, 126, 116, 104, 133, 140, 117,
    ];
    expected.sort_unstable();

    assert_eq!(vmids, expected);
}

#[test]
fn test_pve_san_query_mode_local_files() {
    let output = run_pve_san_query(&["--node", "pve001", "--mode", "local-files"]);
    let json_output = String::from_utf8(output).expect("Output should be valid UTF-8");
    let data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Output should be valid JSON");

    let vms = data["vms"].as_array().expect("vms should be an array");
    let mut vmids: Vec<u64> = vms.iter().map(|vm| vm["vmid"].as_u64().unwrap()).collect();
    vmids.sort_unstable();

    let mut expected = vec![
        104, 105, 114, 116, 117, 122, 126, 130, 131, 132, 133, 140, 141, 144, 145, 147, 999,
    ];
    expected.sort_unstable();

    assert_eq!(vmids, expected);
}

#[test]
fn test_pve_san_query_no_node_local_files() {
    let output = run_pve_san_query(&["--mode", "local-files"]);
    let json_output = String::from_utf8(output).expect("Output should be valid UTF-8");
    let data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Output should be valid JSON");

    let vms = data["vms"].as_array().expect("vms should be an array");
    assert!(!vms.is_empty());
}

#[test]
fn test_pve_san_query_no_node_pvesh_fails() {
    let count = RUN_COUNTER.fetch_add(1, Ordering::SeqCst);
    let unique_temp = env::temp_dir().join(format!(
        "pve-san-query-test-run-{}-{}",
        std::process::id(),
        count
    ));
    fs::create_dir_all(&unique_temp).unwrap();
    let script_path = unique_temp.join("pvesh");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script_content = format!("#!/bin/sh\nexec {} \"$@\"", pvesh_mock_path().display());
        fs::write(&script_path, script_content).unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        let path = env::var_os("PATH").unwrap();
        let new_path = format!("{}:{}", unique_temp.display(), path.to_string_lossy());
        let _guard = EnvGuard::new(&["PATH", "PVE_SAN_TEST_DATA_DIR"]);
        env::set_var("PATH", new_path);
        env::set_var("PVE_SAN_TEST_DATA_DIR", test_data_dir());

        let output = Command::new(pve_san_query_path())
            .args(["--mode", "pvesh"])
            .current_dir(workspace_root())
            .output()
            .expect("Failed to run pve-san-query");

        fs::remove_dir_all(&unique_temp).ok();

        assert!(
            !output.status.success(),
            "pve-san-query should fail when node is omitted in pvesh mode"
        );
    }
}

struct EnvGuard {
    saved_vars: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn new(keys: &[&str]) -> Self {
        let mut saved_vars = Vec::new();
        for key in keys {
            let val = std::env::var_os(key);
            saved_vars.push((key.to_string(), val));
        }
        Self { saved_vars }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, val) in &self.saved_vars {
            if let Some(v) = val {
                std::env::set_var(key, v);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}
