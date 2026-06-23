# pve-san-fenced

A SAN fencing daemon for Proxmox VE (PVE) designed to prevent split-brain scenarios and storage corruption when a hypervisor node loses all connectivity to active SAN/Fibre Channel (FC) multipath devices.

## Project Structure

The project consists of the following components:

- **`pve-san-fenced` (Daemon)**: The main background service that continuously monitors multipath maps in use by running VMs on the Proxmox VE node. If persistent, complete storage loss is detected, it triggers a hardware fence using the Linux SysRq trigger (`/proc/sysrq-trigger`).
- **`libmultipath` (Library)**: A Rust library that communicates with `multipathd` via its abstract namespace Unix socket using the protocol defined by `libmpathcmd`.
- **`libpve-san` (Library)**: A Rust library that queries Proxmox VE local VM configurations (via `pvesh`), resolves disk-to-device mapping using `lsblk`, and returns structured info.
- **`pve-san-query` (Tool)**: A CLI tool to inspect the local Proxmox node's VM storage mappings in JSON format.
- **`mpath-query` (Tool)**: A CLI tool that queries `multipathd` using commands like `show maps json` or `show config`.
- **`mpath-mockd` (Mock Tool)**: A mock socket daemon used in tests to simulate `multipathd` behaviour.
- **`pvesh-mock` (Mock Tool)**: A mock executable used in tests to simulate `pvesh` output.

## How it Works

1. **Discovery Phase**: On startup and at configurable intervals (default: 60 seconds), `pve-san-fenced` queries running VMs using `libpve-san` and discovers which multipath WWIDs are actively used.
2. **Monitoring Phase**: At regular intervals (default: 5 seconds), it queries `multipathd` to get the path state of the monitored WWIDs.
3. **Fencing Mechanism**: If all paths for any monitored WWID are down (i.e. state is `faulty`, `failed`, or `offline`) consecutively for a predefined threshold (default: 6 failures), the daemon immediately writes a character (default: `b` for reboot) to `/proc/sysrq-trigger` to fence the node.

## Installation & Configuration

### Debian Package Build

The project includes standard Debian packaging (`debian/` directory). To build the package:

```bash
# Clean up target and temporary build files
just clean

# Build the Debian package
dpkg-buildpackage -us -uc -b
```

During the package build, a current stable Rust toolchain is downloaded and installed locally via `rustup` within `.cargo_home/` and `.rustup_home/` directories.

### Systemd Integration

The Debian package installs `pve-san-fenced` as a systemd service that depends on `multipathd.service`:

- Systemd Unit: `pve-san-fenced.service`
- Config File: `/etc/default/pve-san-fenced`

Configuration options can be customized in `/etc/default/pve-san-fenced`:

- `PVE_SAN_POLL_INTERVAL`: Check interval in seconds (default: 5).
- `PVE_SAN_DISCOVERY_INTERVAL`: Discovery rescanning interval in seconds (default: 60).
- `PVE_SAN_MAX_FAILURES`: Number of consecutive failures before fencing (default: 6).
- `PVE_SAN_SOCKET`: Path to the `multipathd` abstract socket (default: `@/org/kernel/linux/storage/multipathd`).
- `PVE_SAN_NODE_NAME`: The name of the local Proxmox VE node (default: system hostname).
- `PVE_SAN_SYSRQ_CHAR`: SysRq command (default: `b` for reboot, `c` for crash dump).
- `PVE_SAN_TEST_MODE`: Run in test/dry-run mode without actually writing to SysRq (default: empty).

## Development and Testing

The codebase has extensive integration tests covering daemon logic, multipath socket interactions, binary garbage, and error conditions.

To run the test suite locally:

```bash
cargo test --workspace
```

## License

This project is licensed under the GNU Affero General Public License (AGPL) version 3 or later. See [LICENSE](LICENSE) for the full text.
