# pve-san-fenced

A SAN fencing daemon for Proxmox VE (PVE) designed to prevent split-brain scenarios and storage corruption when a hypervisor node loses all connectivity to active SAN/Fibre Channel (FC) multipath devices.

## Project Structure

The project consists of the following components:

- **`pve-san-fenced` (Daemon)**: The main background service that continuously monitors multipath maps in use by running VMs on the Proxmox VE node. If persistent, complete storage loss is detected, it triggers a hardware fence using the Linux SysRq trigger (`/proc/sysrq-trigger`).
- **`libmultipath` (Library)**: A Rust library that communicates with `multipathd` via its abstract namespace Unix socket using the protocol defined by `libmpathcmd`.
- **`libpve-san` (Library)**: A Rust library that queries Proxmox VE VM configurations (either directly from local files or via `pvesh`), resolves disk-to-device mapping using `/sys/block` sysfs traversal and the `lsblk` crate, and returns structured info.
- **`pve-san-query` (Tool)**: A CLI tool to inspect the local Proxmox node's VM storage mappings in JSON format.
- **`mpath-query` (Tool)**: A CLI tool that queries `multipathd` using commands like `show maps json` or `show config`.
- **`mpath-mockd` (Mock Tool)**: A mock socket daemon used in tests to simulate `multipathd` behaviour.
- **`pvesh-mock` (Mock Tool)**: A mock executable used in tests to simulate `pvesh` output.

## How it Works

1. **Discovery Phase**: On startup and at configurable intervals (default: 60 seconds), `pve-san-fenced` queries running VMs using `libpve-san` and discovers which multipath WWIDs are actively used.
2. **Monitoring Phase**: At regular intervals (default: 5 seconds), it queries `multipathd` to get the path state of the monitored WWIDs.
3. **Fencing Mechanism**: If all paths for any monitored WWID are down (i.e. state is `faulty`, `failed`, or `offline`) consecutively for a predefined threshold (default: 6 failures), the daemon immediately writes the configured SysRq sequence (default: `s,b` for sync followed by reboot) to `/proc/sysrq-trigger` to fence the node.
4. **Status & Monitoring Reporting**: The daemon periodically aggregates its warning and error states (e.g. discovery failures, config recommendation warnings, path failures) into a Nagios-compatible status file (default: `/run/pve-san-fenced/status`).

## Recommended Multipath Configuration

For the fencing daemon to operate safely and reliably, `multipathd` must be configured with specific settings. Add or update the following recommendations in the `defaults` section of `/etc/multipath.conf`:

```text
defaults {
    # Keep queueing I/O when all paths are down. The fencing daemon will take
    # care of rebooting/fencing if recovery fails.
    no_path_retry "queue"

    # Prevent multipathd from removing block devices when paths are lost.
    dev_loss_tmo "infinity"

    # Fast detection configuration
    polling_interval 5
    fast_io_fail_tmo 5
}
```

- **`no_path_retry "queue"`**: Keeps I/O queued when all paths are lost. This allows the fencing daemon time to monitor path state and decide whether to fence the node. If set to `fail`, I/O fails immediately, potentially causing VM file systems to switch to read-only before fencing occurs.
- **`dev_loss_tmo "infinity"`**: Prevents `multipathd` from deleting the device mapper mapping after a timeout. If the device was deleted, the VM could not recover even if paths are restored, and the daemon would lose its ability to track the device.
- **`polling_interval 5`**: Aligns multipath daemon's path checking interval with the fencing daemon's poll interval.
- **`fast_io_fail_tmo 5`**: Promotes fast I/O failure detection.

The `pve-san-fenced` daemon automatically and continuously inspects these settings for actively used multipath devices and reports warnings in its status output if they do not match the recommended values.
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
- `PVE_SAN_SOCKET`: Path to the `multipathd` Unix socket (default: `@/org/kernel/linux/storage/multipathd`). Supports both abstract namespace sockets (prefixed with `@`) and standard Unix domain socket files.
- `PVE_SAN_NODE_NAME`: The name of the local Proxmox VE node (default: system hostname).
- `PVE_SAN_SYSRQ_CHAR`: Comma-separated list of SysRq characters to send sequentially (default: `s,b` for sync followed by reboot. A sync `'s'` causes a 1-second sleep).
- `PVE_SAN_TEST_MODE`: Run in test/dry-run mode without actually writing to SysRq (default: empty).
- `PVE_SAN_DEBUG`: Enable verbose debug logging of discovered VMs, storages, and multipath devices and their states on each discovery run (default: empty).
- `PVE_SAN_STATUS_FILE`: Path to write the Nagios-compatible status file (default: `/run/pve-san-fenced/status`).

### Nagios Monitoring Integration

The daemon provides a Nagios-compatible health-check mechanism. You can query the current daemon health by running the daemon executable in status-query mode:

```bash
pve-san-fenced --status [--status-file /run/pve-san-fenced/status]
```

The status query exits with the corresponding Nagios-compliant exit codes:
- `0` (OK): The daemon is running normally and all monitored maps are healthy.
- `1` (WARNING): Non-critical warnings detected (e.g. transient query/discovery failures, config recommendation warnings, or partial path failure states).
- `2` (CRITICAL): Non-transient FC storage failure, fencing decision reached, or reboot execution failed.
- `3` (UNKNOWN): Status file missing, unreadable, or invalid.

## Development and Testing

The codebase has extensive integration tests covering daemon logic, multipath socket interactions, binary garbage, and error conditions.

To run the test suite locally:

```bash
cargo test --workspace
```

## License

This project is licensed under the GNU Affero General Public License (AGPL) version 3 or later. See [LICENSE](LICENSE) for the full text.
