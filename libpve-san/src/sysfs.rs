//! Traversal helpers for the Linux sysfs block device directory structure.
//!
//! Provides utilities to resolve parent multipath devices for device-mapper
//! devices by recursively exploring their slave devices.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.

use std::collections::HashSet;

fn sys_prefix() -> String {
    std::env::var("PVE_SAN_SYS_PATH").unwrap_or_else(|_| "/sys".to_string())
}

/// Recursively traverses `/sys/block/{dm_name}/slaves/` to find all parent multipath devices.
///
/// A device is considered a multipath device if its mapped name starts with `"mpath"`.
/// Keeps track of visited devices to prevent infinite loops.
pub fn find_multipaths_for_dm(dm_name: &str, visited: &mut HashSet<String>) -> HashSet<String> {
    let mut mpaths = HashSet::new();
    if !visited.insert(dm_name.to_string()) {
        return mpaths;
    }

    let prefix = sys_prefix();

    // Check if dm_name itself is a multipath device
    let name_path = format!("{prefix}/block/{dm_name}/dm/name");
    if let Ok(mapped_name) = std::fs::read_to_string(name_path).map(|s| s.trim().to_string()) {
        if mapped_name.starts_with("mpath") {
            mpaths.insert(mapped_name);
            return mpaths;
        }
    }

    // Otherwise, check its slaves
    let slaves_dir = format!("{prefix}/block/{dm_name}/slaves");
    if let Ok(entries) = std::fs::read_dir(slaves_dir) {
        for entry in entries.flatten() {
            let slave_name = entry.file_name().to_string_lossy().to_string();
            if slave_name.starts_with("dm-") {
                let sub_mpaths = find_multipaths_for_dm(&slave_name, visited);
                mpaths.extend(sub_mpaths);
            }
        }
    }

    mpaths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};

    #[test]
    fn test_find_multipaths_for_dm() {
        let temp_dir_path = std::env::current_dir()
            .unwrap()
            .join("target/mock-sys-test");
        let _ = std::fs::remove_dir_all(&temp_dir_path);

        let dm2_dir = temp_dir_path.join("block/dm-2");
        let dm0_dir = temp_dir_path.join("block/dm-0");
        let dm1_dir = temp_dir_path.join("block/dm-1");

        create_dir_all(dm2_dir.join("dm")).unwrap();
        create_dir_all(dm2_dir.join("slaves")).unwrap();
        write(
            dm2_dir.join("dm/name"),
            "storage-pool-001-vm--104--disk--0.qcow2\n",
        )
        .unwrap();
        write(dm2_dir.join("slaves/dm-0"), "").unwrap();
        write(dm2_dir.join("slaves/dm-1"), "").unwrap();

        create_dir_all(dm0_dir.join("dm")).unwrap();
        create_dir_all(dm0_dir.join("slaves")).unwrap();
        write(dm0_dir.join("dm/name"), "mpatha\n").unwrap();

        create_dir_all(dm1_dir.join("dm")).unwrap();
        create_dir_all(dm1_dir.join("slaves")).unwrap();
        write(dm1_dir.join("dm/name"), "mpathb\n").unwrap();

        std::env::set_var("PVE_SAN_SYS_PATH", &temp_dir_path);

        let mut visited = HashSet::new();
        let mpaths = find_multipaths_for_dm("dm-2", &mut visited);

        assert_eq!(mpaths.len(), 2);
        assert!(mpaths.contains("mpatha"));
        assert!(mpaths.contains("mpathb"));

        let _ = std::fs::remove_dir_all(&temp_dir_path);
        std::env::remove_var("PVE_SAN_SYS_PATH");
    }
}
