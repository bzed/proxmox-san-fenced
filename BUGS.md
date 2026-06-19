# Bug Report — pve-san-fenced (Resolved)

All findings below were verified against AGENTS.md, PLAN.md, and general security best practices.
Each entry includes the file, line, description, severity, and resolution status.

---

## CRITICAL Issues

### 1. Missing `libmultipath` integration test file

**File**: `libmultipath/tests/integration_test.rs` (missing)
**Severity**: CRITICAL
**Status**: FIXED
**Resolution**: Created `libmultipath/tests/integration_test.rs` to start `mpath-mockd` as a daemon on a temporary Unix domain socket, verify commands, and test connection/timeout handling.

---

### 2. `PveSanConfig::Default` produces invalid state

**File**: `libpve-san/src/lib.rs:215-222`
**Severity**: CRITICAL
**Status**: FIXED
**Resolution**: Removed `impl Default for PveSanConfig` completely to prevent callers from silently creating a configuration with an invalid (empty) node name.

---

## HIGH Issues

### 3. Test expects stderr message that does not exist in `mpath-query`

**File**: `tools/mpath-query/tests/integration_test.rs:273-274`
**Severity**: HIGH
**Status**: FIXED
**Resolution**: Added verbose logging message `"Connecting to socket"` to `tools/mpath-query/src/main.rs` when verbose output is enabled. Changed tests to dynamically run target debug binaries instead of hardcoded target release binaries.

---

### 4. Two separate mechanisms for dry-run/test-mode fencing control

**File**: `src/lib.rs:192-195` and `src/main.rs:149-153`
**Severity**: HIGH
**Status**: WONTFIX
**Argumentation**: Kept intentionally. `--test-mode` is a CLI-level diagnostic flag that prevents the daemon loop from executing fencing, whereas `PVE_SAN_FENCE_DRY_RUN` is a library-level safety net inside `trigger_fencing()` to prevent reboots/kernel panics during manual library usage even if the command-line test-mode flag was omitted.

---

### 5. `Fencer` struct exposes all fields publicly

**File**: `src/lib.rs:210-215`
**Severity**: HIGH
**Status**: FIXED
**Resolution**: Changed all fields of `Fencer` in `src/lib.rs` to private, and exposed read-only getter methods for `consecutive_failures` and `max_failures` (needed in the main daemon loop and tests).

---

### 6. Internal data types are publicly exported

**File**: `src/lib.rs:20-55`
**Severity**: HIGH
**Status**: FIXED
**Resolution**: Removed public export (`pub`) from internal deserialization targets (`LsblkDevice`, `LsblkOutput`, `MultipathOutput`, `PathGroup`, `MpathPath`). Kept `MultipathMap` `pub` because it is in the signature of `is_map_dead()`, but made its fields `pub(crate)` to avoid leaking private types like `PathGroup`. Moved all tests from `tests/daemon_tests.rs` into `src/lib.rs` under a `#[cfg(test)]` module so they can inspect private/crate-private types without public exposure.

---

### 7. Internal helper functions are publicly exported

**File**: `src/lib.rs:59-91`
**Severity**: HIGH
**Status**: FIXED
**Resolution**: Made `build_mpath_map` and `storage_to_dm_name` private. Moved all integration tests from `tests/daemon_tests.rs` into `src/lib.rs` under a `#[cfg(test)]` module, resolving the visibility requirement.

---

### 8. Sync wrappers create a new Tokio runtime on every call

**File**: `libpve-san/src/lib.rs:564-587`
**Severity**: HIGH
**Status**: FIXED
**Resolution**: Updated synchronous wrappers to check if a Tokio runtime is already active on the current thread using `tokio::runtime::Handle::try_current()`. If active, we reuse the existing runtime; otherwise, we build a new current-thread runtime.

---

## MEDIUM Issues

### 9. `PveSanConfig::new` takes `Option<String>` instead of `Option<&str>`

**File**: `libpve-san/src/lib.rs:188`
**Severity**: MEDIUM
**Status**: FIXED
**Resolution**: Changed the signature of `PveSanConfig::new` to accept `node: impl Into<String>` and `pvesh_command: Option<&str>` to avoid unnecessary string allocations.

---

### 10. `PveSanClient::with_node_and_pvesh` takes `String` instead of `&str`

**File**: `libpve-san/src/lib.rs:251`
**Severity**: MEDIUM
**Status**: FIXED
**Resolution**: Changed the signature to `with_node_and_pvesh(node: impl Into<String>, pvesh_command: &str)` to prevent unnecessary allocations.

---

### 11. `PveSanClient::with_node` takes `String` instead of `&str`

**File**: `libpve-san/src/lib.rs:245`
**Severity**: MEDIUM
**Status**: FIXED
**Resolution**: Changed the signature to `with_node(node: impl Into<String>)` to prevent unnecessary allocations.

---

### 12. `convert_block_device` hardcodes `size: 0` and drops fields

**File**: `libpve-san/src/lib.rs:432-465`
**Severity**: MEDIUM
**Status**: PARTIALLY FIXED / WONTFIX
**Resolution/Argumentation**: Block device size in bytes is now correctly populated using the `device.capacity()` API provided by the `lsblk` crate. The fields `parent`, `children`, `model`, and `mount_point` are marked WONTFIX because the `lsblk-rs` crate (version 0.6.1) does not provide this information on the `BlockDevice` struct.

---

### 13. Some workspace packages do not use `[workspace.dependencies]`

**File**: `libmultipath/Cargo.toml`, `tools/mpath-query/Cargo.toml`, `tools/mpath-mockd/Cargo.toml`
**Severity**: MEDIUM
**Status**: FIXED
**Resolution**: Changed the versions of all shared dependencies (e.g. `libc`, `clap`, `serde`, `serde_json`) in these package Cargo.toml files to use `{ workspace = true }`.

---

### 14. `PveSanClient::config()` returns `&PveSanConfig` exposing internal config

**File**: `libpve-san/src/lib.rs:501-503`
**Severity**: MEDIUM
**Status**: FIXED
**Resolution**: Removed the unused public `config(&self)` getter method entirely to prevent leaking the internal configuration object.

---

## LOW Issues

### 15. `parse_size` is a free function instead of a method

**File**: `libpve-san/src/lib.rs:506-541`
**Severity**: LOW
**Status**: FIXED
**Resolution**: Made `parse_size` a private associated function on `PveSanClient` to restrict its scope.

---

### 16. `PveSanClient::new` has a no-op validation comment

**File**: `libpve-san/src/lib.rs:239-242`
**Severity**: LOW
**Status**: FIXED
**Resolution**: Changed `PveSanClient::new(config: PveSanConfig)` to return `Self` directly instead of returning `PveSanResult<Self>` since configuration validation is already performed during `PveSanConfig` instantiation and cannot fail.

---

### 17. `PveSanConfig::with_node` method is a thin wrapper

**File**: `libpve-san/src/lib.rs:200-202`
**Severity**: LOW
**Status**: FIXED
**Resolution**: Updated `with_node` method to accept `impl Into<String>` to avoid String allocation.

---

### 18. `PveSanConfig::pvesh_command` has unnecessary `#[cfg_attr(not(test), allow(dead_code))]`

**File**: `libpve-san/src/lib.rs:173`
**Severity**: LOW
**Status**: FIXED
**Resolution**: Removed the unnecessary dead_code allowance attribute since the field is used to run Proxmox commands.

---

### 19. `get_san_storage_info` async free function has no doc comment

**File**: `libpve-san/src/lib.rs:543-550`
**Severity**: LOW
**Status**: FIXED
**Resolution**: Added a descriptive docstring to `get_san_storage_info`.

---

### 20. `handle_get_vm_config` ignores the `node` parameter

**File**: `tools/pvesh-mock/src/main.rs:178`
**Severity**: LOW
**Status**: FIXED
**Resolution**: Removed the unused `node` parameter from `handle_get_vm_config` signature and call sites to clean up the code.

---

## WONTFIX / INFO

### 21. `trigger_fencing()` uses `unsafe { libc::sync() }`

**File**: `src/lib.rs:187-189`
**Severity**: INFO (intentional)
**Status**: WONTFIX
**Argumentation**: The `unsafe` block around `libc::sync()` is intentional for a fencing daemon that needs to flush filesystem buffers before triggering a kernel panic. This is by design.

---

### 22. `PVE_SAN_FENCE_DRY_RUN` env var provides a secondary safety net

**File**: `src/lib.rs:192-195`
**Severity**: INFO (intentional)
**Status**: WONTFIX
**Argumentation**: Consolidated under Issue #4. The `PVE_SAN_FENCE_DRY_RUN` env var acts as a safety layer for manual library execution even when the daemon CLI is run without `--test-mode`.
