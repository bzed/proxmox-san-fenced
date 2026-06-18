//! Integration tests for pve-san-query tool
//!
//! These tests use the pvesh-mock tool to simulate pvesh responses.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Helper to get the workspace root directory
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the directory containing pve-san-query's Cargo.toml
    // which is /home/bzed/workspace/conova/vibe/pve-san-fenced/tools/pve-san-query
    // We need to go up two levels to get the workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
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

/// Run pve-san-query with pvesh-mock in PATH
fn run_pve_san_query(args: &[&str]) -> Vec<u8> {
    let temp_dir = env::temp_dir();
    let script_path = temp_dir.join("pvesh");
    
    // Create a wrapper script that calls pvesh-mock
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script_content = format!(
            "#!/bin/sh\nexec {} \"$@\"",
            pvesh_mock_path().display()
        );
        fs::write(&script_path, script_content).unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        
        // Set PATH to include temp dir with our mock pvesh
        let path = env::var_os("PATH").unwrap();
        let new_path = format!("{}:{}", temp_dir.display(), path.to_string_lossy());
        env::set_var("PATH", new_path);
        
        // Set environment variable for pvesh-mock
        env::set_var("PVE_SAN_TEST_DATA_DIR", test_data_dir());
        
        // Run pve-san-query
        let output = Command::new(pve_san_query_path())
            .args(args)
            .current_dir(&workspace_root())
            .output()
            .expect("Failed to run pve-san-query");
        
        // Clean up
        fs::remove_file(&script_path).ok();
        env::remove_var("PATH");
        env::remove_var("PVE_SAN_TEST_DATA_DIR");
        
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
    let _data: serde_json::Value = serde_json::from_str(&json_output)
        .expect("Output should be valid JSON");
    
    // If we got here, the JSON is valid
}

#[test]
fn test_pve_san_query_has_expected_structure() {
    let output = run_pve_san_query(&["--node", "pve001"]);
    
    let json_output = String::from_utf8(output).expect("Output should be valid UTF-8");
    let data: serde_json::Value = serde_json::from_str(&json_output)
        .expect("Output should be valid JSON");
    
    // Check that we have the expected structure
    assert!(data.get("node").is_some(), "Should have 'node' field");
    assert!(data.get("vms").is_some(), "Should have 'vms' field");
    
    // Check node value
    assert_eq!(data["node"], "pve001");
    
    // Check that vms is an array
    assert!(data["vms"].is_array(), "vms should be an array");
}

#[test]
fn test_pve_san_query_with_output_file() {
    let temp_dir = env::temp_dir();
    let output_file = temp_dir.join("pve_san_query_test_output.json");
    
    // Clean up if file exists
    fs::remove_file(&output_file).ok();
    
    let output_file_clone = output_file.clone();
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        
        let script_path = temp_dir.join("pvesh");
        let script_content = format!(
            "#!/bin/sh\nexec {} \"$@\"",
            pvesh_mock_path().display()
        );
        fs::write(&script_path, script_content).unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        
        // Set PATH to include temp dir with our mock pvesh
        let path = env::var_os("PATH").unwrap();
        let new_path = format!("{}:{}", temp_dir.display(), path.to_string_lossy());
        env::set_var("PATH", new_path);
        
        // Set environment variable for pvesh-mock
        env::set_var("PVE_SAN_TEST_DATA_DIR", test_data_dir());
        
        // Run pve-san-query with output file
        let output = Command::new(pve_san_query_path())
            .args(&["--node", "pve001", "--output", output_file.to_str().unwrap()])
            .current_dir(&workspace_root())
            .output()
            .expect("Failed to run pve-san-query");
        
        // Check that the command succeeded
        assert!(output.status.success(), "pve-san-query should succeed");
        
        // Clean up
        fs::remove_file(&script_path).ok();
        env::remove_var("PATH");
        env::remove_var("PVE_SAN_TEST_DATA_DIR");
        
        // Check that the output file was created
        assert!(output_file_clone.exists(), "Output file should exist");
        
        // Read and verify the output file
        let data = fs::read_to_string(&output_file_clone).expect("Should be able to read output file");
        let _json: serde_json::Value = serde_json::from_str(&data)
            .expect("Output file should contain valid JSON");
        
        // Clean up the output file
        fs::remove_file(&output_file_clone).ok();
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
    assert!(json_output.contains('\n'), "Pretty output should contain newlines");
    assert!(json_output.contains("  "), "Pretty output should contain indentation");
    
    // Should still be valid JSON
    let _data: serde_json::Value = serde_json::from_str(&json_output)
        .expect("Pretty output should still be valid JSON");
}
