# Monitoring pve-san-fenced

The `pve-san-fenced` daemon maintains a status file to communicate its internal health and the state of monitored SAN storage paths. Monitoring tools (such as Nagios, Icinga, Zabbix, or Prometheus) can inspect this status file to raise alerts or trigger reboots.

---

## The Status Check Interface

The fencer daemon writes its status to a text file (default: `/run/pve-san-fenced/status`).
You can query this status and receive Nagios-compatible exit codes and health descriptions by running:

```bash
pve-san-fenced --status
```

### Exit Code Mapping
The status check exits with one of the following standard Nagios/Icinga codes:

| Exit Code | Status | Description |
|---|---|---|
| **0** | **OK** | The daemon is running normally and all monitored storage paths are healthy. |
| **1** | **WARNING** | Non-critical conditions. The fencer continues monitoring, but attention is required (e.g. transient storage failures or config issues). |
| **2** | **CRITICAL** | Critical health issues. The reboot/fencing sequence has been triggered, or a startup configuration validation failed. |
| **3** | **UNKNOWN** | Status file is missing, empty, badly formatted, or outdated (indicating the daemon has crashed or hung). |

---

## Status Messages, Reasons & Solutions

### 1. OK - Daemon is running normally
* **Meaning**: Storage paths are healthy, VM discovery is up to date, and the daemon is active.
* **Troubleshooting**: No action required.

---

### 2. UNKNOWN - Status file is outdated (last modified X seconds ago)
* **Meaning**: The daemon has stopped updating the status file. Outdated check threshold is `max(30, 3 * poll_interval)` seconds (default 30 seconds).
* **Possible Reasons**:
  * The `pve-san-fenced` service is stopped.
  * The daemon crashed due to an unhandled panic.
  * The daemon is hung (e.g. blocked on a synchronous system call or lockup).
* **Solutions**:
  1. Check if the service is running:
     ```bash
     systemctl status pve-san-fenced
     ```
  2. Inspect the service logs:
     ```bash
     journalctl -u pve-san-fenced -n 50
     ```
  3. Restart the service:
     ```bash
     systemctl restart pve-san-fenced
     ```

---

### 3. WARNING - Active LUN data is stale (older than 2x discovery interval)
* **Meaning**: The fencer has not successfully updated its list of VM-attached SAN LUNs. To avoid false-positive fencing reboots on outdated information, the fencer skips storage state evaluations while this condition is present.
* **Possible Reasons**:
  * The VM/storage discovery thread is encountering errors querying the Proxmox VE API/config.
  * The cluster configuration filesystem (`/etc/pve`) is locked or slow.
  * `pvesh` commands are timing out due to cluster communication lags.
* **Solutions**:
  1. Check the daemon log for VM discovery errors:
     ```bash
     journalctl -u pve-san-fenced | grep "Error discovering active multipath"
     ```
  2. Verify that local Proxmox VE configurations are readable:
     ```bash
     ls -la /etc/pve/local/qemu-server/
     ```
  3. Verify that the Proxmox VE cluster API is responsive:
     ```bash
     pvesh get /nodes/localhost/qemu --output-format json
     ```

---

### 4. WARNING - Consecutive storage failure: X/Y
* **Meaning**: One or more SAN storage paths in use by running VMs have failed.
* **Possible Reasons**:
  * Storage controller (Fibre Channel/iSCSI target) port flap or offline event.
  * Fiber optic cable or switch port failures.
  * Storage array controller reboot/takeover.
* **Solutions**:
  1. Inspect the multipath configuration and paths state:
     ```bash
     multipath -ll
     ```
  2. Inspect kernel storage connection logs:
     ```bash
     dmesg -T | grep -E "sd|lpfc|qla|iscsi|multipath"
     ```
  3. Investigate SAN switches and storage controller health.

---

### 5. WARNING - Multipath configuration recommendation warnings: ...
* **Meaning**: The daemon successfully queried `multipathd` but detected configuration parameters that do not align with optimal fencing recommendations.
* **Possible Reasons**:
  * `no_path_retry` is not set to `queue`. If set to `fail` or a numeric value, paths fail immediately on transient drops rather than queueing.
  * `dev_loss_tmo` is not set to `infinity`. If set to a numeric value, paths are removed from the system during sustained drops, which can prevent the fencer from detecting the dead LUNs and executing panic/reboots.
* **Solutions**:
  1. Edit `/etc/multipath.conf` and update the defaults:
     ```text
     defaults {
         no_path_retry "queue"
         dev_loss_tmo "infinity"
     }
     ```
  2. Reload `multipathd` to apply:
     ```bash
     systemctl reload multipath-tools
     ```
  3. Restart `pve-san-fenced` to clear the warning.

---

### 6. CRITICAL - Rebooting node because monitored multipath maps in use by running VMs have failed
* **Meaning**: A total storage loss has occurred on a SAN LUN that is currently mapped to one or more running virtual machines. The fencer has initiated the SysRq panic/reboot sequence to prevent VM disk corruption.
* **Possible Reasons**:
  * Total connection loss to the storage array (e.g. host-bus adapter failure, storage target failure, or SAN fabric disruption).
* **Solutions**:
  1. If the node has rebooted, inspect the logs prior to the reboot to identify the failing LUNs.
  2. Resolve the underlying hardware/network connectivity issue before starting the cluster node.

---

## General Debugging & Diagnostics

### Run in Debug/Verbose Mode
To output detailed discovery logs and state evaluations, set the log level using `PVE_SAN_DEBUG=true` or `RUST_LOG=debug`:

```bash
# Run manually to inspect output
PVE_SAN_DEBUG=true pve-san-fenced --node-name $(hostname)
```

### Inspect the Status File Directly
To check the status raw content without using the CLI status tool:

```bash
cat /run/pve-san-fenced/status
```

### Verify Status Configuration
Ensure that `/lib/systemd/system/pve-san-fenced.service` has the correct `RuntimeDirectory` configuration so that `/run/pve-san-fenced/` exists and has the correct permissions:

```ini
[Service]
RuntimeDirectory=pve-san-fenced
```
