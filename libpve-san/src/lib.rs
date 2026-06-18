//! Library for retrieving SAN/FC storage information from Proxmox VE hosts.
//!
//! This library provides functionality to:
//! - Query running VMs on a Proxmox host using pvesh CLI
//! - Retrieve disk configurations for each VM
//! - Discover underlying device-mapper and block devices using lsblk
//! - Return structured data in a thread-safe manner

use lsblk::BlockDevice;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::{Command, Stdio};
use thiserror::Error;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Configuration for the PVE SAN client
#[derive(Debug, Clone)]
pub struct PveSanConfig {
    /// The node name to query (local node name on Proxmox host)
    pub node: String,
}

impl Default for PveSanConfig {
    fn default() -> Self {
        Self {
            node: String::new(),
        }
    }
}

/// Main client for retrieving SAN information
pub struct PveSanClient {
    config: PveSanConfig,
}

impl PveSanClient {
    /// Creates a new PveSanClient with the given configuration.
    pub fn new(config: PveSanConfig) -> PveSanResult<Self> {
        if config.node.is_empty() {
            return Err(PveSanError::NoNodeError);
        }

        Ok(Self { config })
    }

    /// Retrieves information about all running VMs and their disks.
    pub async fn get_san_storage_info(&self) -> PveSanResult<SanStorageInfo> {
        let node = self.config.node.clone();
        let vms = self.list_running_vms().await?;

        let mut vm_infos = Vec::new();
        for vmid in vms {
            match self.get_vm_info(vmid).await {
                Ok(vm_info) => vm_infos.push(vm_info),
                Err(e) => {
                    eprintln!("Warning: Failed to get info for VM {}: {}", vmid, e);
                    continue;
                }
            }
        }

        let block_devices = self.get_block_devices()?;

        Ok(SanStorageInfo {
            node,
            vms: vm_infos,
            block_devices: Some(block_devices),
        })
    }

    async fn list_running_vms(&self) -> PveSanResult<Vec<u64>> {
        let node = &self.config.node;
        let path = format!("/nodes/{}/qemu", node);

        let json_output = self.run_pvesh_ls(&path).await?;

        let data: Vec<serde_json::Value> = serde_json::from_str(&json_output)
            .map_err(|e| PveSanError::JsonParseError(e.to_string()))?;

        let mut vms = Vec::new();
        for item in data {
            let vmid = item["vmid"]
                .as_u64()
                .ok_or_else(|| PveSanError::ListVmError("VMID is missing or not a number".to_string()))?;
            let status = item["status"].as_str().unwrap_or("unknown");

            if status == "running" {
                vms.push(vmid);
            }
        }

        Ok(vms)
    }

    async fn get_vm_info(&self, vmid: u64) -> PveSanResult<VmInfo> {
        let node = &self.config.node;
        let path = format!("/nodes/{}/qemu/{}/config", node, vmid);

        // Use pvesh get to retrieve the VM config as a string
        let config_text = self.run_pvesh_get(&path).await?;

        let config_map = self.parse_vm_config(&config_text)?;

        let name = config_map
            .get("name")
            .cloned()
            .unwrap_or_else(|| format!("vm-{}", vmid));
        let status = config_map
            .get("status")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        let disks = self.extract_disks(&config_map)?;

        Ok(VmInfo {
            vmid,
            name,
            status,
            disks,
        })
    }

    fn parse_vm_config(&self, config_text: &str) -> PveSanResult<HashMap<String, String>> {
        let mut config_map = HashMap::new();

        for line in config_text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
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
                if key.starts_with(prefix) && key.len() > prefix.len() {
                    let index_str = &key[prefix.len()..];
                    if let Ok(index) = index_str.parse::<u32>() {
                        let device_id = format!("{}{}", prefix, index);
                        let (storage, metadata) = self.parse_disk_value(value)?;

                        disks.push(VmDisk {
                            device_id,
                            storage,
                            device_path: None,
                            device_mapper_name: None,
                            size_bytes: metadata.get("size").and_then(|s| parse_size(s)),
                            metadata: Some(metadata),
                        });
                    }
                }
            }
        }

        Ok(disks)
    }

    fn parse_disk_value(&self, value: &str) -> PveSanResult<(String, HashMap<String, String>)> {
        let storage: String;
        let mut metadata = HashMap::new();

        let parts: Vec<&str> = value.split(',').collect();

        if parts.is_empty() {
            return Err(PveSanError::ConfigParseError("Empty disk value".to_string()));
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
        let devices = BlockDevice::list()
            .map_err(|e| PveSanError::LsblkError(e.to_string()))?;

        Ok(devices.into_iter().map(|device| self.convert_block_device(&device)).collect())
    }

    fn convert_block_device(&self, device: &BlockDevice) -> BlockDeviceInfo {
        let name = device.name.clone();
        let path = device.fullname.to_string_lossy().to_string();

        let device_type = if device.partuuid.is_some() || device.partlabel.is_some() {
            "part".to_string()
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

        BlockDeviceInfo {
            name,
            path,
            device_type,
            size: 0,
            dm_name,
            parent: None,
            children: Vec::new(),
            uuid: device.uuid.clone(),
            model: None,
            serial: device.id.clone(),
            mount_point: None,
        }
    }

    /// Run pvesh ls command to list resources at the given path
    async fn run_pvesh_ls(&self, path: &str) -> PveSanResult<String> {
        self.run_pvesh(&["ls", path, "--output-format", "json"]).await
    }

    /// Run pvesh get command to retrieve a specific resource
    async fn run_pvesh_get(&self, path: &str) -> PveSanResult<String> {
        self.run_pvesh(&["get", path, "--output-format", "json"]).await
    }

    /// Execute a pvesh command and return its stdout as a string
    async fn run_pvesh(&self, args: &[&str]) -> PveSanResult<String> {
        // Check if pvesh is available
        if Command::new("pvesh").arg("--version").output().is_err() {
            return Err(PveSanError::PveshNotFound);
        }

        let output = Command::new("pvesh")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| {
                PveSanError::PveshError(format!("Failed to spawn pvesh: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PveSanError::PveshError(format!(
                "pvesh command failed with status {}: {}",
                output.status,
                stderr
            )));
        }

        String::from_utf8(output.stdout)
            .map_err(|e| PveSanError::PveshError(format!("Invalid UTF-8 output: {}", e)))
    }

    pub fn config(&self) -> &PveSanConfig {
        &self.config
    }
}

fn parse_size(size_str: &str) -> Option<u64> {
    let size_str = size_str.trim().to_uppercase();
    
    if size_str.ends_with("K") || size_str.ends_with("KB") {
        let num = size_str.trim_end_matches(|c| c == 'K' || c == 'B');
        num.parse::<u64>().ok().map(|n| n * 1024)
    } else if size_str.ends_with("M") || size_str.ends_with("MB") {
        let num = size_str.trim_end_matches(|c| c == 'M' || c == 'B');
        num.parse::<u64>().ok().map(|n| n * 1024 * 1024)
    } else if size_str.ends_with("G") || size_str.ends_with("GB") {
        let num = size_str.trim_end_matches(|c| c == 'G' || c == 'B');
        num.parse::<u64>().ok().map(|n| n * 1024 * 1024 * 1024)
    } else if size_str.ends_with("T") || size_str.ends_with("TB") {
        let num = size_str.trim_end_matches(|c| c == 'T' || c == 'B');
        num.parse::<u64>().ok().map(|n| n * 1024 * 1024 * 1024 * 1024)
    } else {
        size_str.parse::<u64>().ok()
    }
}

pub async fn get_san_storage_info(
    node: &str,
) -> PveSanResult<SanStorageInfo> {
    let config = PveSanConfig {
        node: node.to_string(),
    };

    let client = PveSanClient::new(config)?;
    client.get_san_storage_info().await
}

pub fn get_san_storage_info_sync(
    node: &str,
) -> PveSanResult<SanStorageInfo> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| PveSanError::RuntimeError(e.to_string()))?;
    rt.block_on(async {
        get_san_storage_info(node).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("10G"), Some(10 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("10GB"), Some(10 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("1T"), Some(1024u64.pow(4)));
        assert_eq!(parse_size("100M"), Some(100 * 1024 * 1024));
        assert_eq!(parse_size("1024K"), Some(1024 * 1024));
        assert_eq!(parse_size("1048576"), Some(1048576));
        assert_eq!(parse_size("invalid"), None);
    }

    #[test]
    fn test_config_requires_node() {
        let config = PveSanConfig::default();
        let result = PveSanClient::new(config);
        assert!(matches!(result, Err(PveSanError::NoNodeError)));
    }

    #[test]
    fn test_parse_vm_config() {
        let client = PveSanClient::new(PveSanConfig { node: "test".to_string() }).unwrap();
        let config_text = "name: test-vm\nscsi0: local-lvm:vm-100-disk-0,size=10G\nstatus: running";
        let result = client.parse_vm_config(config_text);
        assert!(result.is_ok());
        let config_map = result.unwrap();
        assert_eq!(config_map.get("name"), Some(&"test-vm".to_string()));
        assert_eq!(config_map.get("scsi0"), Some(&"local-lvm:vm-100-disk-0,size=10G".to_string()));
        assert_eq!(config_map.get("status"), Some(&"running".to_string()));
    }

    #[test]
    fn test_parse_disk_value() {
        let client = PveSanClient::new(PveSanConfig { node: "test".to_string() }).unwrap();
        
        // Test with storage and size
        let (storage, metadata) = client.parse_disk_value("local-lvm:vm-100-disk-0,size=10G").unwrap();
        assert_eq!(storage, "local-lvm:vm-100-disk-0");
        assert_eq!(metadata.get("size"), Some(&"10G".to_string()));
        
        // Test without additional metadata
        let (storage, metadata) = client.parse_disk_value("local-lvm:vm-100-disk-0").unwrap();
        assert_eq!(storage, "local-lvm:vm-100-disk-0");
        assert!(metadata.is_empty());
        
        // Test with multiple metadata fields
        let (storage, metadata) = client.parse_disk_value("local-lvm:vm-100-disk-0,size=10G,backup=0").unwrap();
        assert_eq!(storage, "local-lvm:vm-100-disk-0");
        assert_eq!(metadata.get("size"), Some(&"10G".to_string()));
        assert_eq!(metadata.get("backup"), Some(&"0".to_string()));
    }

    #[test]
    fn test_extract_disks() {
        let client = PveSanClient::new(PveSanConfig { node: "test".to_string() }).unwrap();
        
        let mut config_map = HashMap::new();
        config_map.insert("name".to_string(), "test-vm".to_string());
        config_map.insert("scsi0".to_string(), "local-lvm:vm-100-disk-0,size=10G".to_string());
        config_map.insert("scsi1".to_string(), "local-lvm:vm-100-disk-1".to_string());
        config_map.insert("virtio0".to_string(), "local-lvm:vm-100-disk-2,size=20G,backup=0".to_string());
        config_map.insert("status".to_string(), "running".to_string());
        
        let disks = client.extract_disks(&config_map).unwrap();
        assert_eq!(disks.len(), 3);
        
        // Find disks by device_id since HashMap iteration order is not guaranteed
        let scsi0 = disks.iter().find(|d| d.device_id == "scsi0").unwrap();
        assert_eq!(scsi0.storage, "local-lvm:vm-100-disk-0");
        assert_eq!(scsi0.size_bytes, Some(10 * 1024 * 1024 * 1024));
        
        let scsi1 = disks.iter().find(|d| d.device_id == "scsi1").unwrap();
        assert_eq!(scsi1.storage, "local-lvm:vm-100-disk-1");
        assert_eq!(scsi1.size_bytes, None);
        
        let virtio0 = disks.iter().find(|d| d.device_id == "virtio0").unwrap();
        assert_eq!(virtio0.storage, "local-lvm:vm-100-disk-2");
        assert_eq!(virtio0.size_bytes, Some(20 * 1024 * 1024 * 1024));
    }
}
