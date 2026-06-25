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

/// Recursively traverses `/sys/block/{dm_name}/slaves/` to find all parent multipath devices.
///
/// A device is considered a multipath device if its mapped name starts with `"mpath"`.
/// Keeps track of visited devices to prevent infinite loops.
pub fn find_multipaths_for_dm(dm_name: &str, visited: &mut HashSet<String>) -> HashSet<String> {
    let mut mpaths = HashSet::new();
    if !visited.insert(dm_name.to_string()) {
        return mpaths;
    }

    // Check if dm_name itself is a multipath device
    let name_path = format!("/sys/block/{dm_name}/dm/name");
    if let Ok(mapped_name) = std::fs::read_to_string(name_path).map(|s| s.trim().to_string()) {
        if mapped_name.starts_with("mpath") {
            mpaths.insert(mapped_name);
            return mpaths;
        }
    }

    // Otherwise, check its slaves
    let slaves_dir = format!("/sys/block/{dm_name}/slaves");
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
