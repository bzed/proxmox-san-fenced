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

use log::{debug, error, info, warn};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::time::Duration;

pub mod config;
pub mod status;

#[derive(Deserialize, Debug)]
pub struct MultipathOutput {
    pub(crate) major_version: Option<u32>,
    pub(crate) minor_version: Option<u32>,
    pub(crate) maps: Option<Vec<MultipathMap>>,
}

pub fn parse_multipathd_response(response_json: &str) -> Option<Vec<MultipathMap>> {
    if response_json.len() > 10 * 1024 * 1024 {
        warn!("Rejected multipathd response: size exceeds 10MB limit");
        return None;
    }

    match serde_json::from_str::<MultipathOutput>(response_json) {
        Ok(out) => {
            if let (Some(major), Some(minor)) = (out.major_version, out.minor_version) {
                if major != 0 {
                    warn!("Unsupported multipathd JSON schema version: {major}.{minor}");
                }
            } else {
                warn!("Missing version fields in multipathd JSON response");
            }
            Some(out.maps.unwrap_or_default())
        }
        Err(e) => {
            warn!("Failed to parse multipathd response: {e}");
            None
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct MultipathMap {
    pub(crate) name: String,
    pub(crate) uuid: String,
    #[serde(rename = "path_groups")]
    pub(crate) path_groups: Option<Vec<PathGroup>>,
    pub(crate) vend: Option<String>,
    pub(crate) prod: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct PathGroup {
    #[serde(rename = "dm_st")]
    dm_st: Option<String>,
    paths: Option<Vec<MpathPath>>,
}

#[derive(Deserialize, Debug, Clone)]
struct MpathPath {
    #[serde(rename = "dm_st")]
    dm_st: Option<String>,
}

/// Discovers multipath devices in use by running VMs
#[tracing::instrument]
pub async fn discover_in_use_mpaths(
    node: &str,
    pvesh_command: &str,
    socket_path: Option<&str>,
    debug_mode: bool,
) -> Result<HashSet<String>, Box<dyn std::error::Error + Send + Sync>> {
    let fut = async {
        // 1. Get VM and storage info using libpve-san
        let client = libpve_san::PveSanClient::with_node_and_pvesh(node, pvesh_command)?;
        let storage_info = client.get_san_storage_info().await?;
        debug!("Discovered storage info: {:?}", storage_info);

        let mut maps_by_name_or_uuid = HashMap::new();
        if debug_mode {
            if let Some(socket) = socket_path {
                match libmultipath::send_multipath_command_to_socket(socket, "show maps json") {
                    Ok(response_str) => {
                        if let Ok(output) = serde_json::from_str::<MultipathOutput>(&response_str) {
                            if let Some(maps) = output.maps {
                                for map in maps {
                                    maps_by_name_or_uuid.insert(map.name.clone(), map.clone());
                                    maps_by_name_or_uuid.insert(map.uuid.clone(), map);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Debug log mode failed to query multipathd status: {e}");
                    }
                }
            }
        }

        // 2. Map running VM disks to their parent multipath devices using storage_info
        let mut active_mpaths = HashSet::new();
        for vm in &storage_info.vms {
            if vm.status == "running" {
                for disk in &vm.disks {
                    if let Some(dm_name) = &disk.device_mapper_name {
                        let mpaths: Vec<String> = dm_name.split(" / ").map(String::from).collect();
                        if debug_mode {
                            for mpath in &mpaths {
                                let state = if let Some(map) = maps_by_name_or_uuid.get(mpath) {
                                    if is_map_dead(map) {
                                        "failed"
                                    } else {
                                        "active"
                                    }
                                } else {
                                    "unknown"
                                };
                                let vm_name = &vm.name;
                                let vmid = vm.vmid;
                                let storage = &disk.storage;
                                info!(
                                    "Discovered VM: {vm_name} (ID: {vmid}), storage: {storage}, multipath device: {mpath}, state: {state}"
                                );
                            }
                        }

                        if mpaths.len() > 1 {
                            let mut sorted_mpaths = mpaths;
                            sorted_mpaths.sort();
                            active_mpaths.insert(sorted_mpaths.join("+"));
                        } else if let Some(mpath) = mpaths.first() {
                            active_mpaths.insert(mpath.clone());
                        }
                    }
                }
            }
        }

        debug!("Final active multipath set: {active_mpaths:?}");
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(active_mpaths)
    };

    match tokio::time::timeout(Duration::from_secs(/*secs*/ 30), fut).await {
        Ok(res) => res,
        Err(_) => Err("Discovery task timed out (30s limit exceeded)".into()),
    }
}

/// Evaluates if a multipath map has lost all paths
pub fn is_map_dead(map: &MultipathMap) -> bool {
    if let Some(pgs) = &map.path_groups {
        for pg in pgs {
            let pg_alive = match &pg.dm_st {
                Some(st) => st != "offline" && st != "failed",
                None => {
                    let map_name = &map.name;
                    warn!("dm_st is missing for path group in map '{map_name}'");
                    crate::status::get_status_tracker().set_issue(
                        &format!("missing_dm_st_{map_name}"),
                        crate::status::StatusLevel::Warning,
                        format!("dm_st is missing for map '{map_name}'"),
                    );
                    true
                }
            };
            if !pg_alive {
                continue;
            }

            if let Some(paths) = &pg.paths {
                let mut active_path_found = false;
                for path in paths {
                    if let Some(dm_st) = &path.dm_st {
                        if dm_st != "failed" && dm_st != "faulty" && dm_st != "ghost" {
                            active_path_found = true;
                            break;
                        }
                    } else {
                        let map_name = &map.name;
                        warn!("dm_st is missing for path in map '{map_name}'");
                        crate::status::get_status_tracker().set_issue(
                            &format!("missing_dm_st_{map_name}"),
                            crate::status::StatusLevel::Warning,
                            format!("dm_st is missing for map '{map_name}'"),
                        );
                        active_path_found = true;
                        break;
                    }
                }
                if active_path_found {
                    return false;
                }
            }
        }
        true
    } else {
        true
    }
}

static FENCING_IN_PROGRESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[tracing::instrument]
pub async fn trigger_fencing(sysrq_char: &str) {
    warn!("SAN FENCER: Total persistent storage loss detected. Threshold met.");

    if env::var("PVE_SAN_FENCE_DRY_RUN").is_ok() {
        warn!("SAN FENCER: DRY RUN: Fencing triggered. Exiting daemon.");
        // Give the status writing thread a brief moment to write the final CRITICAL state
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::process::exit(/*code*/ 0);
    }

    if FENCING_IN_PROGRESS.swap(true, std::sync::atomic::Ordering::SeqCst) {
        warn!("SAN FENCER: Fencing operation already in progress. Ignoring duplicate request.");
        return;
    }

    let chars: Vec<char> = sysrq_char
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .flat_map(str::chars)
        .collect();

    warn!("SAN FENCER: Triggering SysRq Fencing sequence: {chars:?}");

    let mut sent_reboot = false;
    for &c in &chars {
        warn!("SAN FENCER: Sending SysRq '{c}' to sysrq-trigger");
        if let Err(e) = tokio::fs::write("/proc/sysrq-trigger", c.to_string()).await {
            error!("Failed to write '{c}' to sysrq-trigger: {e}");
        } else if c == 'b' {
            sent_reboot = true;
            let timeout_secs = match env::var("PVE_SAN_FENCE_REBOOT_TIMEOUT") {
                Ok(val) => val.parse::<u64>().unwrap_or(10),
                Err(_) => 10,
            };
            warn!("SAN FENCER: Sent reboot character 'b'. Waiting {timeout_secs} seconds for system to reboot...");
            tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
            error!("SAN FENCER: CRITICAL: System did not reboot after {timeout_secs} seconds! Trying 'b' again...");
            if let Err(err) = tokio::fs::write("/proc/sysrq-trigger", "b").await {
                error!("Failed to write fallback 'b' to sysrq-trigger: {err}");
            }
        }

        if c == 's' {
            warn!("SAN FENCER: Waiting for sync to complete...");
            tokio::time::sleep(Duration::from_secs(/*secs*/ 1)).await;
        }
    }

    // Fallback to reboot if 'b' was not sent/attempted
    if !sent_reboot {
        warn!("SAN FENCER: Sequence did not contain 'b' or failed to reboot. Attempting fallback reboot ('b')...");
        if let Err(err) = tokio::fs::write("/proc/sysrq-trigger", "b").await {
            error!("Failed to write fallback 'b' to sysrq-trigger: {err}");
        }
        let timeout_secs = match env::var("PVE_SAN_FENCE_REBOOT_TIMEOUT") {
            Ok(val) => val.parse::<u64>().unwrap_or(10),
            Err(_) => 10,
        };
        tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
    }
}

/// Stateful fencer to evaluate storage path states across cycles
pub struct Fencer {
    consecutive_failures: u64,
    max_failures: u64,
    target_wwids: HashSet<String>,
    previous_map_states: HashMap<String, bool>,
}

impl Fencer {
    /// Creates a new Fencer
    pub fn new(max_failures: u64, target_wwids: HashSet<String>) -> Self {
        Self {
            consecutive_failures: 0,
            max_failures,
            target_wwids,
            previous_map_states: HashMap::new(),
        }
    }

    /// Returns the number of consecutive failures.
    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures
    }

    /// Returns the maximum allowed consecutive failures.
    pub fn max_failures(&self) -> u64 {
        self.max_failures
    }

    /// Evaluates the current state of multipath maps against the active LUN set by parsing a JSON response.
    /// Returns true if fencing should be triggered.
    pub fn update(&mut self, response_json: &str, active_luns: &HashSet<String>) -> bool {
        if let Some(maps) = crate::parse_multipathd_response(response_json) {
            self.update_with_maps(&maps, active_luns)
        } else {
            false
        }
    }

    /// Evaluates the current state of multipath maps against the active LUN set.
    /// Returns true if fencing should be triggered.
    pub fn update_with_maps(
        &mut self,
        maps: &[MultipathMap],
        active_luns: &HashSet<String>,
    ) -> bool {
        crate::status::get_status_tracker().clear_issues_with_prefix("missing_dm_st_");
        debug!("All multipath maps returned from multipathd: {maps:?}");

        let monitored_maps: Vec<&MultipathMap> = maps
            .iter()
            .filter(|map| {
                let is_active = active_luns.iter().any(|lun| {
                    if lun.contains('+') {
                        let parts: Vec<&str> = lun.split('+').collect();
                        parts.contains(&map.name.as_str()) || parts.contains(&map.uuid.as_str())
                    } else {
                        lun == &map.name || lun == &map.uuid
                    }
                });
                let is_targeted = if self.target_wwids.is_empty() {
                    true
                } else {
                    self.target_wwids.contains(&map.uuid) || self.target_wwids.contains(&map.name)
                };
                is_active && is_targeted
            })
            .collect();

        debug!("Monitored maps subset: {:?}", monitored_maps);

        for map in &monitored_maps {
            let name = &map.name;
            let pg = &map.path_groups;
            debug!("Map {name} path details: {pg:?}");
            let is_dead = is_map_dead(map);
            let prev_dead = self.previous_map_states.insert(map.name.clone(), is_dead);
            if prev_dead != Some(is_dead) {
                let status_str = if is_dead {
                    "FAILED (all paths dead)"
                } else {
                    "HEALTHY (active path(s) found)"
                };
                info!("Multipath map {name} state changed to: {status_str}");
            }
        }

        let monitored_names: HashSet<&String> = monitored_maps.iter().map(|m| &m.name).collect();
        self.previous_map_states
            .retain(|name, _| monitored_names.contains(name));

        if monitored_maps.is_empty() {
            if self.consecutive_failures > 0 {
                info!("No active maps monitored. Resetting failure counter.");
            }
            self.consecutive_failures = 0;
            debug!("Fencer cycle result - no monitored maps, consecutive_failures: 0");
            return false;
        }

        let mut has_failed_spanned = false;
        let mut all_groups_failed = true;

        for lun in active_luns {
            let group_failed = if lun.contains('+') {
                let parts: Vec<&str> = lun.split('+').collect();
                parts.iter().any(|&part| {
                    monitored_maps
                        .iter()
                        .find(|m| m.name == part || m.uuid == part)
                        .copied()
                        .map(is_map_dead)
                        .unwrap_or(false)
                })
            } else {
                monitored_maps
                    .iter()
                    .find(|m| m.name == *lun || m.uuid == *lun)
                    .copied()
                    .map(is_map_dead)
                    .unwrap_or(false)
            };

            if lun.contains('+') && group_failed {
                has_failed_spanned = true;
            }

            if !group_failed {
                all_groups_failed = false;
            }
        }

        let fencing_condition_met = has_failed_spanned || all_groups_failed;

        let cf = self.consecutive_failures;
        debug!(
            "Fencer cycle result - fencing_condition_met: {fencing_condition_met}, consecutive_failures: {cf}"
        );

        if fencing_condition_met {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            let cf = self.consecutive_failures;
            let mf = self.max_failures;
            let msg = format!("Consecutive storage failure: {cf}/{mf}");
            warn!("{msg}");

            if self.consecutive_failures >= self.max_failures {
                let dead_map_names: Vec<String> = monitored_maps
                    .iter()
                    .filter(|m| is_map_dead(m))
                    .map(|m| {
                        let name = &m.name;
                        let uuid = &m.uuid;
                        format!("{name} ({uuid})")
                    })
                    .collect();
                let targets = &self.target_wwids;
                let decision_msg = format!(
                    "DECISION: Rebooting node because monitored multipath maps in use by running VMs have failed. \
                     Failed monitored maps: {dead_map_names:?}. Active LUNs: {active_luns:?}. Target WWIDs: {targets:?}."
                );
                warn!("{decision_msg}");

                crate::status::get_status_tracker().set_issue(
                    "fencing",
                    crate::status::StatusLevel::Critical,
                    format!("Fencing decision reached: {decision_msg}"),
                );

                return true;
            } else {
                crate::status::get_status_tracker().set_issue(
                    "fencing",
                    crate::status::StatusLevel::Warning,
                    msg,
                );
            }
        } else {
            if self.consecutive_failures > 0 {
                info!("Storage connectivity restored. Resetting failure counter.");
            }
            self.consecutive_failures = 0;
            crate::status::get_status_tracker().clear_issue("fencing");
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::env;
    use std::path::PathBuf;

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
    fn test_is_map_dead_alive_cases() {
        // 1. Map with at least one active path
        let alive_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "36001405a415ff6800000000000000000".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("active".to_string()),
                paths: Some(vec![
                    MpathPath {
                        dm_st: Some("active".to_string()),
                    },
                    MpathPath {
                        dm_st: Some("failed".to_string()),
                    },
                ]),
            }]),
        };
        assert!(!is_map_dead(&alive_map));

        // 2. Map with missing dm_st field (treated as alive since not explicitly "failed")
        let missing_st_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "36001".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: None,
                paths: Some(vec![MpathPath { dm_st: None }]),
            }]),
        };
        assert!(!is_map_dead(&missing_st_map));

        // 3. Map with path state "undef" (treated as alive since it is not failed/faulty/ghost)
        let undef_path_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "36002".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("active".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("undef".to_string()),
                }]),
            }]),
        };
        assert!(!is_map_dead(&undef_path_map));

        // 4. Map with path group state "enabled"
        let enabled_pg_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "36003".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("enabled".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("active".to_string()),
                }]),
            }]),
        };
        assert!(!is_map_dead(&enabled_pg_map));

        // 5. Map with path group state "disabled" but with an active path
        let disabled_pg_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "36004".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("disabled".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("active".to_string()),
                }]),
            }]),
        };
        assert!(!is_map_dead(&disabled_pg_map));

        // 6. Map with path group state "undef"
        let undef_pg_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "36005".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("undef".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("active".to_string()),
                }]),
            }]),
        };
        assert!(!is_map_dead(&undef_pg_map));
    }

    #[test]
    fn test_is_map_dead_failed_cases() {
        // 1. Map with all failed paths
        let dead_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "368a".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("active".to_string()),
                paths: Some(vec![
                    MpathPath {
                        dm_st: Some("failed".to_string()),
                    },
                    MpathPath {
                        dm_st: Some("failed".to_string()),
                    },
                ]),
            }]),
        };
        assert!(is_map_dead(&dead_map));

        // 2. Map with empty paths list
        let empty_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "368b".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("active".to_string()),
                paths: Some(Vec::new()),
            }]),
        };
        assert!(is_map_dead(&empty_map));

        // 3. Map with no path groups field
        let no_pg_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "368c".to_string(),
            vend: None,
            prod: None,
            path_groups: None,
        };
        assert!(is_map_dead(&no_pg_map));

        // 4. Map with ghost paths only
        let ghost_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "368d".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("active".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("ghost".to_string()),
                }]),
            }]),
        };
        assert!(is_map_dead(&ghost_map));

        // 5. Map with inactive path group (dm_st is offline)
        let inactive_pg_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "368e".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("offline".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("active".to_string()),
                }]),
            }]),
        };
        assert!(is_map_dead(&inactive_pg_map));

        // 6. Map with path group state "failed"
        let failed_pg_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "368f".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("failed".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("active".to_string()),
                }]),
            }]),
        };
        assert!(is_map_dead(&failed_pg_map));

        // 7. Map with path state "faulty"
        let faulty_path_map = MultipathMap {
            name: "mpatha".to_string(),
            uuid: "368g".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("active".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("faulty".to_string()),
                }]),
            }]),
        };
        assert!(is_map_dead(&faulty_path_map));
    }

    #[tokio::test]
    async fn test_discover_in_use_mpaths_integration() {
        let pvesh_mock = pvesh_mock_path();
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

        // Set the environment variables for mocking
        let _guard = EnvGuard::new(&["PVE_SAN_TEST_DATA_DIR"]);
        env::set_var("PVE_SAN_TEST_DATA_DIR", &test_data);

        // Call discovery logic
        let result = discover_in_use_mpaths(
            "pve001",
            pvesh_mock.to_str().unwrap(),
            None,
            /*debug_mode*/ false,
        )
        .await;

        let err = result.as_ref().err();
        assert!(result.is_ok(), "discover_in_use_mpaths failed: {err:?}");
        let active_mpaths = result.unwrap();

        // Call discovery logic in debug mode (handles fallback warning elegantly when socket is missing)
        let result_debug = discover_in_use_mpaths(
            "pve001",
            pvesh_mock.to_str().unwrap(),
            Some("@/tmp/nonexistent-socket"),
            /*debug_mode*/ true,
        )
        .await;
        assert!(result_debug.is_ok());

        // Verify discovered multipath devices match expected ones based on running VMs in pve001
        assert!(active_mpaths.contains("mpatha"), "mpatha should be active");
        assert!(active_mpaths.contains("mpathb"), "mpathb should be active");

        // mpathc is only used by test-adm, not any running VM, so it should NOT be active
        assert!(
            !active_mpaths.contains("mpathc"),
            "mpathc should not be active"
        );
    }

    /// A step in a declarative monitoring loop simulation scenario
    struct ScenarioStep {
        /// The multipathd test data file name to read from
        multipath_file: &'static str,
        /// The list of active LUNs (mpath device names/WWIDs) for this step
        active_luns: HashSet<String>,
        /// Expected consecutive failures counter value AFTER this step
        expected_failures: u64,
        /// Expected fencing trigger result (true if triggered, false otherwise)
        expected_fencing: bool,
    }

    /// Runs a declarative fencing simulation scenario
    fn run_scenario(max_failures: u64, target_wwids: &[&str], steps: &[ScenarioStep]) {
        let target_wwids_set: HashSet<String> =
            target_wwids.iter().map(|s| s.to_string()).collect();
        let mut fencer = Fencer::new(max_failures, target_wwids_set);

        let test_data_base = workspace_root().join("test-data/multipathd/show_maps_json");

        for (i, step) in steps.iter().enumerate() {
            let file_path = test_data_base.join(step.multipath_file);
            let content = std::fs::read_to_string(&file_path).unwrap_or_else(|e| {
                let display_path = file_path.display();
                panic!("Failed to read {display_path}: {e}");
            });

            let fencing_triggered = fencer.update(&content, &step.active_luns);

            let result = (fencer.consecutive_failures(), fencing_triggered);
            let expected = (step.expected_failures, step.expected_fencing);
            assert_eq!(
                result, expected,
                "Step {i} failed: expected {expected:?} (consecutive_failures, fencing_triggered), got {result:?}"
            );
        }
    }

    #[test]
    fn test_fencing_scenario_sustained_failure() {
        let active_luns: HashSet<String> = vec!["mpatha".to_string()].into_iter().collect();

        let steps = vec![
            // 1. Initial healthy state
            ScenarioStep {
                multipath_file: "all_active_running.json",
                active_luns: active_luns.clone(),
                expected_failures: 0,
                expected_fencing: false,
            },
            // 2. Failure starts
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 1,
                expected_fencing: false,
            },
            // 3. Failure continues
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 2,
                expected_fencing: false,
            },
            // 4. Failure continues
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 3,
                expected_fencing: false,
            },
            // 5. Failure continues
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 4,
                expected_fencing: false,
            },
            // 6. Failure continues
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 5,
                expected_fencing: false,
            },
            // 7. Threshold reached -> fence!
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 6,
                expected_fencing: true,
            },
        ];

        run_scenario(6, &[], &steps);
    }

    #[test]
    fn test_fencing_scenario_transient_failure() {
        let active_luns: HashSet<String> = vec!["mpatha".to_string()].into_iter().collect();

        let steps = vec![
            // 1. Initial healthy state
            ScenarioStep {
                multipath_file: "all_active_running.json",
                active_luns: active_luns.clone(),
                expected_failures: 0,
                expected_fencing: false,
            },
            // 2. Failure starts
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 1,
                expected_fencing: false,
            },
            // 3. Failure continues
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 2,
                expected_fencing: false,
            },
            // 4. Recovery!
            ScenarioStep {
                multipath_file: "all_active_running.json",
                active_luns: active_luns.clone(),
                expected_failures: 0,
                expected_fencing: false,
            },
        ];

        run_scenario(6, &[], &steps);
    }

    #[test]
    fn test_fencing_scenario_not_in_use() {
        // The failed LUN (mpatha) is not in use (empty active luns list)
        let steps = vec![ScenarioStep {
            multipath_file: "failed_all_timeout.json",
            active_luns: HashSet::new(),
            expected_failures: 0,
            expected_fencing: false,
        }];

        run_scenario(6, &[], &steps);
    }

    #[test]
    fn test_fencing_scenario_targeted() {
        let active_luns: HashSet<String> = vec!["mpatha".to_string()].into_iter().collect();

        let steps = vec![
            // Target is non-existent WWID, so we ignore the failure on mpatha
            ScenarioStep {
                multipath_file: "failed_all_timeout.json",
                active_luns: active_luns.clone(),
                expected_failures: 0,
                expected_fencing: false,
            },
        ];

        run_scenario(6, &["3600nonexistentwwid"], &steps);
    }

    #[test]
    fn test_fencing_scenario_partial_failure() {
        let active_luns: HashSet<String> = vec!["mpatha".to_string(), "mpathb".to_string()]
            .into_iter()
            .collect();

        let steps = vec![
            // Both mpatha (alive) and mpathb (dead) are in use.
            // We should NOT trigger fencing since not all in-use LUNs are dead.
            ScenarioStep {
                multipath_file: "mpatha_active_mpathb_failed.json",
                active_luns: active_luns.clone(),
                expected_failures: 0,
                expected_fencing: false,
            },
        ];

        run_scenario(/*max_failures*/ 6, &[], &steps);
    }

    #[test]
    fn test_fencing_scenario_only_failed_in_use() {
        let active_luns: HashSet<String> = vec!["mpathb".to_string()].into_iter().collect();

        let steps = vec![
            // Only mpathb (dead) is in use.
            // Fencing should trigger after 3 consecutive failures.
            ScenarioStep {
                multipath_file: "mpatha_active_mpathb_failed.json",
                active_luns: active_luns.clone(),
                expected_failures: 1,
                expected_fencing: false,
            },
            ScenarioStep {
                multipath_file: "mpatha_active_mpathb_failed.json",
                active_luns: active_luns.clone(),
                expected_failures: 2,
                expected_fencing: false,
            },
            ScenarioStep {
                multipath_file: "mpatha_active_mpathb_failed.json",
                active_luns: active_luns.clone(),
                expected_failures: 3,
                expected_fencing: true,
            },
        ];

        run_scenario(/*max_failures*/ 3, &[], &steps);
    }

    #[test]
    fn test_fencer_update_large_json_and_recursion() {
        let mut fencer = Fencer::new(3, HashSet::new());
        let active = HashSet::new();

        // 1. Check size limit (> 10MB)
        let large_str = " ".repeat(10 * 1024 * 1024 + 1);
        assert!(!fencer.update(&large_str, &active));

        // 2. Check recursion limit
        // Deeply nested JSON array/objects to exceed default 128 level limit
        let mut nested = String::new();
        for _ in 0..150 {
            nested.push_str("{\"maps\":[");
        }
        nested.push_str("{}");
        for _ in 0..150 {
            nested.push_str("]}");
        }
        assert!(!fencer.update(&nested, &active));

        // 3. Check invalid JSON / binary garbage response
        assert!(!fencer.update("invalid json", &active));
        assert!(!fencer.update("\u{FFFD}\u{FFFD}\u{FFFD}", &active));
    }

    #[test]
    fn test_fencer_consecutive_failures_overflow() {
        let mut fencer = Fencer::new(3, HashSet::new());
        fencer.consecutive_failures = u64::MAX;

        // Trigger one failure cycle to verify it saturates instead of overflowing/wrapping
        let active = vec!["mpathb".to_string()].into_iter().collect();
        let maps = vec![MultipathMap {
            name: "mpathb".to_string(),
            uuid: "368f".to_string(),
            vend: None,
            prod: None,
            path_groups: Some(vec![PathGroup {
                dm_st: Some("failed".to_string()),
                paths: Some(vec![MpathPath {
                    dm_st: Some("failed".to_string()),
                }]),
            }]),
        }];
        fencer.update_with_maps(&maps, &active);
        assert_eq!(fencer.consecutive_failures(), u64::MAX);
    }

    #[test]
    fn test_fencer_previous_map_states_pruning() {
        let mut fencer = Fencer::new(3, HashSet::new());
        let mut active = vec!["mpatha".to_string(), "mpathb".to_string()]
            .into_iter()
            .collect();

        let maps = vec![
            MultipathMap {
                name: "mpatha".to_string(),
                uuid: "uuid-a".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
            MultipathMap {
                name: "mpathb".to_string(),
                uuid: "uuid-b".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
        ];

        // First update: should have both mpatha and mpathb in previous_map_states
        fencer.update_with_maps(&maps, &active);
        assert!(fencer.previous_map_states.contains_key("mpatha"));
        assert!(fencer.previous_map_states.contains_key("mpathb"));

        // Second update: remove mpathb from active_luns, it should be pruned from previous_map_states
        active.remove("mpathb");
        fencer.update_with_maps(&maps, &active);
        assert!(fencer.previous_map_states.contains_key("mpatha"));
        assert!(!fencer.previous_map_states.contains_key("mpathb"));
    }

    #[test]
    fn test_fencing_scenario_spanned_vg() {
        let active_luns: HashSet<String> = vec!["mpatha+mpathb".to_string()].into_iter().collect();

        // 1. Both mpatha and mpathb are alive -> expect no failure, no fencing
        let steps_both_healthy = vec![ScenarioStep {
            multipath_file: "all_active_running.json",
            active_luns: active_luns.clone(),
            expected_failures: 0,
            expected_fencing: false,
        }];
        run_scenario(/*max_failures*/ 3, &[], &steps_both_healthy);

        // 2. mpatha fails, mpathb remains healthy -> should trigger fencing since one of the involved multipath devices fails
        let steps_one_failed = vec![
            ScenarioStep {
                multipath_file: "mpatha_active_mpathb_failed.json",
                active_luns: active_luns.clone(),
                expected_failures: 1,
                expected_fencing: false,
            },
            ScenarioStep {
                multipath_file: "mpatha_active_mpathb_failed.json",
                active_luns: active_luns.clone(),
                expected_failures: 2,
                expected_fencing: false,
            },
            ScenarioStep {
                multipath_file: "mpatha_active_mpathb_failed.json",
                active_luns: active_luns.clone(),
                expected_failures: 3,
                expected_fencing: true,
            },
        ];
        run_scenario(/*max_failures*/ 3, &[], &steps_one_failed);
    }

    #[test]
    fn test_fencing_spanned_vg_in_memory() {
        let mut fencer = Fencer::new(3, HashSet::new());
        let active: HashSet<String> = vec!["mpatha+mpathb".to_string(), "mpathc".to_string()]
            .into_iter()
            .collect();

        // Step 1: All healthy
        let maps_all_healthy = vec![
            MultipathMap {
                name: "mpatha".to_string(),
                uuid: "uuid-a".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
            MultipathMap {
                name: "mpathb".to_string(),
                uuid: "uuid-b".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
            MultipathMap {
                name: "mpathc".to_string(),
                uuid: "uuid-c".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
        ];

        let triggered = fencer.update_with_maps(&maps_all_healthy, &active);
        assert!(!triggered);
        assert_eq!(fencer.consecutive_failures(), 0);

        // Step 2: One member of spanned group fails (mpatha is dead)
        let maps_mpatha_failed = vec![
            MultipathMap {
                name: "mpatha".to_string(),
                uuid: "uuid-a".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("failed".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("failed".to_string()),
                    }]),
                }]),
            },
            MultipathMap {
                name: "mpathb".to_string(),
                uuid: "uuid-b".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
            MultipathMap {
                name: "mpathc".to_string(),
                uuid: "uuid-c".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
        ];

        let triggered = fencer.update_with_maps(&maps_mpatha_failed, &active);
        assert!(!triggered);
        assert_eq!(fencer.consecutive_failures(), 1);

        // Step 3: Sustained failure for 3 cycles -> fence!
        fencer.update_with_maps(&maps_mpatha_failed, &active);
        let triggered = fencer.update_with_maps(&maps_mpatha_failed, &active);
        assert!(triggered);
        assert_eq!(fencer.consecutive_failures(), 3);
    }

    #[test]
    fn test_fencing_spanned_vg_mix_single_failed() {
        let mut fencer = Fencer::new(3, HashSet::new());
        let active: HashSet<String> = vec!["mpatha+mpathb".to_string(), "mpathc".to_string()]
            .into_iter()
            .collect();

        // mpathc fails, but spanned group mpatha+mpathb remains healthy.
        // We should NOT trigger fencing because not all single LUNs/groups are dead.
        let maps_mpathc_failed = vec![
            MultipathMap {
                name: "mpatha".to_string(),
                uuid: "uuid-a".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
            MultipathMap {
                name: "mpathb".to_string(),
                uuid: "uuid-b".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("active".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("active".to_string()),
                    }]),
                }]),
            },
            MultipathMap {
                name: "mpathc".to_string(),
                uuid: "uuid-c".to_string(),
                vend: None,
                prod: None,
                path_groups: Some(vec![PathGroup {
                    dm_st: Some("failed".to_string()),
                    paths: Some(vec![MpathPath {
                        dm_st: Some("failed".to_string()),
                    }]),
                }]),
            },
        ];

        let triggered = fencer.update_with_maps(&maps_mpathc_failed, &active);
        assert!(!triggered);
        assert_eq!(fencer.consecutive_failures(), 0);
    }
}
