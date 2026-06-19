//! pve-san-fenced: SAN fencing daemon library for Proxmox VE
//!
//! Exposes core data structures and logic for monitoring multipath storage
//! states and mapping VM configurations to block devices.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::Path;
use std::time::Duration;
use log::{error, warn};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct LsblkDevice {
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub children: Option<Vec<LsblkDevice>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LsblkOutput {
    pub blockdevices: Option<Vec<LsblkDevice>>,
}

#[derive(Deserialize, Debug)]
pub struct MultipathOutput {
    pub maps: Option<Vec<MultipathMap>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MultipathMap {
    pub name: String,
    pub uuid: String,
    #[serde(rename = "path_groups")]
    pub path_groups: Option<Vec<PathGroup>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PathGroup {
    pub paths: Option<Vec<MpathPath>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MpathPath {
    #[serde(rename = "dm_st")]
    pub dm_st: Option<String>,
}

/// Recursively traverses the lsblk device tree to build a map of device names
/// to their parent multipath device names (e.g. "storage-pool-001-vm--104--disk--0.qcow2" -> {"mpatha", "mpathb"}).
pub fn build_mpath_map(
    devices: &[LsblkDevice],
    current_mpath: Option<&str>,
    map: &mut HashMap<String, HashSet<String>>,
) {
    for dev in devices {
        let next_mpath = if dev.device_type == "mpath" {
            Some(dev.name.as_str())
        } else {
            current_mpath
        };

        if let Some(mpath) = next_mpath {
            map.entry(dev.name.clone())
                .or_default()
                .insert(mpath.to_string());
        }

        if let Some(children) = &dev.children {
            build_mpath_map(children, next_mpath, map);
        }
    }
}

/// Converts a Proxmox storage identifier (e.g., "vg:lv") to the device-mapper
/// format (e.g., "vg-lv_doubled" where single dashes in lv are doubled).
pub fn storage_to_dm_name(storage: &str) -> String {
    if let Some((vg, lv)) = storage.split_once(':') {
        format!("{vg}-{}", lv.replace("-", "--"))
    } else {
        storage.to_string()
    }
}

/// Discovers multipath devices in use by running VMs
pub async fn discover_in_use_mpaths(
    node: &str,
    pvesh_command: &str,
) -> Result<HashSet<String>, Box<dyn std::error::Error + Send + Sync>> {
    // 1. Get VM and storage info using libpve-san
    let client = libpve_san::PveSanClient::with_node_and_pvesh(
        node.to_string(),
        pvesh_command.to_string(),
    )?;
    let storage_info = client.get_san_storage_info().await?;

    // 2. Fetch lsblk tree (either mock data or command execution)
    let lsblk_json = if let Ok(test_data_dir) = env::var("PVE_SAN_TEST_DATA_DIR") {
        let path = Path::new(&test_data_dir).join("lsblk.json");
        tokio::fs::read_to_string(path).await?
    } else {
        let output = tokio::process::Command::new("lsblk")
            .args(["-o", "NAME,TYPE", "-J"])
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!("lsblk command failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        String::from_utf8(output.stdout)?
    };

    let lsblk_output: LsblkOutput = serde_json::from_str(&lsblk_json)?;
    let mut mpath_map = HashMap::new();
    if let Some(devices) = lsblk_output.blockdevices {
        build_mpath_map(&devices, /*current_mpath*/ None, &mut mpath_map);
    }

    // 3. Map running VM disks to their parent multipath devices
    let mut active_mpaths = HashSet::new();
    for vm in storage_info.vms {
        if vm.status == "running" {
            for disk in vm.disks {
                let dm_name = storage_to_dm_name(&disk.storage);
                if let Some(mpaths) = mpath_map.get(&dm_name) {
                    for mpath in mpaths {
                        active_mpaths.insert(mpath.clone());
                    }
                } else if let Some(mpaths) = mpath_map.get(&disk.storage) {
                    for mpath in mpaths {
                        active_mpaths.insert(mpath.clone());
                    }
                } else if let Some(dm_name_only) = dm_name.strip_prefix("/dev/mapper/") {
                    if let Some(mpaths) = mpath_map.get(dm_name_only) {
                        for mpath in mpaths {
                            active_mpaths.insert(mpath.clone());
                        }
                    }
                }
            }
        }
    }

    Ok(active_mpaths)
}

/// Evaluates if a multipath map has lost all paths
pub fn is_map_dead(map: &MultipathMap) -> bool {
    if let Some(pgs) = &map.path_groups {
        for pg in pgs {
            if let Some(paths) = &pg.paths {
                for path in paths {
                    if let Some(dm_st) = &path.dm_st {
                        if dm_st != "failed" && dm_st != "faulty" {
                            // Found an active/enabled path
                            return false;
                        }
                    } else {
                        // If dm_st is missing, assume it might be alive to prevent false reboots
                        return false;
                    }
                }
            }
        }
        true
    } else {
        true
    }
}

/// Executes the fencing sequence
pub async fn trigger_fencing() {
    warn!("SAN FENCER: Total persistent storage loss detected. Threshold met.");
    warn!("SAN FENCER: Initiating filesystem sync...");

    // Sync filesystems
    unsafe {
        libc::sync();
    }
    tokio::time::sleep(Duration::from_secs(/*secs*/ 2)).await;

    if env::var("PVE_SAN_FENCE_DRY_RUN").is_ok() {
        warn!("SAN FENCER: DRY RUN: Fencing triggered. Exiting daemon.");
        std::process::exit(/*code*/ 0);
    }

    warn!("SAN FENCER: Triggering SysRq Kernel Panic NOW.");

    // Attempt kernel panic
    if let Err(e) = tokio::fs::write("/proc/sysrq-trigger", "c").await {
        error!("Failed to write 'c' to sysrq-trigger: {e}");
        // Fallback to reboot
        if let Err(err) = tokio::fs::write("/proc/sysrq-trigger", "b").await {
            error!("Failed to write 'b' to sysrq-trigger: {err}");
        }
    }
}

/// Stateful fencer to evaluate storage path states across cycles
pub struct Fencer {
    pub consecutive_failures: u64,
    pub max_failures: u64,
    pub target_wwids: HashSet<String>,
}

impl Fencer {
    /// Creates a new Fencer
    pub fn new(max_failures: u64, target_wwids: HashSet<String>) -> Self {
        Self {
            consecutive_failures: 0,
            max_failures,
            target_wwids,
        }
    }

    /// Evaluates the current state of multipath maps against the active LUN set.
    /// Returns true if fencing should be triggered.
    pub fn update(&mut self, maps: &[MultipathMap], active_luns: &HashSet<String>) -> bool {
        let monitored_maps: Vec<&MultipathMap> = maps
            .iter()
            .filter(|map| {
                let is_active = active_luns.contains(&map.name) || active_luns.contains(&map.uuid);
                let is_targeted = if self.target_wwids.is_empty() {
                    true
                } else {
                    self.target_wwids.contains(&map.uuid) || self.target_wwids.contains(&map.name)
                };
                is_active && is_targeted
            })
            .collect();

        if monitored_maps.is_empty() {
            if self.consecutive_failures > 0 {
                log::info!("No active maps monitored. Resetting failure counter.");
            }
            self.consecutive_failures = 0;
            return false;
        }

        let mut all_paths_dead = true;
        for map in &monitored_maps {
            if !is_map_dead(map) {
                all_paths_dead = false;
                break;
            }
        }

        if all_paths_dead {
            self.consecutive_failures += 1;
            log::warn!(
                "Consecutive storage failure: {}/{}",
                self.consecutive_failures, self.max_failures
            );

            if self.consecutive_failures >= self.max_failures {
                return true;
            }
        } else {
            if self.consecutive_failures > 0 {
                log::info!("Storage connectivity restored. Resetting failure counter.");
            }
            self.consecutive_failures = 0;
        }

        false
    }
}
