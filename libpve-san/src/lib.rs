//! Library for retrieving SAN/FC storage information from Proxmox VE hosts.
//!
//! This library provides functionality to:
//! - Query running VMs on a Proxmox host using pvesh CLI
//! - Retrieve disk configurations for each VM
//! - Discover underlying device-mapper and block devices using lsblk
//! - Return structured data in a thread-safe manner
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

use log::warn;
use lsblk::BlockDevice;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use thiserror::Error;

pub(crate) mod sysfs;

/// Custom error type for the library
#[derive(Error, Debug)]
pub enum PveSanError {
    #[error("pvesh command failed: {0}")]
    PveshError(String),

    #[error("Failed to list VMs: {0}")]
    ListVmError(String),

    #[error("Failed to get VM config for VMID {0}: {1}")]
    VmConfigError(u64, String),

    #[error("Failed to parse VM config: {0}")]
    ConfigParseError(String),

    #[error("Failed to parse pvesh JSON output: {0}")]
    JsonParseError(String),

    #[error("Failed to list block devices: {0}")]
    LsblkError(String),

    #[error("No node name specified")]
    NoNodeError,

    #[error("Runtime error: {0}")]
    RuntimeError(String),

    #[error("pvesh command not found")]
    PveshNotFound,
}

/// Type alias for thread-safe result
pub type PveSanResult<T> = Result<T, PveSanError>;

/// Information about a VM's disk
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VmDisk {
    /// The disk identifier (e.g., "scsi0", "virtio0")
    pub device_id: String,

    /// The backing storage (e.g., "local-lvm:vm-100-disk-0")
    pub storage: String,

    /// The underlying device path if discovered (e.g., "/dev/dm-0")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_path: Option<String>,

    /// The device mapper name if applicable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_mapper_name: Option<String>,

    /// The size of the disk in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,

    /// Additional metadata from the config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Information about a VM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    /// The VM ID
    pub vmid: u64,

    /// The VM name
    pub name: String,

    /// The VM status
    pub status: String,

    /// List of disks configured for this VM
    pub disks: Vec<VmDisk>,
}

/// Complete SAN storage information for a host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanStorageInfo {
    /// The node name
    pub node: String,

    /// List of running VMs with their disk information
    pub vms: Vec<VmInfo>,

    /// List of block devices (from lsblk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_devices: Option<Vec<BlockDeviceInfo>>,

    /// Discovered multipath devices associated with each VMID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multipath_devices: Option<HashMap<String, Vec<String>>>,
}

/// Simplified block device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDeviceInfo {
    /// Device name (e.g., "sda", "dm-0")
    pub name: String,

    /// Full device path (e.g., "/dev/sda", "/dev/dm-0")
    pub path: String,

    /// Device type (e.g., "disk", "part", "lvm", "mpath")
    #[serde(rename = "type")]
    pub device_type: String,

    /// Size in bytes
    pub size: u64,

    /// Device mapper name if applicable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dm_name: Option<String>,

    /// Parent device name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,

    /// Children device names
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,

    /// UUID if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,

    /// Model information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Serial number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,

    /// Mount point
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_point: Option<String>,
}

/// Query mode for the PVE SAN client
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PveMode {
    /// Query using pvesh command
    Pvesh,
    /// Query local configuration files only (default)
    #[default]
    LocalFiles,
}

/// Configuration for the PVE SAN client
#[derive(Debug, Clone)]
pub struct PveSanConfig {
    /// The node name to query (local node name on Proxmox host)
    node: String,

    /// The pvesh command to use (default: "pvesh")
    pvesh_command: String,

    /// The query mode (default: PveMode::LocalFiles)
    mode: PveMode,
}

impl PveSanConfig {
    /// Creates a new PveSanConfig with the given node name and optional pvesh command.
    ///
    /// # Arguments
    ///
    /// * `node` - The node name to query (cannot be empty)
    /// * `pvesh_command` - The pvesh command to use (default: "pvesh")
    ///
    /// # Returns
    ///
    /// Returns the configuration if valid, or an error if the node name is empty.
    pub fn new(node: impl Into<String>, pvesh_command: Option<&str>) -> PveSanResult<Self> {
        let node = node.into();
        if node.is_empty() {
            return Err(PveSanError::NoNodeError);
        }

        let cmd = pvesh_command.unwrap_or("pvesh");
        if cmd.is_empty()
            || !cmd
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '-' || c == '_')
        {
            return Err(PveSanError::PveshError(format!(
                "Invalid pvesh command path: {cmd}"
            )));
        }

        Ok(Self {
            node,
            pvesh_command: cmd.to_string(),
            mode: PveMode::default(),
        })
    }

    /// Creates a new PveSanConfig with the given node name and default pvesh command.
    pub fn with_node(node: impl Into<String>) -> PveSanResult<Self> {
        Self::new(node, /*pvesh_command*/ None)
    }

    /// Returns the node name.
    pub fn node(&self) -> &str {
        &self.node
    }

    /// Returns the pvesh command.
    pub fn pvesh_command(&self) -> &str {
        &self.pvesh_command
    }

    /// Sets the query mode.
    pub fn with_mode(mut self, mode: PveMode) -> Self {
        self.mode = mode;
        self
    }

    /// Returns the query mode.
    pub fn mode(&self) -> PveMode {
        self.mode
    }
}

/// Main client for retrieving SAN information
pub struct PveSanClient {
    config: PveSanConfig,
}

impl PveSanClient {
    /// Creates a new PveSanClient with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration to use (must have a non-empty node name)
    ///
    /// # Returns
    ///
    /// Returns the client.
    pub fn new(config: PveSanConfig) -> Self {
        Self { config }
    }

    /// Creates a new PveSanClient with the given node name and default pvesh command.
    pub fn with_node(node: impl Into<String>) -> PveSanResult<Self> {
        let config = PveSanConfig::with_node(node)?;
        Ok(Self { config })
    }

    /// Creates a new PveSanClient with the given node name and custom pvesh command.
    pub fn with_node_and_pvesh(node: impl Into<String>, pvesh_command: &str) -> PveSanResult<Self> {
        let config = PveSanConfig::new(node, Some(pvesh_command))?;
        Ok(Self { config })
    }

    /// Retrieves information about all running VMs and their disks.
    #[tracing::instrument(skip(self))]
    pub async fn get_san_storage_info(&self) -> PveSanResult<SanStorageInfo> {
        let node = self.config.node.clone();
        let vms = self.list_running_vms().await?;

        // Retrieve lsblk tree to build a mapping from child devices to parent mpaths
        let mut mpath_map: HashMap<String, HashSet<String>> = HashMap::new();
        if std::env::var("PVE_SAN_TEST_DATA_DIR").is_ok()
            && std::env::var("PVE_SAN_SYS_PATH").is_err()
        {
            if let Some(json_content) = self.get_lsblk_json().await {
                if let Ok(lsblk_output) = serde_json::from_str::<LsblkOutput>(&json_content) {
                    if let Some(devices) = lsblk_output.blockdevices {
                        self.build_mpath_map(&devices, None, 0, &mut mpath_map);
                    }
                }
            }
        } else {
            let prefix = std::env::var("PVE_SAN_SYS_PATH").unwrap_or_else(|_| "/sys".to_string());
            let dev_names: Vec<String> = if std::env::var("PVE_SAN_SYS_PATH").is_ok() {
                let mut names = Vec::new();
                if let Ok(entries) = std::fs::read_dir(format!("{prefix}/block")) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with("dm-") {
                            names.push(name);
                        }
                    }
                }
                names
            } else {
                BlockDevice::list()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|d| d.name.starts_with("dm-"))
                    .map(|d| d.name)
                    .collect()
            };

            for dev_name in dev_names {
                let sys_path = format!("{prefix}/block/{dev_name}/dm/name");
                if let Ok(mapped_name) = std::fs::read_to_string(sys_path) {
                    let mapped_name = mapped_name.trim().to_string();
                    let mpaths = sysfs::find_multipaths_for_dm(&dev_name, &mut HashSet::new());
                    if !mpaths.is_empty() {
                        mpath_map.insert(mapped_name, mpaths.clone());
                        mpath_map.insert(dev_name.clone(), mpaths);
                    }
                }
            }
        }

        let mut vm_infos = Vec::new();
        let mut multipath_devices = HashMap::new();

        for (vmid, status) in vms {
            match self.get_vm_config(vmid).await {
                Ok(config_map) => {
                    let name = config_map
                        .get("name")
                        .cloned()
                        .unwrap_or_else(|| format!("vm-{vmid}"));

                    let mut disks = self.extract_disks(&config_map)?;
                    let mut vm_mpaths = HashSet::new();

                    for disk in &mut disks {
                        let dm_name = self.storage_to_dm_name(&disk.storage);
                        let dm_name_legacy = self.storage_to_dm_name_legacy(&disk.storage);
                        let mut matched_mpaths = None;

                        if let Some(mpaths) = mpath_map.get(&dm_name) {
                            matched_mpaths = Some(mpaths);
                        } else if let Some(mpaths) = mpath_map.get(&dm_name_legacy) {
                            matched_mpaths = Some(mpaths);
                        } else if let Some(mpaths) = mpath_map.get(&disk.storage) {
                            matched_mpaths = Some(mpaths);
                        } else if let Some(mpaths) = dm_name
                            .strip_prefix("/dev/mapper/")
                            .and_then(|name| mpath_map.get(name))
                        {
                            matched_mpaths = Some(mpaths);
                        } else if let Some(mpaths) = dm_name_legacy
                            .strip_prefix("/dev/mapper/")
                            .and_then(|name| mpath_map.get(name))
                        {
                            matched_mpaths = Some(mpaths);
                        }

                        if let Some(mpaths) = matched_mpaths {
                            let mut mpath_list: Vec<String> = mpaths.iter().cloned().collect();
                            mpath_list.sort();

                            let mapper_name = mpath_list.join(" / ");
                            let path_name = mpath_list
                                .iter()
                                .map(|name| format!("/dev/mapper/{name}"))
                                .collect::<Vec<_>>()
                                .join(" / ");

                            disk.device_mapper_name = Some(mapper_name);
                            disk.device_path = Some(path_name);

                            for mpath in mpaths {
                                vm_mpaths.insert(mpath.clone());
                            }
                        }
                    }

                    if !vm_mpaths.is_empty() {
                        let mut sorted_mpaths: Vec<String> = vm_mpaths.into_iter().collect();
                        sorted_mpaths.sort();
                        multipath_devices.insert(vmid.to_string(), sorted_mpaths);
                    }

                    vm_infos.push(VmInfo {
                        vmid,
                        name,
                        status,
                        disks,
                    });
                }
                Err(e) => {
                    warn!("Failed to get config for VM {vmid}: {e}");
                    continue;
                }
            }
        }

        let block_devices = self.get_block_devices()?;

        Ok(SanStorageInfo {
            node,
            vms: vm_infos,
            block_devices: Some(block_devices),
            multipath_devices: Some(multipath_devices),
        })
    }

    async fn get_lsblk_json(&self) -> Option<String> {
        if let Ok(test_data_dir) = std::env::var("PVE_SAN_TEST_DATA_DIR") {
            let path = std::path::Path::new(&test_data_dir).join("lsblk.json");
            std::fs::read_to_string(path).ok()
        } else {
            None
        }
    }

    fn build_mpath_map(
        &self,
        devices: &[LsblkDevice],
        current_mpath: Option<&str>,
        depth: u32,
        map: &mut HashMap<String, HashSet<String>>,
    ) {
        if depth > 32 {
            warn!("Exceeded maximum recursion depth of 32 in build_mpath_map");
            return;
        }
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
                self.build_mpath_map(children, next_mpath, depth + 1, map);
            }
        }
    }

    fn storage_to_dm_name(&self, storage: &str) -> String {
        if let Some((vg, lv)) = storage.split_once(':') {
            format!("{}-{}", vg.replace("-", "--"), lv.replace("-", "--"))
        } else {
            storage.to_string()
        }
    }

    fn storage_to_dm_name_legacy(&self, storage: &str) -> String {
        if let Some((vg, lv)) = storage.split_once(':') {
            format!("{vg}-{}", lv.replace("-", "--"))
        } else {
            storage.to_string()
        }
    }

    #[tracing::instrument(skip(self))]
    async fn list_running_vms(&self) -> PveSanResult<Vec<(u64, String)>> {
        match self.config.mode {
            PveMode::Pvesh => {
                let node = &self.config.node;
                let path = format!("/nodes/{node}/qemu");

                let json_output = self.run_pvesh_ls(&path).await?;

                let data: Vec<serde_json::Value> = serde_json::from_str(&json_output)
                    .map_err(|e| PveSanError::JsonParseError(e.to_string()))?;

                let mut vms = Vec::new();
                for item in data {
                    let vmid = item["vmid"]
                        .as_u64()
                        .or_else(|| item["vmid"].as_str().and_then(|s| s.parse::<u64>().ok()))
                        .or_else(|| item["subdir"].as_u64())
                        .or_else(|| item["subdir"].as_str().and_then(|s| s.parse::<u64>().ok()))
                        .ok_or_else(|| {
                            PveSanError::ListVmError("VMID is missing or not a number".to_string())
                        })?;
                    let status = item["status"].as_str().unwrap_or("unknown").to_string();

                    if status == "running" {
                        vms.push((vmid, status));
                    }
                }
                Ok(vms)
            }
            PveMode::LocalFiles => {
                let dir_path = if let Ok(test_dir) = std::env::var("PVE_SAN_TEST_DATA_DIR") {
                    std::path::Path::new(&test_dir)
                        .parent()
                        .map(|p| p.join("pve/local/qemu-server"))
                        .unwrap_or_else(|| std::path::PathBuf::from("/etc/pve/local/qemu-server"))
                } else {
                    std::path::PathBuf::from("/etc/pve/local/qemu-server")
                };

                let mut vms = Vec::new();
                if dir_path.exists() {
                    let entries = std::fs::read_dir(&dir_path).map_err(|e| {
                        let dir_path_str = dir_path.display();
                        PveSanError::ListVmError(format!(
                            "Failed to read config directory '{dir_path_str}': {e}"
                        ))
                    })?;

                    for entry in entries {
                        let entry = entry.map_err(|e| {
                            PveSanError::ListVmError(format!("Failed to read directory entry: {e}"))
                        })?;
                        let path = entry.path();
                        if path.is_file() && path.extension() == Some(std::ffi::OsStr::new("conf"))
                        {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                if let Ok(vmid) = stem.parse::<u64>() {
                                    vms.push((vmid, "running".to_string()));
                                }
                            }
                        }
                    }
                }
                vms.sort_by_key(|(vmid, _)| *vmid);
                Ok(vms)
            }
        }
    }

    // NOTE: `vmid` is currently a u64, preventing path traversal vulnerabilities.
    // If the API is ever changed to accept a string identifier, the input MUST be validated
    // as a pure numeric string to prevent path traversal via config file path construction.
    #[tracing::instrument(skip(self))]
    async fn get_vm_config(&self, vmid: u64) -> PveSanResult<HashMap<String, String>> {
        let local_path = if let Ok(test_dir) = std::env::var("PVE_SAN_TEST_DATA_DIR") {
            let path = std::path::Path::new(&test_dir)
                .parent()
                .map(|p| p.join("pve/local/qemu-server").join(format!("{vmid}.conf")))
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(format!("/etc/pve/local/qemu-server/{vmid}.conf"))
                });
            if path.exists() {
                path.to_string_lossy().to_string()
            } else {
                format!("/etc/pve/local/qemu-server/{vmid}.conf")
            }
        } else {
            format!("/etc/pve/local/qemu-server/{vmid}.conf")
        };

        // Try reading directly from pmxcfs config file first (optimization)
        if let Ok(config_text) = std::fs::read_to_string(&local_path) {
            return self.parse_vm_config(&config_text);
        }

        let node = &self.config.node;
        let path = format!("/nodes/{node}/qemu/{vmid}/config");

        // Use pvesh get to retrieve the VM config as a string
        let config_text = self.run_pvesh_get(&path).await?;

        self.parse_vm_config(&config_text)
    }

    fn parse_vm_config(&self, config_text: &str) -> PveSanResult<HashMap<String, String>> {
        let mut config_map = HashMap::new();

        // Try parsing as JSON first
        if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(config_text) {
            if let Some(obj) = json_value.as_object() {
                for (key, value) in obj {
                    // Convert JSON value to string
                    let value_str = match value {
                        serde_json::Value::Null => "".to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                            serde_json::to_string(value).unwrap_or_else(|_| {
                                format!("<unrepresentable JSON value: {value:?}>")
                            })
                        }
                    };
                    config_map.insert(key.clone(), value_str);
                }
                return Ok(config_map);
            }
        }

        // Fall back to key: value format
        for line in config_text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.starts_with('[') {
                break; // Stop parsing when we hit a section header ([PENDING] or snapshot)
            }

            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                config_map.insert(key, value);
            }
        }

        Ok(config_map)
    }

    fn extract_disks(&self, config_map: &HashMap<String, String>) -> PveSanResult<Vec<VmDisk>> {
        let mut disks = Vec::new();
        let disk_prefixes = ["scsi", "virtio", "sata", "ide", "efidisk"];

        for (key, value) in config_map {
            for prefix in &disk_prefixes {
                if let Some(index_str) = key.strip_prefix(prefix) {
                    if index_str.is_empty() {
                        tracing::warn!("Unexpected disk key '{key}' without numeric index");
                        continue;
                    }
                    match index_str.parse::<u32>() {
                        Ok(index) => {
                            if index > 99 {
                                tracing::warn!("Ignoring unexpected disk key '{key}' with index {index} exceeding limit of 99");
                                continue;
                            }
                            let device_id = format!("{}{}", prefix, index);
                            let (storage, metadata) = self.parse_disk_value(value)?;

                            disks.push(VmDisk {
                                device_id,
                                storage,
                                device_path: None,
                                device_mapper_name: None,
                                size_bytes: metadata.get("size").and_then(|s| Self::parse_size(s)),
                                metadata: Some(metadata),
                            });
                        }
                        Err(_) => {
                            tracing::warn!(
                                "Unexpected disk key '{key}' with non-numeric suffix '{index_str}'"
                            );
                        }
                    }
                }
            }
        }

        // Sort disks by device_id to ensure deterministic order
        disks.sort_by(|a, b| a.device_id.cmp(&b.device_id));

        Ok(disks)
    }

    fn parse_disk_value(&self, value: &str) -> PveSanResult<(String, HashMap<String, String>)> {
        let storage: String;
        let mut metadata = HashMap::new();

        let parts: Vec<&str> = value.split(',').collect();

        if parts.is_empty() {
            return Err(PveSanError::ConfigParseError(
                "Empty disk value".to_string(),
            ));
        }

        let storage_part = parts[0];
        if let Some((storage_name, volume)) = storage_part.split_once(':') {
            storage = format!("{}:{}", storage_name, volume);
        } else {
            storage = storage_part.to_string();
        }

        for part in &parts[1..] {
            if let Some((key, val)) = part.split_once('=') {
                metadata.insert(key.to_string(), val.to_string());
            }
        }

        Ok((storage, metadata))
    }

    fn get_block_devices(&self) -> PveSanResult<Vec<BlockDeviceInfo>> {
        let devices = BlockDevice::list().map_err(|e| PveSanError::LsblkError(e.to_string()))?;

        Ok(devices
            .into_iter()
            .map(|device| self.convert_block_device(&device))
            .collect())
    }

    fn convert_block_device(&self, device: &BlockDevice) -> BlockDeviceInfo {
        let name = device.name.clone();
        let path = device.fullname.to_string_lossy().to_string();

        let device_type = if device.partuuid.is_some() || device.partlabel.is_some() {
            "part".to_string()
        } else if name.starts_with("dm-") {
            "mpath".to_string()
        } else if device.uuid.is_some() {
            "disk".to_string()
        } else {
            "unknown".to_string()
        };

        let dm_name = if name.starts_with("dm-") {
            Some(name.clone())
        } else {
            None
        };

        let size = device
            .capacity()
            .ok()
            .flatten()
            .map(|c| c * 512)
            .unwrap_or(0);

        BlockDeviceInfo {
            name,
            path,
            device_type,
            size,
            dm_name,
            parent: None,
            children: Vec::new(),
            uuid: device.uuid.clone(),
            model: None,
            serial: device.id.clone(), // Using id as serial number fallback
            mount_point: None,
        }
    }

    /// Run pvesh ls command to list resources at the given path
    #[tracing::instrument(skip(self))]
    async fn run_pvesh_ls(&self, path: &str) -> PveSanResult<String> {
        self.run_pvesh(&["ls", path, "--output-format", "json"])
            .await
    }

    /// Run pvesh get command to retrieve a specific resource
    #[tracing::instrument(skip(self))]
    async fn run_pvesh_get(&self, path: &str) -> PveSanResult<String> {
        self.run_pvesh(&["get", path, "--output-format", "json"])
            .await
    }

    /// Execute a pvesh command and return its stdout as a string
    #[tracing::instrument(skip(self))]
    async fn run_pvesh(&self, args: &[&str]) -> PveSanResult<String> {
        let output = Command::new(&self.config.pvesh_command)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| {
                let cmd = &self.config.pvesh_command;
                PveSanError::PveshError(format!("Failed to spawn {cmd}: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let status = output.status;
            return Err(PveSanError::PveshError(format!(
                "pvesh command failed with status {status}: {stderr}"
            )));
        }

        String::from_utf8(output.stdout)
            .map_err(|e| PveSanError::PveshError(format!("Invalid UTF-8 output: {e}")))
    }

    fn parse_size(size_str: &str) -> Option<u64> {
        let size_str = size_str.trim().to_uppercase();
        const MAX_SIZE_BYTES: u64 = 100 * 1024 * 1024 * 1024 * 1024 * 1024; // 100 PB

        // Try to parse the number part, handling both integer and decimal values
        // Returns the numeric value in bytes
        fn parse_and_convert(num_str: &str, multiplier: u64) -> Option<u64> {
            // Try integer parse first
            if let Ok(n) = num_str.parse::<u64>() {
                let bytes = n.saturating_mul(multiplier);
                if bytes > MAX_SIZE_BYTES {
                    return None;
                }
                return Some(bytes);
            }
            // Try decimal parse
            if let Ok(n) = num_str.parse::<f64>() {
                if n < 0.0 || n.is_nan() || n.is_infinite() {
                    return None;
                }
                let bytes_f = n * (multiplier as f64);
                if bytes_f > MAX_SIZE_BYTES as f64 {
                    return None;
                }
                return Some(bytes_f as u64);
            }
            None
        }

        let parsed = if size_str.ends_with("K") || size_str.ends_with("KB") {
            let num = size_str.trim_end_matches(['K', 'B']);
            parse_and_convert(num, 1024)
        } else if size_str.ends_with("M") || size_str.ends_with("MB") {
            let num = size_str.trim_end_matches(['M', 'B']);
            parse_and_convert(num, 1024 * 1024)
        } else if size_str.ends_with("G") || size_str.ends_with("GB") {
            let num = size_str.trim_end_matches(['G', 'B']);
            parse_and_convert(num, 1024 * 1024 * 1024)
        } else if size_str.ends_with("T") || size_str.ends_with("TB") {
            let num = size_str.trim_end_matches(['T', 'B']);
            parse_and_convert(num, 1024 * 1024 * 1024 * 1024)
        } else {
            // Try to parse as plain number (bytes)
            if let Ok(n) = size_str.parse::<u64>() {
                Some(n)
            } else if let Ok(n) = size_str.parse::<f64>() {
                if n < 0.0 || n.is_nan() || n.is_infinite() {
                    None
                } else {
                    Some(n as u64)
                }
            } else {
                None
            }
        };

        match parsed {
            Some(bytes) if bytes <= MAX_SIZE_BYTES => Some(bytes),
            _ => None,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
struct LsblkDevice {
    name: String,
    #[serde(rename = "type")]
    device_type: String,
    children: Option<Vec<LsblkDevice>>,
}

#[derive(Deserialize, Debug, Clone)]
struct LsblkOutput {
    blockdevices: Option<Vec<LsblkDevice>>,
}

#[tracing::instrument]
pub async fn get_san_storage_info(node: &str) -> PveSanResult<SanStorageInfo> {
    let config = PveSanConfig::with_node(node)?;
    let client = PveSanClient::new(config);
    client.get_san_storage_info().await
}

/// Get SAN storage info with a custom pvesh command (for testing)
#[cfg_attr(not(test), allow(dead_code))]
#[tracing::instrument]
pub async fn get_san_storage_info_with_pvesh(
    node: &str,
    pvesh_command: &str,
) -> PveSanResult<SanStorageInfo> {
    let config = PveSanConfig::new(node, Some(pvesh_command))?.with_mode(PveMode::Pvesh);
    let client = PveSanClient::new(config);
    client.get_san_storage_info().await
}

pub fn get_san_storage_info_sync(node: &str) -> PveSanResult<SanStorageInfo> {
    get_san_storage_info_sync_with_mode(node, PveMode::LocalFiles)
}

/// Get SAN storage info synchronously with a specific query mode.
pub fn get_san_storage_info_sync_with_mode(
    node: &str,
    mode: PveMode,
) -> PveSanResult<SanStorageInfo> {
    let config = PveSanConfig::with_node(node)?.with_mode(mode);
    let client = PveSanClient::new(config);
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async { client.get_san_storage_info().await })
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| PveSanError::RuntimeError(e.to_string()))?;
        rt.block_on(async { client.get_san_storage_info().await })
    }
}

/// Get SAN storage info synchronously with a custom pvesh command (for testing)
#[cfg_attr(not(test), allow(dead_code))]
pub fn get_san_storage_info_sync_with_pvesh(
    node: &str,
    pvesh_command: &str,
) -> PveSanResult<SanStorageInfo> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(async { get_san_storage_info_with_pvesh(node, pvesh_command).await })
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| PveSanError::RuntimeError(e.to_string()))?;
        rt.block_on(async { get_san_storage_info_with_pvesh(node, pvesh_command).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(
            PveSanClient::parse_size("10G"),
            Some(10 * 1024 * 1024 * 1024)
        );
        assert_eq!(
            PveSanClient::parse_size("10GB"),
            Some(10 * 1024 * 1024 * 1024)
        );
        assert_eq!(PveSanClient::parse_size("1T"), Some(1024u64.pow(4)));
        assert_eq!(PveSanClient::parse_size("100M"), Some(100 * 1024 * 1024));
        assert_eq!(PveSanClient::parse_size("1024K"), Some(1024 * 1024));
        assert_eq!(PveSanClient::parse_size("1048576"), Some(1048576));
        assert_eq!(PveSanClient::parse_size("invalid"), None);

        // Test extreme inputs & overflow cases
        assert_eq!(PveSanClient::parse_size("18446744073709551615G"), None);
        assert_eq!(PveSanClient::parse_size("99999999999999999999G"), None);
        assert_eq!(PveSanClient::parse_size("101PB"), None);

        // Test negative-like values
        assert_eq!(PveSanClient::parse_size("-1G"), None);
        assert_eq!(PveSanClient::parse_size("-100"), None);
    }

    #[test]
    fn test_config_requires_node() {
        // With private fields, config creation fails for empty node
        let config_result = PveSanConfig::with_node("");
        assert!(matches!(config_result, Err(PveSanError::NoNodeError)));
    }

    #[test]
    fn test_parse_vm_config() {
        let config = PveSanConfig::with_node("test").unwrap();
        let client = PveSanClient::new(config);
        let config_text = "name: test-vm\nscsi0: local-lvm:vm-100-disk-0,size=10G\nstatus: running\n\n[PENDING]\nscsi1: local-lvm:vm-100-disk-1,size=20G";
        let result = client.parse_vm_config(config_text);
        assert!(result.is_ok());
        let config_map = result.unwrap();
        assert_eq!(config_map.get("name"), Some(&"test-vm".to_string()));
        assert_eq!(
            config_map.get("scsi0"),
            Some(&"local-lvm:vm-100-disk-0,size=10G".to_string())
        );
        assert_eq!(config_map.get("status"), Some(&"running".to_string()));
        assert_eq!(config_map.get("scsi1"), None); // scsi1 is in PENDING and should be ignored
    }

    #[test]
    fn test_parse_vm_config_json() {
        let config = PveSanConfig::with_node("test").unwrap();
        let client = PveSanClient::new(config);
        let config_text = r#"{"name": "test-vm", "scsi0": "local-lvm:vm-100-disk-0,size=10G", "status": "running"}"#;
        let result = client.parse_vm_config(config_text);
        assert!(result.is_ok());
        let config_map = result.unwrap();
        assert_eq!(config_map.get("name"), Some(&"test-vm".to_string()));
        assert_eq!(
            config_map.get("scsi0"),
            Some(&"local-lvm:vm-100-disk-0,size=10G".to_string())
        );
        assert_eq!(config_map.get("status"), Some(&"running".to_string()));
    }

    #[test]
    fn test_parse_disk_value() {
        let config = PveSanConfig::with_node("test").unwrap();
        let client = PveSanClient::new(config);

        // Test with storage and size
        let result = client
            .parse_disk_value("local-lvm:vm-100-disk-0,size=10G")
            .unwrap();
        let expected = (
            "local-lvm:vm-100-disk-0".to_string(),
            HashMap::from([("size".to_string(), "10G".to_string())]),
        );
        assert_eq!(result, expected);

        // Test without additional metadata
        let result = client.parse_disk_value("local-lvm:vm-100-disk-0").unwrap();
        let expected = ("local-lvm:vm-100-disk-0".to_string(), HashMap::new());
        assert_eq!(result, expected);

        // Test with multiple metadata fields
        let result = client
            .parse_disk_value("local-lvm:vm-100-disk-0,size=10G,backup=0")
            .unwrap();
        let expected = (
            "local-lvm:vm-100-disk-0".to_string(),
            HashMap::from([
                ("size".to_string(), "10G".to_string()),
                ("backup".to_string(), "0".to_string()),
            ]),
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn test_extract_disks() {
        let config = PveSanConfig::with_node("test").unwrap();
        let client = PveSanClient::new(config);

        let mut config_map = HashMap::new();
        config_map.insert("name".to_string(), "test-vm".to_string());
        config_map.insert(
            "scsi0".to_string(),
            "local-lvm:vm-100-disk-0,size=10G".to_string(),
        );
        config_map.insert("scsi1".to_string(), "local-lvm:vm-100-disk-1".to_string());
        config_map.insert(
            "virtio0".to_string(),
            "local-lvm:vm-100-disk-2,size=20G,backup=0".to_string(),
        );
        config_map.insert("status".to_string(), "running".to_string());
        // Insert invalid/excessive disk keys to test sanitization and warning paths
        config_map.insert(
            "scsi999".to_string(),
            "local-lvm:vm-100-disk-excessive,size=10G".to_string(),
        );
        config_map.insert(
            "scsi".to_string(),
            "local-lvm:vm-100-disk-missing-index,size=10G".to_string(),
        );
        config_map.insert(
            "scsiaux".to_string(),
            "local-lvm:vm-100-disk-invalid-index,size=10G".to_string(),
        );

        let mut disks = client.extract_disks(&config_map).unwrap();
        assert_eq!(disks.len(), 3);

        let mut expected_disks = vec![
            VmDisk {
                device_id: "scsi0".to_string(),
                storage: "local-lvm:vm-100-disk-0".to_string(),
                device_path: None,
                device_mapper_name: None,
                size_bytes: Some(10 * 1024 * 1024 * 1024),
                metadata: Some(HashMap::from([("size".to_string(), "10G".to_string())])),
            },
            VmDisk {
                device_id: "scsi1".to_string(),
                storage: "local-lvm:vm-100-disk-1".to_string(),
                device_path: None,
                device_mapper_name: None,
                size_bytes: None,
                metadata: Some(HashMap::new()),
            },
            VmDisk {
                device_id: "virtio0".to_string(),
                storage: "local-lvm:vm-100-disk-2".to_string(),
                device_path: None,
                device_mapper_name: None,
                size_bytes: Some(20 * 1024 * 1024 * 1024),
                metadata: Some(HashMap::from([
                    ("size".to_string(), "20G".to_string()),
                    ("backup".to_string(), "0".to_string()),
                ])),
            },
        ];

        disks.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        expected_disks.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        assert_eq!(disks, expected_disks);
    }

    #[test]
    fn test_parse_vms_from_qemu_json() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let test_data_path = manifest_dir
            .parent()
            .unwrap()
            .join("test-data/pvesh/get_nodes/pve001_qemu.json");
        let content = std::fs::read_to_string(test_data_path).unwrap();
        let data: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();

        let mut extracted_vmids = Vec::new();
        for item in data {
            let vmid = item["vmid"]
                .as_u64()
                .or_else(|| item["vmid"].as_str().and_then(|s| s.parse::<u64>().ok()))
                .or_else(|| item["subdir"].as_u64())
                .or_else(|| item["subdir"].as_str().and_then(|s| s.parse::<u64>().ok()))
                .unwrap();
            let status = item["status"].as_str().unwrap_or("unknown").to_string();
            if status == "running" {
                extracted_vmids.push(vmid);
            }
        }

        assert_eq!(
            extracted_vmids,
            vec![132, 145, 141, 105, 144, 147, 131, 122, 114, 126, 116, 104, 133, 140, 117]
        );
    }

    #[test]
    fn test_robust_vmid_parsing() {
        let json_input = r#"[
            {"vmid": 123, "status": "running"},
            {"vmid": "456", "status": "running"},
            {"subdir": 789, "status": "running"},
            {"subdir": "999", "status": "running"}
        ]"#;
        let data: Vec<serde_json::Value> = serde_json::from_str(json_input).unwrap();
        let mut extracted = Vec::new();
        for item in data {
            let vmid = item["vmid"]
                .as_u64()
                .or_else(|| item["vmid"].as_str().and_then(|s| s.parse::<u64>().ok()))
                .or_else(|| item["subdir"].as_u64())
                .or_else(|| item["subdir"].as_str().and_then(|s| s.parse::<u64>().ok()))
                .unwrap();
            extracted.push(vmid);
        }
        assert_eq!(extracted, vec![123, 456, 789, 999]);
    }

    #[tokio::test]
    async fn test_list_running_vms_local_files() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let test_data_path = manifest_dir.parent().unwrap().join("test-data/pvesh");

        struct EnvGuard {
            saved: Option<String>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                if let Some(val) = &self.saved {
                    std::env::set_var("PVE_SAN_TEST_DATA_DIR", val);
                } else {
                    std::env::remove_var("PVE_SAN_TEST_DATA_DIR");
                }
            }
        }
        let saved = std::env::var("PVE_SAN_TEST_DATA_DIR").ok();
        let _guard = EnvGuard { saved };
        std::env::set_var("PVE_SAN_TEST_DATA_DIR", &test_data_path);

        let config = PveSanConfig::with_node("pve001").unwrap();
        assert_eq!(config.mode(), PveMode::LocalFiles);

        let client = PveSanClient::new(config);
        let vms = client.list_running_vms().await.unwrap();

        let mut expected = vec![
            104, 105, 114, 116, 117, 122, 126, 130, 131, 132, 133, 140, 141, 144, 145, 147, 999,
        ];
        expected.sort_unstable();

        let mut vmids: Vec<u64> = vms.iter().map(|(vmid, _)| *vmid).collect();
        vmids.sort_unstable();

        assert_eq!(vmids, expected);
    }

    #[test]
    fn test_parse_vm_999_all_disk_types() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let test_data_path = manifest_dir
            .parent()
            .unwrap()
            .join("test-data/pve/local/qemu-server/999.conf");
        let content = std::fs::read_to_string(test_data_path).unwrap();

        let config = PveSanConfig::with_node("test").unwrap();
        let client = PveSanClient::new(config);

        let config_map = client.parse_vm_config(&content).unwrap();
        let disks = client.extract_disks(&config_map).unwrap();

        // 999.conf has efidisk0, efidisk5, sata1, ide2, scsi3, virtio4 (6 disks in total)
        assert_eq!(disks.len(), 6);

        let device_ids: Vec<String> = disks.iter().map(|d| d.device_id.clone()).collect();
        assert!(device_ids.contains(&"efidisk0".to_string()));
        assert!(device_ids.contains(&"sata1".to_string()));
        assert!(device_ids.contains(&"ide2".to_string()));
        assert!(device_ids.contains(&"scsi3".to_string()));
        assert!(device_ids.contains(&"virtio4".to_string()));
        assert!(device_ids.contains(&"efidisk5".to_string()));
    }
}
