# PVE SAN Fenced - Implementation Plan

This document describes the implementation of the tools and libraries in the pve-san-fenced project in sufficient detail that an agent can either create or verify the code based on the description.

## Table of Contents

1. [Project Overview](#project-overview)
2. [Workspace Structure](#workspace-structure)
3. [Libraries](#libraries)
   - [3.1 libmultipath](#31-libmultipath)
   - [3.2 libpve-san](#32-libpve-san)
4. [Tools](#tools)
   - [4.1 mpath-query](#41-mpath-query)
   - [4.2 mpath-mockd](#42-mpath-mockd)
   - [4.3 pve-san-query](#43-pve-san-query)
   - [4.4 pvesh-mock](#44-pvesh-mock)
5. [Test Data](#test-data)
6. [PVE SAN Fencing Daemon (pve-san-fenced)](#pve-san-fencing-daemon-pve-san-fenced)

---

## 1. Project Overview <a name="project-overview"></a>

The pve-san-fenced project develops a SAN fencing daemon for Proxmox VE (Virtual Environment) along with supporting tools and libraries. The primary purpose is to provide SAN/FC (Fibre Channel) storage fencing capabilities for Proxmox clusters.

The project consists of:
- Two Rust libraries for interacting with multipathd and Proxmox VE
- Four CLI tools for querying and mocking system components
- A main daemon (pve-san-fenced) - to be implemented

All components are written in Rust 2021 edition and use the following common dependencies (defined in workspace Cargo.toml):
- `clap = { version = "4.0", features = ["derive"] }` - For CLI argument parsing
- `lsblk = "0.6.1"` - For block device information
- `serde = { version = "1.0", features = ["derive"] }` - For serialization
- `serde_json = "1.0"` - For JSON handling
- `thiserror = "1.0"` - For error handling
- `tokio = { version = "1.0", features = ["rt", "sync", "macros"] }` - For async runtime

---

## 2. Workspace Structure <a name="workspace-structure"></a>

```
pve-san-fenced/
├── Cargo.toml                    # Workspace manifest with shared dependencies
├── AGENTS.md                     # Coding guidelines and requirements
├── LICENSE                       # GNU AGPL v3 license
├── libmultipath/
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs                # Multipathd communication library
│   └── tests/
│       └── integration_test.rs   # Integration tests for libmultipath
├── libpve-san/
│   ├── Cargo.toml
│   ├── src/
│   │   └── lib.rs                # Proxmox VE SAN information library
│   └── tests/
│       └── integration_test.rs   # Integration tests for libpve-san
└── tools/
    ├── mpath-query/
    │   ├── Cargo.toml
    │   ├── src/
    │   │   └── main.rs            # CLI tool for querying multipathd
    │   └── tests/
    │       └── integration_test.rs
    ├── mpath-mockd/
    │   ├── Cargo.toml
    │   ├── src/
    │   │   └── main.rs            # Mock multipathd daemon for testing
    │   └── tests/
    │       └── daemon_test.rs     # Daemon tests
    ├── pve-san-query/
    │   ├── Cargo.toml
    │   ├── src/
    │   │   └── main.rs            # CLI tool for querying Proxmox SAN info
    │   └── tests/
    │       └── integration_test.rs
    └── pvesh-mock/
        ├── Cargo.toml
        └── src/
            └── main.rs            # Mock pvesh command for testing
└── test-data/
    ├── multipathd/                # Test data for multipathd mocking
    │   ├── show_maps_json/
    │   │   ├── all_active_running.json
    │   │   └── failed_all_timeout.json
    │   ├── show_config/
    │   │   └── show_config.txt
    │   ├── show_status/
    │   │   └── show_status.txt
    │   ├── show_topology/
    │   │   └── show_topology.txt
    │   └── list_maps/
    │       └── list_maps.txt
    └── pvesh/                     # Test data for pvesh mocking
        ├── get_nodes/
        │   ├── pve001_qemu.json
        │   ├── config/
        │   │   ├── 104.json
        │   │   ├── 105.json
        │   │   ├── ...
        │   │   └── 147.json
        └── lsblk.json
```

---

## 3. Libraries <a name="libraries"></a>

### 3.1 libmultipath <a name="31-libmultipath"></a>

**Purpose**: Provide a Rust library for communicating with the multipathd daemon via its abstract namespace Unix domain socket, using the same protocol as the C library `libmpathcmd`.

**Location**: `libmultipath/`

**Dependencies**:
- None (uses standard library `std::os::unix::net` and `std::os::linux::net`)

**Key Constants**:
- `DEFAULT_SOCKET: &str = "/org/kernel/linux/storage/multipathd"` - Default abstract namespace socket path
- `MAX_REPLY_LEN: usize = 32 * 1024 * 1024` - Maximum reply length (32 MB, matching C implementation)
- `DEFAULT_REPLY_TIMEOUT_MS: u64 = 4000` - Default reply timeout in milliseconds

**Main Types**:

```rust
pub struct MultipathConnection {
    stream: std::os::unix::net::UnixStream,
}
```
- Represents a connection to the multipathd daemon
- `stream` is the `UnixStream` for the socket connection

**Implementation Details**:

The `MultipathConnection` struct provides methods for:

1. **Creating connections**:
   - `new()` - Creates a connection using the default socket path
   - `with_socket(socket_path: &str)` - Creates a connection to a specified socket

2. **Sending commands**:
   - `send_command(&self, command: &str, timeout_ms: Option<u64>)` - Sends a command and receives reply
   - `send_command_on_stream(stream: &UnixStream, command: &str, timeout_ms: Option<u64>)` - Static method to send on given stream
   - `send_command_on_fd(fd: i32, command: &str, timeout_ms: Option<u64>)` - Static compatibility method to send on given FD
   - `send_command_no_reply(&self, command: &str)` - Sends command without waiting for reply

3. **Internal helper methods**:
   - `connect_to_socket(socket_path: &str)` - Establishes connection to abstract namespace socket
   - `send_command_stream(stream: &UnixStream, command: &str)` - Sends command bytes with length prefix
   - `receive_reply_stream(stream: &UnixStream)` - Receives and validates reply from stream

**Protocol Details**:

The library implements the multipathd protocol:
1. Connect to abstract namespace socket (`AF_UNIX`, `SOCK_STREAM`)
2. For abstract namespace: `sun_path[0] = 0`, name starts at `sun_path[1]`
3. Strip `@` prefix from socket path if present (it's a systemd convention, not part of the name)
4. Send command: First send 8-byte little-endian length, then command bytes (null-terminated)
5. Receive reply: First read 8-byte little-endian length, then read that many bytes
6. Reply is null-terminated by daemon; library ensures null byte and converts to String

**Convenience Functions**:

```rust
pub fn send_multipath_command(command: &str) -> io::Result<String>
pub fn send_multipath_command_with_timeout(command: &str, timeout_ms: u64) -> io::Result<String>
pub fn send_multipath_command_to_socket(socket_path: &str, command: &str) -> io::Result<String>
pub fn send_multipath_command_to_socket_with_timeout(
    socket_path: &str,
    command: &str,
    timeout_ms: u64,
) -> io::Result<String>
```

These provide simple one-line access to multipathd without managing connections manually.

**Error Handling**:
- Uses `std::io::Error` for all errors
- Handles connection errors, timeouts (EAGAIN/EWOULDBLOCK), invalid data, UTF-8 conversion errors
- Validates reply length is within bounds (0 < len < MAX_REPLY_LEN)

**Cleanup**:
- `UnixStream` automatically closes the socket file descriptor on drop, so no manual `Drop` implementation is required for `MultipathConnection`.

**Testing**:
- Uses `mpath-mockd` as a test double
- Integration tests verify all command types and timeout behavior

---

### 3.2 libpve-san <a name="32-libpve-san"></a>

**Purpose**: Library for retrieving SAN/FC storage information from Proxmox VE hosts by querying running VMs, their disk configurations, and underlying block devices.

**Location**: `libpve-san/`

**Dependencies**:
- `lsblk = "0.6.1"` - For enumerating block devices
- `serde = { version = "1.0", features = ["derive"] }` - For serialization traits
- `serde_json = "1.0"` - For JSON parsing
- `thiserror = "1.0"` - For custom error types
- `tokio = { version = "1.0", features = ["rt", "sync", "macros", "process"] }` - For async operations
- `log = "0.4"` - For logging

**Error Type**:

```rust
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

pub type PveSanResult<T> = Result<T, PveSanError>;
```

**Data Structures**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmDisk {
    pub device_id: String,           // e.g., "scsi0", "virtio0"
    pub storage: String,             // e.g., "local-lvm:vm-100-disk-0"
    pub device_path: Option<String>, // e.g., "/dev/dm-0"
    pub device_mapper_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub vmid: u64,
    pub name: String,
    pub status: String,
    pub disks: Vec<VmDisk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDeviceInfo {
    pub name: String,
    pub path: String,
    pub device_type: String,
    pub size: u64,
    pub dm_name: Option<String>,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub uuid: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub mount_point: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanStorageInfo {
    pub node: String,
    pub vms: Vec<VmInfo>,
    pub block_devices: Option<Vec<BlockDeviceInfo>>,
}

#[derive(Debug, Clone)]
pub struct PveSanConfig {
    pub node: String,                // Required: node name to query
    pub pvesh_command: String,       // Default: "pvesh"
}
```

**Main Client**:

```rust
pub struct PveSanClient {
    config: PveSanConfig,
}

impl PveSanClient {
    pub fn new(config: PveSanConfig) -> PveSanResult<Self>
}
```

**Core Methods**:

1. `get_san_storage_info(&self) -> PveSanResult<SanStorageInfo>` (async)
   - Main entry point that retrieves complete SAN storage info
   - Calls `list_running_vms()` to get VM list
   - For each running VM, calls `get_vm_config()` and `extract_disks()`
   - Calls `get_block_devices()` to enumerate block devices via lsblk
   - Returns combined `SanStorageInfo` structure

2. `list_running_vms(&self) -> PveSanResult<Vec<(u64, String)>>` (async)
   - Uses pvesh to list VMs at `/nodes/{node}/qemu`
   - Parses JSON output to extract VMID and status
   - Filters to only running VMs

3. `get_vm_config(&self, vmid: u64) -> PveSanResult<HashMap<String, String>>` (async)
   - **Optimization**: First attempts to read the config directly from `/etc/pve/local/qemu-server/{vmid}.conf`. This avoids the massive CPU overhead of spawning `pvesh` for every VM.
   - If the local file read fails (e.g., querying a remote node without local pmxcfs), falls back to using `pvesh get /nodes/{node}/qemu/{vmid}/config --output-format json`.
   - Parses the resulting config file or JSON.

4. `parse_vm_config(&self, config_text: &str) -> PveSanResult<HashMap<String, String>>`
   - Handles both JSON and key:value config formats
   - For JSON: converts all values to strings
   - For key:value: splits on first colon, trims whitespace, skips comments and blank lines

5. `extract_disks(&self, config_map: &HashMap<String, String>) -> PveSanResult<Vec<VmDisk>>`
   - Scans config keys for disk prefixes: scsi, virtio, sata, ide, efidisk
   - For each matching key, extracts index (e.g., "scsi0" -> index 0)
   - Calls `parse_disk_value()` to parse the storage specification

6. `parse_disk_value(&self, value: &str) -> PveSanResult<(String, HashMap<String, String>)>`
   - First part (before comma) is storage specification
   - Format: `storage_name:volume` or just `storage_name`
   - Subsequent parts are `key=value` pairs added to metadata
   - Example: `"local-lvm:vm-100-disk-0,size=10G,backup=0"`
     - Storage: `"local-lvm:vm-100-disk-0"`
     - Metadata: `{ size: "10G", backup: "0" }`

7. `get_block_devices(&self) -> PveSanResult<Vec<BlockDeviceInfo>>`
   - Uses `lsblk::BlockDevice::list()` to enumerate all block devices
   - Converts each `BlockDevice` to `BlockDeviceInfo`

8. `convert_block_device(&self, device: &BlockDevice) -> BlockDeviceInfo`
   - Maps lsblk device properties to simplified structure
   - Determines device_type based on UUID/partitions
   - Extracts dm_name for device-mapper devices

9. `run_pvesh(&self, args: &[&str]) -> PveSanResult<String>` (async)
   - Executes pvesh command with given arguments
   - Validates pvesh is available
   - Returns stdout as String
   - Handles errors and command failure

**Helper Methods**:

1. `run_pvesh_ls(&self, path: &str) -> PveSanResult<String>`
   - Wrapper for `pvesh ls <path> --output-format json`

2. `run_pvesh_get(&self, path: &str) -> PveSanResult<String>`
   - Wrapper for `pvesh get <path> --output-format json`

3. `parse_size(size_str: &str) -> Option<u64>`
   - Parses size strings like "10G", "10GB", "100M", "1T", etc.
   - Supports K/KB, M/MB, G/GB, T/TB suffixes
   - Returns bytes as u64

**Public Convenience Functions**:

```rust
pub async fn get_san_storage_info(node: &str) -> PveSanResult<SanStorageInfo>
pub async fn get_san_storage_info_with_pvesh(
    node: &str,
    pvesh_command: &str,
) -> PveSanResult<SanStorageInfo>
pub fn get_san_storage_info_sync(node: &str) -> PveSanResult<SanStorageInfo>
pub fn get_san_storage_info_sync_with_pvesh(
    node: &str,
    pvesh_command: &str,
) -> PveSanResult<SanStorageInfo>
```

The sync versions create a single-threaded Tokio runtime and block on the async version.

**Testing**:
- Unit tests for config parsing, disk extraction, size parsing
- Integration tests use `pvesh-mock` to simulate pvesh responses

---

## 4. Tools <a name="tools"></a>

### 4.1 mpath-query <a name="41-mpath-query"></a>

**Purpose**: CLI tool to send commands to multipathd daemon and retrieve responses.

**Location**: `tools/mpath-query/`

**Dependencies**:
- `libmultipath = { path = "../../libmultipath" }`
- `clap = { version = "4.0", features = ["derive"] }`

**CLI Arguments** (using clap derive):

```rust
#[derive(Parser, Debug)]
#[command(name = "mpath-query")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
struct Cli {
    /// The command to send to multipathd (default: "show maps json")
    #[arg(long, short, default_value = "show maps json")]
    command: String,

    /// The output file (default: stdout)
    #[arg(long, short)]
    output: Option<String>,

    /// The socket path to connect to
    #[arg(long, default_value = DEFAULT_SOCKET)]
    socket: String,

    /// Verbose output
    #[arg(long, short)]
    verbose: bool,

    /// Subcommands (alternative to direct command argument)
    #[command(subcommand)]
    subcommand: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(alias = "maps")]
    ShowMapsJson,
    #[command(alias = "topology")]
    ShowTopology,
    #[command(alias = "config")]
    ShowConfig,
    #[command(alias = "status")]
    ShowStatus,
    #[command(alias = "list")]
    ListMaps,
}
```

**Behavior**:

1. Parse CLI arguments
2. Determine command to send:
   - If subcommand provided, map to corresponding string
   - Otherwise use `--command` argument
3. If verbose, print socket and command to stderr
4. Call `send_multipath_command_to_socket()` from libmultipath
5. On success:
   - If `--output` specified, write to file
   - Otherwise write to stdout
6. On error:
   - Print error to stderr
   - If verbose, print troubleshooting hints
   - Exit with code 1

**Subcommand Mapping**:
- `show-maps-json` or `maps` -> "show maps json"
- `show-topology` or `topology` -> "show topology"
- `show-config` or `config` -> "show config"
- `show-status` or `status` -> "show status"
- `list-maps` or `list` -> "list maps"

**Testing**:
- Integration tests use `mpath-mockd` as the multipathd test double
- Tests verify: default command, output to file, custom commands, subcommands, verbose mode
- Tests must run serially (--test-threads=1) to avoid socket conflicts

---

### 4.2 mpath-mockd <a name="42-mpath-mockd"></a>

**Purpose**: Mock multipathd daemon for testing that responds to multipath commands with test data from JSON files.

**Location**: `tools/mpath-mockd/`

**Dependencies**:
- `clap = { version = "4.0", features = ["derive"] }`
- `serde = { version = "1.0", features = ["derive"] }`
- `serde_json = "1.0"`

**CLI Arguments**:

```rust
#[derive(Parser, Debug)]
#[command(name = "mpath-mockd")]
struct Cli {
    /// The socket path to listen on (default: @/org/kernel/linux/storage/multipathd)
    #[arg(long, default_value = DEFAULT_SOCKET)]
    socket: String,

    /// The directory containing test data JSON files
    #[arg(long, default_value = "test-data/multipathd")]
    test_data_dir: PathBuf,

    /// Verbose output
    #[arg(long, short)]
    verbose: bool,

    /// File mappings for commands (format: command=filename or command=file1,file2,file3)
    /// Can be specified multiple times. Files are cycled through in round-robin fashion.
    #[arg(long, value_name = "command=file[s]", action = clap::ArgAction::Append)]
    file_map: Vec<String>,
}
```

**Constants**:
- `DEFAULT_SOCKET = "@/org/kernel/linux/storage/multipathd-mock"`
- `DEFAULT_SHOW_MAPS_JSON_FILE = "all_active_running.json"`

**Behavior**:

1. Parse CLI arguments and custom file mappings
2. Load test data files:
   - For each known command, try to load from subdirectory (e.g., `show_maps_json/`, `show_topology/`, etc.)
   - Support custom file mappings via `--file-map` argument
   - Format: `command=file1,file2,file3` - cycles through files in round-robin
   - If no custom mapping, auto-load all files from command's subdirectory
   - Fallback to default files if subdirectory approach fails
3. Create abstract namespace socket and bind to it
4. Set SO_REUSEADDR for quick restart capability
5. Listen for connections (backlog of 5)
6. For each connection:
   - Spawn a thread to handle it
   - Read 8-byte command length (little-endian)
   - Read command string (stripping null terminator)
   - Look up response based on command
   - If multiple files mapped to command, cycle through them using FileCounters
   - Send 8-byte response length (little-endian)
   - Send response with null terminator
   - Close connection

**Command to Subdirectory Mapping**:
- `"show maps json"` -> `"show_maps_json"`
- `"show topology"` -> `"show_topology"`
- `"list maps"` -> `"list_maps"`
- `"show status"` -> `"show_status"`
- `"show config"` -> `"show_config"`

**Default Files**:
- `"show maps json"` -> `"all_active_running.json"`
- `"show topology"` -> `"show_topology.txt"`
- `"list maps"` -> `"list_maps.txt"`
- `"show status"` -> `"show_status.txt"`
- `"show config"` -> `"show_config.txt"`

**FileCounters Structure**:
- Tracks current index for each command
- Incremented with each request, wraps around at max
- Thread-safe using Mutex

**Socket Implementation**:
- Creates AF_UNIX SOCK_STREAM socket
- For abstract namespace: sun_path[0] = 0, name at sun_path[1..]
- Strips `@` prefix from socket path if present
- Binds with calculated address length: 2 (sun_family) + 1 (null at sun_path[0]) + name_len

**Testing**:
- Tests verify daemon starts and responds to commands
- Tests for custom socket paths
- Tests for file mapping and cycling
- Tests for all known commands

---

### 4.3 pve-san-query <a name="43-pve-san-query"></a>

**Purpose**: CLI tool to query Proxmox VE hosts for SAN/FC storage information and output as JSON.

**Location**: `tools/pve-san-query/`

**Dependencies**:
- `libpve-san = { path = "../../libpve-san" }`
- `clap = { workspace = true }`
- `serde_json = { workspace = true }`
- `tokio = { workspace = true, features = ["rt", "macros"] }`

**CLI Arguments**:

```rust
#[derive(Parser, Debug)]
#[command(name = "pve-san-query")]
struct Cli {
    /// The Proxmox node name to query
    #[arg(long, short = 'n')]
    node: String,

    /// The output file (default: stdout)
    #[arg(long, short = 'o')]
    output: Option<String>,

    /// Pretty print JSON output
    #[arg(long, short = 'p')]
    pretty: bool,

    /// Verbose output
    #[arg(long, short = 'v')]
    verbose: bool,
}
```

**Behavior**:

1. Parse CLI arguments
2. If verbose, print node being queried
3. Call `get_san_storage_info_sync(&node)` from libpve-san
4. On success:
   - Serialize result to JSON (pretty or compact based on --pretty)
   - If --output specified, write to file
   - Otherwise write to stdout
   - Print byte count to stderr if verbose
5. On error:
   - Print error to stderr
   - If verbose, print troubleshooting hints (pvesh availability, API access, node existence, network)
   - Exit with code 1

**Testing**:
- Integration tests use `pvesh-mock` to simulate pvesh responses
- Tests verify: valid JSON output, expected structure (node, vms fields), output to file, pretty printing
- Uses a wrapper script approach to override pvesh command in PATH

---

### 4.4 pvesh-mock <a name="44-pvesh-mock"></a>

**Purpose**: Mock pvesh command that simulates the Proxmox VE CLI by parsing arguments and returning test data from JSON files.

**Location**: `tools/pvesh-mock/`

**Dependencies**:
- `clap = { workspace = true }`
- `serde = { workspace = true }`
- `serde_json = { workspace = true }`

**CLI Arguments**:

```rust
#[derive(Parser, Debug)]
#[command(name = "pvesh-mock")]
struct Cli {
    /// The command to execute (ls, get)
    #[arg(value_enum)]
    command: CommandType,

    /// The path to query (e.g., /nodes/pve001/qemu)
    path: String,

    /// Output format
    #[arg(long, short = 'o', value_name = "FORMAT")]
    output_format: Option<String>,

    /// Verbose output
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CommandType {
    #[value(alias = "ls")]
    Ls,
    #[value(alias = "get")]
    Get,
}
```

**Constants**:
- `DEFAULT_TEST_DATA_DIR = "test-data/pvesh"`

**Behavior**:

1. Parse CLI arguments
2. If verbose, print command, path, and output_format
3. Determine test data directory:
   - First try `PVE_SAN_TEST_DATA_DIR` environment variable
   - Otherwise, go up 3 levels from binary location and append default dir
4. Parse path to extract components (node, vmid, etc.)
5. Route to appropriate handler based on command and path:
   - `ls /nodes/{node}/qemu` -> `handle_ls_nodes_qemu()`
   - `get /nodes/{node}/qemu/{vmid}/config` -> `handle_get_vm_config()`
   - Unsupported paths -> error and exit
6. If output_format is "json" or "json-pretty":
   - Get response JSON string
   - If "json-pretty", pretty-print the JSON
   - Output to stdout
7. For other output formats, print error (not supported)

**Handler Functions**:

1. `handle_ls_nodes_qemu(node: &str, test_data_dir: &PathBuf) -> Option<String>`
   - Looks for file: `test-data/pvesh/get_nodes/{node}_qemu.json`
   - Falls back to: `test-data/pvesh/get_nodes/pve001_qemu.json`
   - Returns file contents or None

2. `handle_get_vm_config(_node: &str, vmid: &str, test_data_dir: &PathBuf) -> Option<String>`
   - Looks for file: `test-data/pvesh/get_nodes/config/{vmid}.json`
   - Returns file contents or None

**Usage Notes**:
- Designed to be used as a drop-in replacement for pvesh in testing
- Can be placed in PATH as "pvesh" for testing tools that call pvesh
- Uses `PVE_SAN_TEST_DATA_DIR` environment variable to override test data location

**Testing**:
- Used by `pve-san-query` integration tests
- Tested indirectly through those integration tests

---

## 5. Test Data <a name="test-data"></a>

**Location**: `test-data/`

**Structure**:

```
test-data/
├── multipathd/                # Test data for mpath-mockd
│   ├── show_maps_json/
│   │   ├── all_active_running.json    # Sample multipath maps with all paths active
│   │   └── failed_all_timeout.json     # Sample with failed paths (I/O timeout)
│   ├── show_config/
│   │   └── show_config.txt             # Sample multipath config
│   ├── show_status/
│   │   └── show_status.txt             # Sample multipath status
│   ├── show_topology/
│   │   └── show_topology.txt           # Sample multipath topology
│   └── list_maps/
│       └── list_maps.txt               # Sample list of maps
└── pvesh/                     # Test data for pvesh-mock
    ├── get_nodes/
    │   ├── pve001_qemu.json             # List of VMs for node pve001
    │   └── config/
    │       ├── 104.json                 # VM config for VMID 104
    │       ├── 105.json                 # VM config for VMID 105
    │       ├── ...
    │       └── 147.json                 # VM config for VMID 147
    └── lsblk.json                       # Sample lsblk output (not currently used)
```

**Important Constraints**:
- Test data files must NOT be edited - they contain real program output for testing
- If test data is missing for a test case, document in an errata
- Mocking daemons must never listen on the same sockets as real-world examples

---

## 6. PVE SAN Fencing Daemon (pve-san-fenced) <a name="pve-san-fencing-daemon-pve-san-fenced"></a>

**Purpose**: A robust daemon to continuously monitor multipath states via `libmultipath` and write to the kernel SysRq trigger to fence the node upon complete, persistent SAN storage loss.

**Location**: `src/` (root package of the workspace)

**Dependencies**:
- `libmultipath = { path = "../libmultipath" }`
- `libpve-san = { path = "../libpve-san" }`
- `tokio = { workspace = true, features = ["rt", "time", "macros", "fs", "process", "sync"] }`
- `serde = { workspace = true }`
- `serde_json = { workspace = true }`
- `log = "0.4"`
- `env_logger = "0.10"` (or similar for stdout logging)
- `clap = { workspace = true }`

**CLI Arguments**:
- `--poll-interval` (default: 5): Seconds between multipathd checks.
- `--discovery-interval` (default: 60): Seconds between VM and storage discovery scans.
- `--max-failures` (default: 6): Number of consecutive failures before fencing (6 * 5s = 30s).
- `--target-wwids` (optional, multiple): Specific WWIDs to monitor. If empty, monitors maps discovered to be in use.
- `--socket` (default: `DEFAULT_SOCKET` from libmultipath): Multipath socket to connect to.
- `--node` (required): The name of the Proxmox node to query for VM data.
- `--test-mode` / `-t` (optional, flag): Runs in test mode (only logs changes and decisions, does not trigger reboot/fencing).
- `--sysrq-char` (default: `b`): The character to write to `/proc/sysrq-trigger`. `b` reboots immediately (recommended for fast HA fencing), while `c` causes a kernel panic (useful for debugging, but delays reboot if kdump is active).

**Data Structures**:

```rust
#[derive(Deserialize)]
struct MultipathOutput {
    maps: Option<Vec<MultipathMap>>,
}

#[derive(Deserialize)]
struct MultipathMap {
    name: String, // WWID
    uuid: String,
    path_groups: Option<Vec<PathGroup>>,
}

#[derive(Deserialize)]
struct PathGroup {
    state: String, // "active", "offline", "failed", etc.
}
```

**Architecture & Core Logic**:

To avoid IO lockups due to FC failures blocking the monitoring loop, the daemon is structured into two independent concurrent asynchronous tasks. Data is shared using a thread-safe structure like `Arc<RwLock<HashSet<String>>>` containing the active WWIDs/dm-names.

0. **Startup Validation**:
   - Verify that `/proc/sys/kernel/sysrq` contains a value that permits the configured sysrq trigger (e.g., > 0). If sysrq is disabled, log a critical warning that fencing will fail.

1. **Discovery Task (VM & Storage Mapping)**:
   - Construct an async loop that executes every `DISCOVERY_INTERVAL` seconds.
   - **Timeout Protection**: Wrap the entire discovery execution (`get_san_storage_info` and `lsblk`) in a `tokio::time::timeout` (e.g., 30 seconds). If SAN fails, block IO can cause these commands to hang in an uninterruptible sleep (D-state). If a timeout occurs, retain the previous `HashSet` of active LUNs.
   - Uses `libpve-san` (`get_san_storage_info`) to read the configs of all running VMs and discover their storage endpoints.
   - Uses `lsblk` (via `libpve-san`) to discover underlying block devices and device mapper layers.
   - Finds the multipath device mapper device (`dm_name` / `WWID`) associated with the storage if it is in use by a running VM.
   - Logs any change in the active multipath devices set at `info` level (showing the previous and new sets).
   - Replaces the shared `HashSet` of active LUNs with the newly discovered in-use multipath devices.
   - This separate thread ensures that if `lsblk` or `pvesh` block due to underlying storage IO lockups during a SAN failure, it does not prevent the monitoring thread from executing the fencing action.
   - Logs discovered storage configurations, built lsblk mapping, and final active multipath sets at `debug` level.

2. **Monitoring Task (Failure Detection and Fencing)**:
   - Construct an async loop that executes every `POLL_INTERVAL` seconds using `tokio::time::interval`.
   - **Multipath Query**:
     - Call `libmultipath::send_multipath_command_to_socket(&config.socket, "show maps json")`.
     - Implement exception handling: If `multipathd` fails to respond or crashes (socket error or timeout), log a critical warning and increment a separate daemon-failure counter, but **do not** immediately panic the system to avoid false positives.
   - **JSON Parsing and Validation**:
     - Parse the JSON output into the `MultipathOutput` struct.
     - Acquire a read lock on the shared `HashSet` of active LUNs discovered by the Discovery Task.
     - Initialize an `all_paths_dead = true` flag.
     - **Filtering**: Only evaluate maps that are present in the active LUNs `HashSet` (or explicitly provided via `--target-wwids`). Ignore failures for LUNs not actively used by any running VM.
     - For each actively used map, iterate through its `path_groups`.
     - Within each path group, also evaluate the `paths` array. A path group is only considered truly alive if it contains at least one path with a state that is not `"failed"`, `"faulty"`, or `"ghost"`.
     - If **any** path group is alive and reports a `state` other than `"offline"` or `"failed"` (e.g., `"active"`, `"enabled"`), set `all_paths_dead = false` and break the loop for that map.
     - Track map states (dead vs. alive) across monitoring cycles. If a monitored map's state transitions, log this change at `info` level.
     - Debug log all states (fencer consecutive failures, active set, fencer cycle status, path states) and raw configs returned by `multipathd` at `debug` level.
   - **Threshold Evaluation**:
     - If `all_paths_dead == true` (meaning all *actively used* LUNs have lost all paths), increment the consecutive failure counter.
     - Log a warning: `"Consecutive storage failure {count}/{MAX_FAILURES}"`.
     - If `all_paths_dead == false` or there are no actively used LUNs, reset the consecutive failure counter to 0.
   - **The Fencing Execution Block**:
     - If the consecutive failure counter meets or exceeds `MAX_FAILURES`, execute the final sequence:
       1. Log the decision with the detailed reason (default logging):
          `"DECISION: Rebooting node because all monitored multipath maps in use by running VMs have failed. Failed monitored maps: {dead_map_names:?}. Active LUNs: {active_luns:?}. Target WWIDs: {target_wwids:?}."`
       2. Check if `--test-mode` / `-t` flag is active:
          - If **active**, log that fencing decision was reached but not executed:
            `"TEST MODE: Fencing decision reached, but not executing reboot/SysRq kernel panic."`
            The loop will then continue normal monitoring without executing the panic or reboot.
          - If **not active**, execute the fencing sequence:
            1. Log critical: `"SAN FENCER: Total persistent storage loss detected. Threshold met."`
             2. Log critical: `"SAN FENCER: Initiating filesystem sync..."`
             3. Sync filesystems to flush any memory buffers for local storage (OS disk) using `std::process::Command::new("sync").status()`.
             4. Wait 2 seconds using `tokio::time::sleep(std::time::Duration::from_secs(2)).await`.
            5. Log critical: `"SAN FENCER: Triggering SysRq Fencing NOW."`
            6. Trigger Fencing:
               - Attempt to write the configured `--sysrq-char` (default `"b"`) to `/proc/sysrq-trigger` using `tokio::fs::write`.
               - If it fails, log the error and aggressively attempt to write `"b"` as a fallback to ensure the node reboots.

**Testing**:
- Use `mpath-mockd` as a test double to simulate multipathd responses.
- Write unit tests for the JSON parsing, LUN filtering, and threshold evaluation logic.
- Mock the fencing execution block (e.g., using a trait, function pointer, or dry-run flag) to verify it is called without actually panicking the test system.
- Include integration tests verifying that transient failures (e.g., 2 failures followed by recovery) reset the counter, and sustained failures trigger fencing.
- Include tests for CLI argument parsing (e.g., validating that the `-t` and `--test-mode` flags are correctly parsed).
