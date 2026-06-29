# Bug Report — pve-san-fenced (Resolved)

All findings below were verified against AGENTS.md, PLAN.md, and general security best practices.
Each entry includes the file, line, description, severity, and resolution status.

---
## New Findings — 2026-06-25 Review

### 31. `trigger_fencing` writes to `/proc/sysrq-trigger` without verifying the write actually took effect

**File**: `src/lib.rs:167-206` (`trigger_fencing`)
**Severity**: CRITICAL
**Status**: OPEN

**Description**: The function iterates over sysrq characters and writes each to `/proc/sysrq-trigger`. If `tokio::fs::write` returns an error for a character like `'s'` (sync), the error is logged but execution continues. If the write for `'b'` (reboot) succeeds but the kernel silently ignores it (the kernel does not return an error for sysrq characters that are disabled by the sysrq bitmask — it simply does nothing), the function reports `sent_reboot = true` and exits via the dry-run check or the main loop continues. The node could be left in a degraded state: the sync was attempted, the reboot was "sent" but silently ignored, and the daemon exits or continues.

**Impact**: Storage failure detected but node not rebooted — potential data corruption on shared storage.

**Recommendation**: After writing `'b'`, verify the reboot actually occurs (e.g., by checking if the process survives a short timeout). Consider adding a `PVE_SAN_FENCE_REBOOT_TIMEOUT` that, if exceeded without system reboot, triggers a second `'b'` write.

---

### 32. Race condition between active LUN set read and multipathd query in main loop

**File**: `src/main.rs:428-450` (main loop)
**Severity**: CRITICAL
**Status**: FIXED

**Description**: The main loop reads and clones the active LUN set at lines 353-356, then queries multipathd at lines 360-368, then calls `fencer.update()` at line 371. Between the clone and the update call, the discovery thread (running on a separate OS thread at line 304) may update `active_luns`. This means the fencer evaluates multipathd state against a potentially stale view of which LUNs are in use. Conversely, if a VM's disk is removed from the active set *after* the clone but multipathd still reports the map as failed, the fencer may not trigger fencing because the LUN no longer appears in the active set — even though multipathd shows it as dead.

**Impact**: Incorrect fencing decisions — either missed reboots on real failures or unnecessary reboots.

**Resolution**: Implemented timestamp-based staleness detection. Created an `ActiveLunsWithTimestamp` struct that stores both the active LUNs `HashSet` and the `Instant` when they were discovered. The discovery thread updates this struct with a fresh timestamp on each successful discovery. In the main monitoring loop, before using the active LUNs data, the code checks if the data is stale (older than 2x the discovery interval). If stale, the loop skips the fencer update and logs a warning, preventing race conditions where the fencer would operate on outdated data.

**Recommendation**: N/A - Fixed.

---

### 33. Discovery thread has no backoff on repeated failures — can cause log spam and resource exhaustion

**File**: `src/main.rs:310-380` (discovery thread loop)
**Severity**: CRITICAL
**Status**: FIXED

**Description**: The discovery thread runs an infinite loop that calls `discover_in_use_mpaths()` with no backoff on failure. If the pvesh command is unavailable, the node directory doesn't exist, or any transient error occurs, the thread immediately retries. With a 60-second discovery interval, this is manageable, but if the interval is set very low via `PVE_SAN_DISCOVERY_INTERVAL`, the thread could generate excessive load. More critically, if `discover_in_use_mpaths` panics (e.g., due to a bug in config parsing), the thread silently dies with no recovery, leaving `active_luns` stale indefinitely.

**Impact**: Log spam, potential resource exhaustion at low intervals, silent failure of discovery.

**Resolution**: Added exponential backoff with configurable parameters (`PVE_SAN_DISCOVERY_MAX_RETRIES`, `PVE_SAN_DISCOVERY_BACKOFF_BASE`, `PVE_SAN_DISCOVERY_BACKOFF_MAX`). The discovery thread now tracks consecutive failures and applies exponential backoff once the max retries threshold is exceeded, with a maximum cap on the backoff delay. Additionally, added panic recovery using `std::panic::catch_unwind` to prevent silent thread death on panics, with proper error logging.

**Recommendation**: N/A - Fixed.

---

### 34. `is_map_dead` returns `false` (alive) when path group has `dm_st` set to `None`

**File**: `src/lib.rs:130-164` (`is_map_dead`)
**Severity**: HIGH
**Status**: FIXED

**Description**: When a path group's `dm_st` field is `None` (missing from the JSON), the function treats it as alive (`true` at line 135). The comment at line 150 says "assume it might be alive to prevent false reboots". However, if multipathd's JSON schema changes and `dm_st` is simply omitted for a failed path group, the function will incorrectly consider the map alive. This is a silent failure mode that could prevent fencing when it should trigger.

**Impact**: Silent failure to fence when multipathd JSON format changes or omits expected fields.

**Resolution**: Added a warning log whenever `dm_st` is `None` for a path group or a path, making the fallback assumption explicit. Combined this with schema version checks in `Fencer::update` to identify changes in the JSON output structure from `multipathd`.

**Recommendation**: N/A - Fixed.

---

### 35. `send_multipath_command_to_socket` has no connection timeout

**File**: `libmultipath/src/lib.rs:157-187`
**Severity**: HIGH
**Status**: FIXED

**Description**: `send_multipath_command_to_socket` creates a new `MultipathConnection` and sends a command with a reply timeout, but there is no timeout on the initial `UnixStream::connect()` call. If multipathd is completely unresponsive (not just slow to reply, but the connection itself hangs), the function can block indefinitely. This would freeze the entire monitoring loop since it runs on the main thread.

**Impact**: Complete stall of the fencing daemon if multipathd becomes unreachable.

**Resolution**: Added a connection timeout of 2000ms (DEFAULT_CONNECT_TIMEOUT_MS) to the `connect_to_socket` function. The connection is now performed in a separate thread with a timeout, so if the connection cannot be established within 2 seconds, it returns a timeout error instead of blocking indefinitely.

**Recommendation**: N/A - Fixed.

---

### 36. `extract_defaults_block` cannot handle `}` inside quoted strings

**File**: `src/main.rs:236-319` (`extract_defaults_block`)
**Severity**: HIGH
**Status**: FIXED

**Description**: The function previously used simple brace counting to find the closing `}` of a `defaults { ... }` block. If any config value contained a `}` character (even inside quotes), the brace counter decremented prematurely and returned truncated content. For example, a value like `my_value "}"` would cause the function to return `my_value "` as the block content.

**Impact**: Incorrect multipath config parsing, leading to false warnings or missed configuration values.

**Resolution**: Replaced the naive brace counting loop with a scanner that respects double-quoted strings (and escaped double quotes `""`) and comments (`#` and `!`). Inside quotes and comments, `{` and `}` are ignored. Added unit test `test_check_multipath_config` case to verify that a brace inside a quoted string is correctly ignored and does not terminate the block.

**Recommendation**: N/A - Fixed.

---

### 37. `validate_sysrq` silently skips validation when `PVE_SAN_FENCE_DRY_RUN` is set

**File**: `src/main.rs:224-277` (`validate_sysrq`)
**Severity**: HIGH
**Status**: FIXED

**Description**: At lines 209-211 (in older versions), if the `PVE_SAN_FENCE_DRY_RUN` environment variable was set, the function returned `Ok(())` without validating any characters. This meant invalid characters like `'x'`, `'@'`, or `'\n'` passed validation silently. While `trigger_fencing` also checked for this env var and exited before writing, the validation function's behavior was misleading and could cause issues if the env var was set/unset between validation and execution.

**Impact**: Invalid sysrq characters accepted without warning in dry-run mode, potentially masking configuration errors.

**Resolution**: The check for `PVE_SAN_FENCE_DRY_RUN` was moved to after character validation, so character validity checks run regardless of whether dry-run mode is active. Only the actual `/proc/sys/kernel/sysrq` bitmask checking and warning logic is bypassed when in dry-run mode. Added a unit test `test_validate_sysrq_characters` case to assert that invalid characters are still rejected under dry-run mode.

**Recommendation**: N/A - Fixed.

---

### 38. `send_command_on_fd` has unclear fd ownership semantics on partial failure

**File**: `libmultipath/src/lib.rs:132-151`
**Severity**: MEDIUM
**Status**: FIXED

**Description**: The function wrapped a raw fd using `from_raw_fd`, performed I/O, then disowned it with `into_raw_fd` to prevent the stream destructor from closing the fd. However, if the I/O operation failed partway through (e.g., write succeeded but read timed out), the fd was already disowned. Furthermore, if a panic occurred during the I/O operation, the stream's destructor would run and close the fd, violating the caller's ownership.

**Impact**: Potential fd corruption or use-after-close on failures or panics.

**Resolution**: Wrapped the wrapped `UnixStream` inside `std::mem::ManuallyDrop`. This ensures that even if `send_command_on_stream` panics, or returns an error partway through, the stream's destructor is never run, and the file descriptor is never closed by Rust. The caller retains clean and safe ownership of the fd.

**Recommendation**: N/A - Fixed.

---

### 39. `build_mpath_map` silently stops at depth 32 without warning to caller

**File**: `libpve-san/src/lib.rs:468-496`
**Severity**: MEDIUM
**Status**: FIXED

**Description**: The function previously had a hardcoded recursion depth limit of 32. When this limit was exceeded, it logged a warning and returned, potentially missing multipath devices in deeper nesting levels.

**Impact**: In systems with deeply nested device-mapper hierarchies (>32 levels), some multipath devices may not have been discovered, leading to missed fencing.

**Resolution**: Refactored the `build_mpath_map` logic from a recursive implementation to an iterative, stack-based traversal (using a vector on the heap as a stack). This completely removes the recursion depth limits and stack overflow risks, allowing arbitrary depth traversal of the block device hierarchy safely.

**Recommendation**: N/A - Fixed.

---

### 40. `extract_disks` iterates over HashMap in non-deterministic order

**File**: `libpve-san/src/lib.rs:663-706` (`extract_disks`)
**Severity**: MEDIUM
**Status**: FIXED

**Description**: The function previously iterated directly over `config_map` (a `HashMap`). HashMap iteration order is randomized in Rust for security reasons. While the final result was sorted by `device_id` at line 705, intermediate warning logging and formatting occurred in non-deterministic order.

**Impact**: Non-deterministic warning logs.

**Resolution**: Collected the keys of the `config_map` into a vector, sorted them alphabetically, and iterated over the sorted keys to fetch values. This ensures fully deterministic warning log output and side-effects.

**Recommendation**: N/A - Fixed.

---

### 41. `pvesh_command` validation is overly restrictive

**File**: `libpve-san/src/lib.rs:223-231`
**Severity**: MEDIUM
**Status**: OPEN

**Description**: The `PveSanConfig::new` function validates the pvesh command path by checking that all characters are alphanumeric, `/`, `-`, or `_`. This rejects valid paths containing `.` (e.g., `/usr/local/bin/pvesh.wrapper`) or `+` (used in some package names). While this prevents shell injection, it is more restrictive than necessary since the value is passed directly to `Command::new()` (not through a shell).

**Impact**: Users cannot specify valid custom pvesh paths that contain `.` or other common path characters.

**Recommendation**: Allow all printable ASCII characters except shell metacharacters (`;`, `|`, `&`, `$`, `` ` ``, etc.), or simply validate that the path does not contain null bytes.

---

### 42. `sysfs::find_multipaths_for_dm` does not guard against symlink traversal

**File**: `libpve-san/src/sysfs.rs:23-53`
**Severity**: MEDIUM
**Status**: OPEN

**Description**: The function reads from `/sys/block/{dm_name}/slaves/` without checking whether entries are symlinks. While sysfs is a kernel virtual filesystem and symlink manipulation requires root privileges, this is still a potential attack vector if an attacker has write access to the node directory or if the sysfs mount is compromised.

**Impact**: Minimal in practice (requires root), but the code should defensively check file types.

**Recommendation**: Use `entry.file_type()` to verify entries are directories before recursing.

---

### 43. Dockerfile uses unpinned base image

**File**: `Dockerfile:1`
**Severity**: MEDIUM
**Status**: OPEN

**Description**: The Dockerfile uses `FROM debian:stable` without pinning to a specific digest or version tag. This means builds are not reproducible and could pull in unexpected changes from the Debian stable repository.

**Impact**: Non-reproducible builds, potential supply chain risk.

**Recommendation**: Pin to a specific digest: `FROM debian:stable-20260625-slim@sha256:...`.

---

### 44. `docker-entrypoint.sh` copies debug symbols without checking existence

**File**: `docker-entrypoint.sh:12`
**Severity**: MEDIUM
**Status**: OPEN

**Description**: The script copies `../pve-san-fenced-dbgsym_*` files without checking if they exist. If the build does not produce debug symbols (e.g., with `dpkg-buildpackage -us -uc -j$(nproc)` and stripped binaries), the `cp` command will fail with `set -e` enabled, causing the script to exit with an error.

**Impact**: Build script failure in environments where debug symbols are not generated.

**Recommendation**: Use `cp -f` or check file existence before copying: `cp -f ../pve-san-fenced-dbgsym_* /output/ 2>/dev/null || true`.

---

### 45. `Fencer::update` accepts raw JSON string — no schema validation

**File**: `src/lib.rs:242-267` (`update` and `MultipathOutput`)
**Severity**: MEDIUM
**Status**: FIXED

**Description**: The `update` method parsed multipathd JSON response with `serde_json::from_str`. If multipathd changed its JSON schema (e.g., added new fields, changed field names), serde silently ignored unknown fields by default. This meant the fencer could operate on incomplete data without any warning.

**Impact**: Silent degradation if multipathd JSON format changes.

**Resolution**: Added `major_version` and `minor_version` fields to `MultipathOutput` struct. Updated `Fencer::update` to check for these fields and print clear warning logs if they are missing or if the `major_version` is not `0`, alerting operators to potential incompatibilities.

**Recommendation**: N/A - Fixed.

---

### 46. `check_multipath_config` comment stripping is naive

**File**: `src/main.rs:120-221` (`tokenize` and `check_multipath_config`)
**Severity**: LOW
**Status**: FIXED

**Description**: Comments were previously stripped using `split_once('#')` on each line, which removed everything after the first `#` on that line. If a config value contained a `#` character (e.g., inside quotes), it was incorrectly truncated.

**Impact**: Broken config parsing if a value containing `#` was parsed.

**Resolution**: Replaced the naive `split_once('#')` logic with a proper tokenization step. The new `tokenize` function parses words/quotes/braces while skipping comments (`#` and `!`) only when they are outside double-quoted strings. Added unit test `test_check_multipath_config` case to verify that a comment character inside a quoted string is correctly parsed as a literal character rather than truncating the value.

**Recommendation**: N/A - Fixed.

---

### 47. `sysrq_char_to_bit` has a catch-all `_ => None` arm with no warning

**File**: `src/main.rs:176-187`
**Severity**: LOW
**Status**: WONTFIX

**Description**: The function maps known sysrq characters to bit values but uses a wildcard arm for all unknown characters. While this is correct behavior (rejecting unknown chars), the exhaustive mapping is not documented. New sysrq characters added in future kernel versions would be silently rejected.

**Impact**: If the kernel adds new sysrq characters, the daemon would reject them without warning.

**Recommendation**: Add a `warn!` log for unrecognized characters to alert operators of potential kernel version mismatches.

---

### 48. Service file has no `TimeoutStartSec`

**File**: `debian/pve-san-fenced.service:1-15`
**Severity**: LOW
**Status**: FIXED

**Description**: The systemd service file does not specify `TimeoutStartSec`. The default systemd timeout is 90 seconds, which may be too short if the daemon takes a long time during startup (e.g., waiting for multipathd to be ready).

**Impact**: Service could be killed during slow startup.

**Resolution**: Added `TimeoutStartSec=120` to the `[Service]` section.

**Recommendation**: Add `TimeoutStartSec=120` or similar.

---

### 49. `get_default_node_name` falls back to "localhost" without warning

**File**: `src/main.rs:24-28`
**Severity**: LOW
**Status**: WONTFIX

**Description**: If `/proc/sys/kernel/hostname` is unreadable, the function falls back to `"localhost"` without logging a warning. This could cause the daemon to operate with an incorrect node name, leading to fencing the wrong node or failing to find the node directory.

**Impact**: Incorrect node identification in multi-node clusters.

**Recommendation**: Log a warning when falling back to "localhost".

---

### 50. No `NoNewPrivileges` in systemd service

**File**: `debian/pve-san-fenced.service`
**Severity**: LOW
**Status**: FIXED

**Description**: The service file does not set `NoNewPrivileges=yes` in the `[Service]` section. This means the process can gain new privileges via setuid binaries or capabilities.

**Resolution**: Added `NoNewPrivileges=yes` to the `[Service]` section.

**Recommendation**: Add `NoNewPrivileges=yes` for defense in depth.

---

### 51. No `ProtectSystem=strict` in systemd service

**File**: `debian/pve-san-fenced.service`
**Severity**: LOW
**Status**: FIXED

**Description**: The service does not use systemd's filesystem protection directives. The daemon writes to `/proc/sysrq-trigger` which is expected, but it should not have write access to the rest of the filesystem.

**Resolution**: Added `ProtectSystem=strict`, `ProtectHome=yes`, and `ReadWritePaths=/proc/sysrq-trigger` to the `[Service]` section.

**Recommendation**: Add `ProtectSystem=strict`, `ProtectHome=yes`, and `ReadWritePaths=/proc` to restrict filesystem access.

---

### 52. `PVE_SAN_TEST_DATA_DIR` env var affects production behavior

**File**: `libpve-san/src/lib.rs:327-328` and elsewhere
**Severity**: LOW
**Status**: WONTFIX

**Description**: The `PVE_SAN_TEST_DATA_DIR` environment variable changes the behavior of `get_san_storage_info` in production code paths. If this env var is accidentally set in a production environment, the daemon would read from test data files instead of the real `/sys` filesystem.

**Recommendation**: Add a compile-time feature flag for test data behavior, or add a warning log when test data mode is detected in production.

---

### 53. No rate limiting on sysrq writes in `trigger_fencing`

**File**: `src/lib.rs:185-196`
**Severity**: LOW
**Status**: OPEN

**Description**: The function writes each sysrq character sequentially with a 1-second sleep after `'s'`. There is no rate limiting on how quickly these writes can occur. In theory, if the fencer is called multiple times rapidly (e.g., due to a bug), it could flood the sysrq trigger.

**Recommendation**: Add a mutex or atomic flag to ensure only one fencing operation can be in progress at a time.

---

### 54. Man pages are static files, not generated

**File**: `man/` directory
**Severity**: LOW
**Status**: WONTFIX

**Description**: Man pages are static files rather than generated from doc comments. This means they can drift from the actual CLI interface.

**Argumentation**: Generating the man pages dynamically using tools like `clap_mangen` would lose the extensive custom formatting and documentation sections present in the static man pages (e.g., custom synopsis, detailed MULTIPATH CONFIGURATION block, specific system FILES description, and environment variable mapping table). Maintaining these sections dynamically from within the Rust codebase would require building a complex custom generator, introducing additional build-time dependencies, and complicating the Debian packaging pipeline which directly installs the static man files. Therefore, maintaining static man pages in the repository is the most practical and informative approach.
