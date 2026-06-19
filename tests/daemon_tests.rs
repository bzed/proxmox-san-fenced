//! Integration tests for pve-san-fenced daemon library.
//!
//! These tests verify the core mapping, parsing, and failure detection
//! logic of the SAN fencing daemon.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use pve_san_fenced::{
    build_mpath_map, discover_in_use_mpaths, is_map_dead, storage_to_dm_name, LsblkDevice,
    MpathPath, MultipathMap, PathGroup,
};

/// Helper to get the workspace root directory
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Helper to get the test data directory for Proxmox configuration
fn test_data_dir() -> PathBuf {
    workspace_root().join("test-data/pvesh")
}

/// Get the path to the pvesh-mock binary
fn pvesh_mock_path() -> PathBuf {
    workspace_root().join("target/debug/pvesh-mock")
}

#[test]
fn test_storage_to_dm_name() {
    assert_eq!(
        storage_to_dm_name("storage-pool-001:vm-104-disk-0.qcow2"),
        "storage-pool-001-vm--104--disk--0.qcow2"
    );
    assert_eq!(
        storage_to_dm_name("storage-nvme-001:vm-141-disk-0"),
        "storage-nvme-001-vm--141--disk--0"
    );
    assert_eq!(
        storage_to_dm_name("local-lvm:vm-100-disk-0"),
        "local-lvm-vm--100--disk--0"
    );
    assert_eq!(
        storage_to_dm_name("some-direct-device"),
        "some-direct-device"
    );
}

#[test]
fn test_build_mpath_map() {
    let devices = vec![
        LsblkDevice {
            name: "sda".to_string(),
            device_type: "disk".to_string(),
            children: Some(vec![LsblkDevice {
                name: "mpatha".to_string(),
                device_type: "mpath".to_string(),
                children: Some(vec![
                    LsblkDevice {
                        name: "storage-pool-001-vm--104--disk--0.qcow2".to_string(),
                        device_type: "lvm".to_string(),
                        children: None,
                    },
                    LsblkDevice {
                        name: "storage-pool-001-vm--116--disk--0.qcow2".to_string(),
                        device_type: "lvm".to_string(),
                        children: None,
                    },
                ]),
            }]),
        },
        LsblkDevice {
            name: "sdb".to_string(),
            device_type: "disk".to_string(),
            children: Some(vec![LsblkDevice {
                name: "mpathb".to_string(),
                device_type: "mpath".to_string(),
                children: Some(vec![LsblkDevice {
                    name: "storage-pool-001-vm--104--disk--0.qcow2".to_string(),
                    device_type: "lvm".to_string(),
                    children: None,
                }]),
            }]),
        },
    ];

    let mut mpath_map = HashMap::new();
    build_mpath_map(&devices, /*current_mpath*/ None, &mut mpath_map);

    // LV vm--104--disk--0 is spanned across mpatha and mpathb
    let mpaths_104 = mpath_map.get("storage-pool-001-vm--104--disk--0.qcow2").unwrap();
    assert_eq!(mpaths_104.len(), 2);
    assert!(mpaths_104.contains("mpatha"));
    assert!(mpaths_104.contains("mpathb"));

    let mpaths_116 = mpath_map.get("storage-pool-001-vm--116--disk--0.qcow2").unwrap();
    assert_eq!(mpaths_116.len(), 1);
    assert!(mpaths_116.contains("mpatha"));
}

#[test]
fn test_is_map_dead_alive_cases() {
    // 1. Map with active paths
    let alive_map = MultipathMap {
        name: "mpatha".to_string(),
        uuid: "368c".to_string(),
        path_groups: Some(vec![
            PathGroup {
                paths: Some(vec![
                    MpathPath {
                        dm_st: Some("active".to_string()),
                    },
                    MpathPath {
                        dm_st: Some("failed".to_string()),
                    },
                ]),
            },
            PathGroup {
                paths: Some(vec![MpathPath {
                    dm_st: Some("enabled".to_string()),
                }]),
            },
        ]),
    };
    assert!(!is_map_dead(&alive_map));

    // 2. Map with missing state should not be treated as dead (fail-safe)
    let missing_st_map = MultipathMap {
        name: "mpatha".to_string(),
        uuid: "368c".to_string(),
        path_groups: Some(vec![PathGroup {
            paths: Some(vec![MpathPath { dm_st: None }]),
        }]),
    };
    assert!(!is_map_dead(&missing_st_map));
}

#[test]
fn test_is_map_dead_failed_cases() {
    // 1. Map with all paths failed
    let dead_map = MultipathMap {
        name: "mpatha".to_string(),
        uuid: "368c".to_string(),
        path_groups: Some(vec![
            PathGroup {
                paths: Some(vec![
                    MpathPath {
                        dm_st: Some("failed".to_string()),
                    },
                    MpathPath {
                        dm_st: Some("failed".to_string()),
                    },
                ]),
            },
            PathGroup {
                paths: Some(vec![MpathPath {
                    dm_st: Some("failed".to_string()),
                }]),
            },
        ]),
    };
    assert!(is_map_dead(&dead_map));

    // 2. Map with empty path groups
    let empty_map = MultipathMap {
        name: "mpatha".to_string(),
        uuid: "368c".to_string(),
        path_groups: Some(vec![]),
    };
    assert!(is_map_dead(&empty_map));

    // 3. Map with no path groups field
    let no_pg_map = MultipathMap {
        name: "mpatha".to_string(),
        uuid: "368c".to_string(),
        path_groups: None,
    };
    assert!(is_map_dead(&no_pg_map));
}

#[tokio::test]
async fn test_discover_in_use_mpaths_integration() {
    let pvesh_mock = pvesh_mock_path();
    let test_data = test_data_dir();

    // Set the environment variables for mocking
    env::set_var("PVE_SAN_TEST_DATA_DIR", &test_data);

    // Call discovery logic
    let result = discover_in_use_mpaths("pve001", pvesh_mock.to_str().unwrap()).await;

    // Clean up
    env::remove_var("PVE_SAN_TEST_DATA_DIR");

    assert!(result.is_ok(), "discover_in_use_mpaths failed: {:?}", result.err());
    let active_mpaths = result.unwrap();

    // Verify discovered multipath devices match expected ones based on running VMs in pve001
    // Running VMs in pve001: 104, 105, 114, 116, 117, 122, 126, 131, 132, 133, 140, 141, 144, 145, 147
    // Looking at lsblk.json:
    // - vm-104 is on mpatha and mpathb
    // - vm-114 is on mpathb
    // - vm-116 is on mpatha and mpathb
    // - vm-126 is on mpatha and mpathb
    // - vm-147 is on mpathb
    // ...
    // So both mpatha and mpathb must be in the active set
    assert!(active_mpaths.contains("mpatha"), "mpatha should be active");
    assert!(active_mpaths.contains("mpathb"), "mpathb should be active");

    // mpathc is only used by test-adm, not any running VM, so it should NOT be active
    assert!(!active_mpaths.contains("mpathc"), "mpathc should not be active");
}
