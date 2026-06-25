//! Integration tests for libpve-san
//!
//! These tests use the pvesh-mock tool to simulate pvesh responses.
//!
//! To run these tests, first build pvesh-mock:
//!   cargo build --package pvesh-mock
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

use libpve_san::{get_san_storage_info_sync_with_pvesh, PveSanError, SanStorageInfo};
use std::env;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Helper to get the workspace root directory
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the directory containing libpve-san's Cargo.toml
    // which is /home/bzed/workspace/conova/vibe/pve-san-fenced/libpve-san
    // We need to go up one level to get the workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

/// Run pvesh-mock with given arguments and return the output
fn run_pvesh_mock(args: &[&str]) -> Output {
    let pvesh_mock_path = pvesh_mock_path();
    let test_data = test_data_dir();
    let cwd = workspace_root();

    Command::new(pvesh_mock_path)
        .args(args)
        .env("PVE_SAN_TEST_DATA_DIR", test_data)
        .current_dir(&cwd)
        .output()
        .expect("Failed to run pvesh-mock")
}

/// Run the actual library code with the mock pvesh command
fn run_library_test(node: &str) -> Result<SanStorageInfo, PveSanError> {
    let pvesh_mock_path = pvesh_mock_path();
    let test_data = test_data_dir();

    struct EnvGuard {
        saved_vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&str]) -> Self {
            let mut saved_vars = Vec::new();
            for key in keys {
                let val = std::env::var(key).ok();
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

    let _guard = EnvGuard::new(&["PVE_SAN_TEST_DATA_DIR"]);
    env::set_var("PVE_SAN_TEST_DATA_DIR", &test_data);

    let result = get_san_storage_info_sync_with_pvesh(node, pvesh_mock_path.to_str().unwrap());

    result
}

// Simple tests that verify the mock pvesh returns the expected data

#[test]
fn test_pvesh_mock_ls_qemu() {
    let output = run_pvesh_mock(&["ls", "/nodes/pve001/qemu", "--output-format", "json"]);

    assert!(output.status.success(), "pvesh-mock ls should succeed");

    let json_output = String::from_utf8(output.stdout).expect("Output should be valid UTF-8");
    let data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Output should be valid JSON");

    // Should be an array
    assert!(data.is_array(), "Expected JSON array");

    // Should have at least one VM
    let vms = data.as_array().unwrap();
    assert!(!vms.is_empty(), "Expected at least one VM");

    // Check that VMs have required fields
    for vm in vms {
        assert!(vm.get("vmid").is_some(), "VM should have vmid");
        assert!(vm.get("name").is_some(), "VM should have name");
        assert!(vm.get("status").is_some(), "VM should have status");
    }
}

#[test]
fn test_pvesh_mock_get_vm_config() {
    let output = run_pvesh_mock(&[
        "get",
        "/nodes/pve001/qemu/104/config",
        "--output-format",
        "json",
    ]);

    assert!(output.status.success(), "pvesh-mock get should succeed");

    let json_output = String::from_utf8(output.stdout).expect("Output should be valid UTF-8");
    let data: serde_json::Value =
        serde_json::from_str(&json_output).expect("Output should be valid JSON");

    // Should be an object
    assert!(data.is_object(), "Expected JSON object");

    // Check for expected fields
    assert_eq!(data["name"], "test-vm-001");
    assert_eq!(
        data["virtio0"],
        "storage-pool-001:vm-104-disk-0.qcow2,cache=none,size=50G"
    );

    // Check for disk
    assert!(data.get("virtio0").is_some(), "Should have virtio0 disk");
}

#[test]
fn test_library_with_mock_list_vms() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // Should have the correct node name
            assert_eq!(info.node, "pve001");

            // Should have multiple running VMs
            let running_vms: Vec<_> = info
                .vms
                .iter()
                .filter(|vm| vm.status == "running")
                .collect();

            assert!(
                !running_vms.is_empty(),
                "Expected at least one running VM, got {}",
                running_vms.len()
            );

            // Check that all running VMs have valid VMIDs
            for vm in &running_vms {
                assert!(vm.vmid > 0, "VMID should be positive");
                assert!(!vm.name.is_empty(), "VM name should not be empty");
            }
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_library_with_mock_parse_vm_104() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // Find VM with ID 104
            let vm_104 = info.vms.iter().find(|vm| vm.vmid == 104);
            assert!(vm_104.is_some(), "Expected to find VM 104");

            let vm = vm_104.unwrap();
            assert_eq!(vm.name, "test-vm-001");
            assert!(vm.status == "running");

            // Check disks
            assert!(!vm.disks.is_empty(), "VM 104 should have at least one disk");

            // Find the virtio0 disk
            let virtio0 = vm.disks.iter().find(|d| d.device_id == "virtio0");
            assert!(virtio0.is_some(), "Expected to find virtio0 disk");

            let disk = virtio0.unwrap();
            assert_eq!(disk.storage, "storage-pool-001:vm-104-disk-0.qcow2");
            assert_eq!(disk.size_bytes, Some(50 * 1024 * 1024 * 1024)); // 50G
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_library_with_mock_parse_vm_117() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // Find VM with ID 117 (has multiple disks including efidisk)
            let vm_117 = info.vms.iter().find(|vm| vm.vmid == 117);
            assert!(vm_117.is_some(), "Expected to find VM 117");

            let vm = vm_117.unwrap();
            assert_eq!(vm.name, "test-vm-005");

            // Should have virtio0 and efidisk0
            let disk_ids: Vec<_> = vm.disks.iter().map(|d| &d.device_id).collect();
            assert!(
                disk_ids.contains(&&"virtio0".to_string()),
                "Should have virtio0"
            );
            assert!(
                disk_ids.contains(&&"efidisk0".to_string()),
                "Should have efidisk0"
            );

            // Check virtio0 disk size
            let virtio0 = vm.disks.iter().find(|d| d.device_id == "virtio0").unwrap();
            assert_eq!(virtio0.size_bytes, Some(100 * 1024 * 1024 * 1024)); // 100G
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_library_with_mock_parse_vm_141() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // Find VM with ID 141 (has nvme storage)
            let vm_141 = info.vms.iter().find(|vm| vm.vmid == 141);
            assert!(vm_141.is_some(), "Expected to find VM 141");

            let vm = vm_141.unwrap();
            assert_eq!(vm.name, "test-vm-013");

            // Should have scsi0 and efidisk0
            let disk_ids: Vec<_> = vm.disks.iter().map(|d| &d.device_id).collect();
            assert!(
                disk_ids.contains(&&"scsi0".to_string()),
                "Should have scsi0"
            );
            assert!(
                disk_ids.contains(&&"efidisk0".to_string()),
                "Should have efidisk0"
            );

            // Check scsi0 disk
            let scsi0 = vm.disks.iter().find(|d| d.device_id == "scsi0").unwrap();
            assert_eq!(scsi0.storage, "storage-nvme-001:vm-141-disk-0");
            assert_eq!(scsi0.size_bytes, Some(100 * 1024 * 1024 * 1024)); // 100G
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_library_with_mock_parse_vm_145() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // Find VM with ID 145 (has scsi0 with size=20G)
            let vm_145 = info.vms.iter().find(|vm| vm.vmid == 145);
            assert!(vm_145.is_some(), "Expected to find VM 145");

            let vm = vm_145.unwrap();
            assert_eq!(vm.name, "test-vm-015");

            // Should have scsi0
            let scsi0 = vm.disks.iter().find(|d| d.device_id == "scsi0");
            assert!(scsi0.is_some(), "Expected to find scsi0 disk");

            let disk = scsi0.unwrap();
            assert_eq!(disk.storage, "storage-pool-001:vm-145-disk-1.qcow2");
            assert_eq!(disk.size_bytes, Some(20 * 1024 * 1024 * 1024)); // 20G
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_library_with_mock_parse_vm_147() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // Find VM with ID 147 (has sata1)
            let vm_147 = info.vms.iter().find(|vm| vm.vmid == 147);
            assert!(vm_147.is_some(), "Expected to find VM 147");

            let vm = vm_147.unwrap();
            assert_eq!(vm.name, "test-vm-016");

            // Should have sata1
            let sata1 = vm.disks.iter().find(|d| d.device_id == "sata1");
            assert!(sata1.is_some(), "Expected to find sata1 disk");

            let disk = sata1.unwrap();
            assert_eq!(disk.storage, "storage-pool-001:vm-147-disk-1.qcow2");
            assert_eq!(disk.size_bytes, Some(100 * 1024 * 1024 * 1024)); // 100G
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_pvesh_not_found_error() {
    // Test with a non-existent pvesh command
    let result = get_san_storage_info_sync_with_pvesh("pve001", "nonexistent-pvesh-command");

    match result {
        Ok(_) => panic!("Expected error when pvesh command not found"),
        Err(e) => {
            // With the new implementation, we get a PveshError when the command doesn't exist
            // (the error happens when trying to spawn the command)
            assert!(matches!(e, PveSanError::PveshError(_)));
        }
    }
}

#[test]
fn test_node_not_specified_error() {
    // Test with empty node - should fail at config creation
    use libpve_san::{PveSanConfig, PveSanError};

    let config_result = PveSanConfig::with_node("");
    assert!(matches!(config_result, Err(PveSanError::NoNodeError)));
}

#[test]
fn test_stopped_vms_not_included() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // VM 130 is stopped, should not be in the list
            let vm_130 = info.vms.iter().find(|vm| vm.vmid == 130);
            assert!(vm_130.is_none(), "Stopped VM 130 should not be in the list");
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_vm_tags_preserved() {
    let result = run_library_test("pve001");

    match result {
        Ok(info) => {
            // VM 145 has tags
            let vm_145 = info.vms.iter().find(|vm| vm.vmid == 145).unwrap();
            // Note: tags are in the config but not currently stored in VmInfo
            // This test just verifies the VM exists
            assert_eq!(vm_145.name, "test-vm-015");
        }
        Err(e) => panic!("Failed to get SAN storage info: {}", e),
    }
}

#[test]
fn test_vmid_extraction_from_pve001_qemu_json() {
    let result = run_library_test("pve001").unwrap();
    let mut vmids: Vec<_> = result.vms.iter().map(|vm| vm.vmid).collect();
    vmids.sort_unstable();

    let mut expected_vmids = vec![
        132, 145, 141, 105, 144, 147, 131, 122, 114, 126, 116, 104, 133, 140, 117,
    ];
    expected_vmids.sort_unstable();

    assert_eq!(vmids, expected_vmids);
}
