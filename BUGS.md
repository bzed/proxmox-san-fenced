# Bug Report — pve-san-fenced

All findings below were verified against AGENTS.md, PLAN.md, and general security best practices.
Each entry includes the file, line, description, and severity.

---

## 1. Trailing Whitespace (AGENTS.md violation)

AGENTS.md states: "Avoid trailing whitespaces. Empty line should only have a linebreak and not be filled with spaces."

| File | Line(s) |
|------|---------|
| `libmultipath/src/lib.rs` | 54, 56, 64, 66, 69, 71, 78, 80, 84, 86, 102, 104, 156, 164, 316, 318, 320, 322, 324, 332, 334, 336, 339, 341, 349, 351, 354, 356, 364, 366, 370, 372 |
| `libpve-san/src/lib.rs` | 275, 293, 451, 572, 577, 582, 593, 600, 603, 608, 612 |
| `tools/mpath-mockd/src/main.rs` | 133, 138, 140, 161, 168, 177, 185, 200, 217, 221, 290, 297, 306 |
| `tools/pvesh-mock/src/main.rs` | 71, 77, 113, 120, 131, 156, 160, 166, 177, 181 |
| `tools/mpath-query/tests/integration_test.rs` | 39, 46, 51, 63, 69, 72, 79, 84, 91, 94, 110, 115, 125, 128, 145, 150, 157, 160, 174, 179, 185, 188, 202, 207, 214, 217, 231, 236, 246, 249 |
| `tools/mpath-mockd/tests/daemon_test.rs` | 36, 43, 48, 60, 66, 69, 76, 78, 86, 91, 99, 102, 117, 122, 130, 133, 148, 153, 165, 169, 186, 194, 206, 213, 216, 218, 238, 246, 257, 264, 267, 269, 277, 280, 295, 307, 315, 326, 333, 336, 338, 346, 349, 365, 370, 379, 387, 392, 400, 420, 428, 439, 446, 449, 451, 461, 465, 468, 474, 484, 489, 499, 503, 506, 511, 513 |
| `tools/pve-san-query/tests/integration_test.rs` | 55, 68, 73, 76, 83, 88, 91, 103, 107, 114, 118, 122, 125, 134, 137, 139, 143, 153, 158, 161, 168, 171, 176, 179, 184, 188, 199, 201, 205 |
| `libpve-san/tests/integration_test.rs` | 53, 66, 71, 73, 82, 84, 88, 91, 95, 107, 109, 113, 116, 120, 128, 133, 138, 140, 154, 160, 164, 167, 171, 183, 189, 192, 197, 209, 215, 218, 223, 236, 242, 245, 249, 261, 267, 270, 274, 287, 301, 306, 308, 321, 335 |

---

## 2. Clippy Warnings

### 2.1 `manual_strip` — strip prefix manually (AGENTS.md: inline format! args convention)

| File | Line | Detail |
|------|------|--------|
| `libmultipath/src/lib.rs` | 159-160 | `socket_path.starts_with('@')` followed by `&socket_path[1..]` should use `strip_prefix('@')` |
| `tools/mpath-mockd/src/main.rs` | 292-293 | Same pattern — `starts_with('@')` + `&socket_path[1..]` should use `strip_prefix('@')` |

### 2.2 `unnecessary_map_or` — use `is_ok_and` instead

| File | Line |
|------|------|
| `tools/mpath-mockd/src/main.rs` | 182 |
| `tools/mpath-mockd/tests/daemon_test.rs` | 45, 191, 243, 312, 425 |
| `tools/mpath-query/tests/integration_test.rs` | 48 |

### 2.3 `ptr_arg` — write `&PathBuf` instead of `&Path`

| File | Line | Detail |
|------|------|--------|
| `tools/pvesh-mock/src/main.rs` | 152 | `handle_ls_nodes_qemu` parameter `test_data_dir: &PathBuf` should be `&Path` |
| `tools/pvesh-mock/src/main.rs` | 170 | `handle_get_vm_config` parameter `test_data_dir: &PathBuf` should be `&Path` |

### 2.4 `manual_pattern_char_comparison` — use char array

| File | Line | Detail |
|------|------|--------|
| `libpve-san/src/lib.rs` | 453 | `|c| c == 'K' || c == 'B'` should be `['K', 'B']` |
| `libpve-san/src/lib.rs` | 456 | `|c| c == 'M' || c == 'B'` should be `['M', 'B']` |
| `libpve-san/src/lib.rs` | 459 | `|c| c == 'G' || c == 'B'` should be `['G', 'B']` |
| `libpve-san/src/lib.rs` | 462 | `|c| c == 'T' || c == 'B'` should be `['T', 'B']` |

### 2.5 `needless_borrows_for_generic_args`

| File | Line | Detail |
|------|------|--------|
| `tools/pve-san-query/tests/integration_test.rs` | 80 | `.current_dir(&workspace_root())` should be `.current_dir(workspace_root())` |
| `tools/pve-san-query/tests/integration_test.rs` | 164 | `.args(&[...])` should be `.args([...])` |
| `tools/pve-san-query/tests/integration_test.rs` | 165 | `.current_dir(&workspace_root())` should be `.current_dir(workspace_root())` |

### 2.6 `len_zero` — use `!is_empty()` instead of `.len() > 0`

| File | Line | Detail |
|------|------|--------|
| `libpve-san/tests/integration_test.rs` | 94 | `vms.len() > 0` should be `!vms.is_empty()` |
| `libpve-san/tests/integration_test.rs` | 139 | `running_vms.len() > 0` should be `!running_vms.is_empty()` |

### 2.7 `unused_imports`

| File | Line | Detail |
|------|------|--------|
| `tools/pve-san-query/tests/integration_test.rs` | 23 | `Stdio` is imported but never used in this file |

---

## 3. `mem::forget` on `File` — double-close risk (Security / Correctness)

Both `libmultipath` and `mpath-mockd` use `std::fs::File::from_raw_fd` followed by `std::mem::forget` on every I/O call. This is a known anti-pattern:

- When `from_raw_fd` creates a `File`, it takes ownership of the fd. `forget` prevents the `File`'s `Drop` from closing it — which is intentional to avoid double-closing the underlying fd.
- However, if an I/O error occurs **between** `from_raw_fd` and `forget`, the `File` may be dropped and the fd closed prematurely, causing subsequent operations on the same fd to fail.
- The code attempts to handle this by calling `forget` in every branch (success, error, and early-return paths), but this is fragile: any new code path added between `from_raw_fd` and `forget` that drops the `File` will silently close the fd.

| File | Lines |
|------|-------|
| `libmultipath/src/lib.rs` | 203-205, 208-210, 222-246, 264-288 |
| `tools/mpath-mockd/src/main.rs` | 338-360, 373-395, 443-468 |

**Recommendation**: Use a wrapper type or RAII guard that only forgets the fd on success, and closes it on error. Or use `std::os::unix::net::UnixStream` / `TcpStream` which handles this safely.

---

## 4. No input validation on `cmd_len` in `mpath-mockd` (Security)

In `tools/mpath-mockd/src/main.rs:363`, the command length read from the socket is cast directly to `usize` with no bounds check:

```rust
let cmd_len = u64::from_le_bytes(len_bytes) as usize;
```

A malicious client could send an extremely large `cmd_len` (e.g., `u64::MAX`), causing the daemon to allocate a massive buffer (`vec![0u8; cmd_len]`) and potentially OOM. The `libmultipath` side has a `MAX_REPLY_LEN` guard, but the mock daemon has none.

---

## 5. No input validation on `reply_len` upper bound in `libmultipath` (Security)

In `libmultipath/src/lib.rs:252`, the reply length validation uses `reply_len >= MAX_REPLY_LEN`. This means a reply of exactly `MAX_REPLY_LEN` bytes (32 MB) is rejected, but a reply of `MAX_REPLY_LEN - 1` bytes is accepted. The C implementation checks `len <= 0 || len >= MAX_REPLY_LEN`, so the Rust implementation is slightly off — it should use `>` instead of `>=` to match the C behavior, or the constant should be adjusted.

Additionally, `reply_len == 0` is rejected, but a legitimate empty reply (length 0 with just a null terminator) from multipathd would be silently dropped rather than handled gracefully.

---

## 6. `eprintln!` used for logging instead of the `log` crate

The `libpve-san` library uses `eprintln!` at line 224 for warning about VM config failures:

```rust
eprintln!("Warning: Failed to get config for VM {}: {}", vmid, e);
```

AGENTS.md lists `log = "0.4"` as a dependency of `libpve-san`, but it is unused. The library should use the `log` crate (e.g., `log::warn!`) for production logging so that log output can be controlled by the application.

---

## 7. `tokio::runtime::Builder::new_current_thread().enable_all()` (Performance / Correctness)

Both `get_san_storage_info_sync` and `get_san_storage_info_sync_with_pvesh` in `libpve-san/src/lib.rs` (lines 499-506 and 514-520) create a new current-thread Tokio runtime with `enable_all()` on every call. This:

- Enables I/O, time, and signal features that may not be needed.
- Is expensive to construct repeatedly.
- Could conflict with an existing Tokio runtime if the sync function is called from within an async context.

**Recommendation**: Use `tokio::runtime::Handle::try_current()` to detect if we're already in a runtime, or use `tokio::runtime::Builder` with only the features needed (`rt` and `macros`).

---

## 8. Hardcoded socket paths in tests (fragile)

Test files use hardcoded relative paths to binaries:

| File | Line | Detail |
|------|------|--------|
| `tools/mpath-mockd/tests/daemon_test.rs` | 21 | `DAEMON_PATH = "../../target/release/mpath-mockd"` |
| `tools/mpath-query/tests/integration_test.rs` | 24 | `MOCKD_PATH = "../../target/release/mpath-mockd"` |

These paths are release-only. Tests will fail if built in debug mode, or if the workspace layout changes. The `pve-san-query` tests correctly use `CARGO_MANIFEST_DIR` for path resolution, but the `mpath-*` tests do not.

---

## 9. `handle_connection` in `mpath-mockd` clones `HashMap` per connection (Performance)

At line 260 in `tools/mpath-mockd/src/main.rs`, `command_responses` (a `HashMap<String, Vec<String>>`) is cloned for every incoming connection:

```rust
let command_responses = command_responses.clone();
```

The `HashMap` is passed by value into the spawned thread. Since the main loop is a `loop` that never exits, this means every connection handler gets its own full copy of the entire command-response map. For large test data sets, this wastes memory.

**Recommendation**: Wrap in `Arc<Mutex<...>>` or `Arc<RwLock<...>>` to share the data across threads.

---

## 10. `FileCounters` mutex contention in `mpath-mockd` (Correctness)

The `FileCounters::next_index` method at line 80-86 locks the mutex, reads the current index, increments it, and returns. However, the increment happens **inside** the lock, and the returned value is the **old** index. This is correct for round-robin cycling, but the mutex is held for the entire duration of the function call including the return, which means the lock is held while the caller processes the response. This could cause blocking in high-concurrency scenarios.

---

## 11. `convert_block_device` sets `size: 0` (Bug)

In `libpve-san/src/lib.rs:372-403`, the `convert_block_device` method sets `size: 0` unconditionally (line 394). The `BlockDevice` from the `lsblk` crate has a `size` field that should be used. This means all block device size information is lost.

---

## 12. `extract_disks` iterates over `HashMap` — non-deterministic order (Correctness)

In `libpve-san/src/lib.rs:315`, `extract_disks` iterates over a `HashMap<String, String>`. Since `HashMap` has non-deterministic iteration order, the order in which disks are added to the `Vec<VmDisk>` is non-deterministic. This is not a correctness bug per se (the data is the same), but it means tests that depend on disk ordering will be flaky.

The test at `libpve-san/tests/integration_test.rs:194-196` already works around this by using `.find()` rather than index-based access, which is the correct approach.

---

## 13. `parse_size` does not handle decimal values (Correctness)

The `parse_size` function in `libpve-san/src/lib.rs:449-467` parses size strings like "10G" but does not handle decimal values like "10.5G". The `num.parse::<u64>()` will fail on "10.5". This may cause silent data loss for fractional disk sizes.

---

## 14. `parse_vm_config` JSON fallback uses `.unwrap_or_default()` (Silent failure)

In `libpve-san/src/lib.rs:286`, when converting a JSON value that doesn't match the explicit arms (array, object, etc.):

```rust
_ => serde_json::to_string(value).unwrap_or_default(),
```

If `serde_json::to_string` fails (which is unlikely for `serde_json::Value`), the value becomes an empty string. This silently drops data.

---

## 15. `pvesh-mock` uses `.unwrap()` on `io::stdout().write_all()` (Silent failure)

In `tools/pvesh-mock/src/main.rs`, lines 135, 138, and 141 use `.unwrap()` on `write_all`:

```rust
io::stdout().write_all(pretty.as_bytes()).unwrap();
io::stdout().write_all(json.as_bytes()).unwrap();
```

If stdout is broken (e.g., the pipe is closed), the process will panic with a stack trace instead of exiting cleanly with an error message.

---

## 16. `pvesh-mock` `output_format` default is "json" but not documented

In `tools/pvesh-mock/src/main.rs:119`:

```rust
let output_format = cli.output_format.as_deref().unwrap_or("json");
```

The `output_format` argument is `Option<String>`, and the default is "json". However, the `--output-format` flag is not marked as optional with a default in clap, and the CLI help doesn't document the default. Users may not realize "json" is assumed.

---

## 17. `run_pvesh` checks `--version` to determine availability (Fragile)

In `libpve-san/src/lib.rs:418`:

```rust
if Command::new(&self.config.pvesh_command).arg("--version").output().is_err() {
    return Err(PveSanError::PveshNotFound);
}
```

This spawns a subprocess just to check if `pvesh` exists. A better approach would be to use `which` or check the PATH. Additionally, if `pvesh` exists but `--version` is not supported, it would still return `PveshNotFound` even though the command exists.

---

## 18. `PveSanConfig` fields are `pub` (Design)

Both `node` and `pvesh_command` in `PveSanConfig` are `pub` (lines 169, 173 in `libpve-san/src/lib.rs`). AGENTS.md says "Prefer private modules and explicitly exported public crate API." Making these fields `pub` allows callers to construct `PveSanConfig` directly and bypass the validation in `PveSanClient::new()` (which checks for empty node).

---

## 19. `mpath-query` verbose mode prints socket path to stderr (Information disclosure)

In `tools/mpath-query/src/main.rs:89`:

```rust
eprintln!("Connecting to socket: {}", cli.socket);
```

When connecting to a mock socket (e.g., `@/tmp/test-mpath-mockd-...`), this reveals the socket path including the process ID, which could be useful for an attacker to target the mock daemon. In production, the default socket is the real multipathd socket, so this is less of a concern, but the verbose flag should be used cautiously.

---

## 20. No `--help` or `--version` in test data verification (Test quality)

The integration tests for `pve-san-query` and `libpve-san` do not verify that the tool's `--help` output or `--version` flag works correctly. This is a minor test coverage gap.

---

## 21. `pve-san-query` and `mpath-query` write to arbitrary file paths (Security)

Both tools accept an `--output` argument that writes to an arbitrary file path. There is no validation on the path — a user could accidentally overwrite system files or files belonging to other users. While this is common for CLI tools, it's worth noting from a security perspective.

---

## 22. `mpath-mockd` test data auto-load sorts by filename (Determinism concern)

In `tools/mpath-mockd/src/main.rs:184`, files are sorted by filename:

```rust
sorted_entries.sort_by_key(|e| e.file_name());
```

This ensures deterministic ordering for cycling, which is good. However, the sort is locale-dependent on some systems. Using `sort_unstable_by` with byte comparison would be more portable.

---

## 23. `libmultipath` does not validate that `fd` is valid in `send_command_on_fd` (Correctness)

The static method `send_command_on_fd` at line 88 accepts any `i32` without validating it. A caller could pass `-1` or an invalid fd, which would cause undefined behavior in the subsequent `from_raw_fd` calls.

---

## 24. `send_multipath_command_with_timeout` and `send_multipath_command_to_socket_with_timeout` always wrap `timeout_ms` in `Some()` (Design)

In `libmultipath/src/lib.rs:343-345` and `374-380`:

```rust
conn.send_command(command, Some(timeout_ms))
```

The `timeout_ms` parameter is always wrapped in `Some()`, meaning these convenience functions never support "no timeout". The `Option<u64>` on `send_command` allows `None` for no timeout, but the convenience wrappers make this impossible to use.

---

## 25. `mpath-mockd` default response for unknown commands is hardcoded JSON (Design)

In `tools/mpath-mockd/src/main.rs:434`:

```rust
r#"{"error": "unknown command"}"#.to_string()
```

This response is returned for any unknown command when no responses are loaded at all. However, the more common case at line 422-429 falls back to "show maps json" responses for unknown commands, which may mask the fact that the command is unrecognized.

---

## 26. `mpath-mockd` `conn_fd` is not closed on early returns in `handle_connection`

In `handle_connection` (lines 327-474), there are multiple early returns where `close(conn_fd)` is called. However, the `mem::forget` pattern used for reading means that if `from_raw_fd` succeeds but `forget` is not reached (e.g., a panic in a `match` arm), the fd could be double-closed. The code does handle this in every branch, but the pattern is fragile.

---

## 27. `lsblk` crate dependency not in workspace dependencies

The `lsblk = "0.6.1"` dependency is listed in workspace dependencies (`Cargo.toml:8`) but is only used by `libpve-san`. The workspace dependency is not referenced via `[workspace.dependencies]` path in `libpve-san/Cargo.toml` — it uses a direct version. This means the workspace dependency is not actually being used.

---

## 28. `tokio` features mismatch

The workspace defines `tokio = { version = "1.0", features = ["rt", "sync", "macros"] }` in `Cargo.toml:9`, but:
- `libpve-san` uses `tokio::runtime::Builder` which requires the `rt-multi-thread` or at least `rt` feature — `rt` is included, so this is fine.
- `pve-san-query` uses `tokio` features `["rt", "macros"]` directly in its Cargo.toml rather than inheriting from workspace.

---

## 29. `libpve-san` `convert_block_device` uses `device.id` for serial number (Correctness)

In `libpve-san/src/lib.rs:400`:

```rust
serial: device.id.clone(),
```

The `lsblk` crate's `id` field is a string that may contain various identifiers (not necessarily a serial number). Using it as the serial number may produce incorrect results. The `lsblk` crate typically has a `serial` field that should be used instead.

---

## 30. `libpve-san` `convert_block_device` `device_type` logic is incomplete (Correctness)

In `libpve-san/src/lib.rs:376-382`:

```rust
let device_type = if device.partuuid.is_some() || device.partlabel.is_some() {
    "part".to_string()
} else if device.uuid.is_some() {
    "disk".to_string()
} else {
    "unknown".to_string()
};
```

This logic does not handle other valid device types like `lvm`, `raid`, `loop`, `rom`, `crypt`, `swap`, etc. All non-partition, non-UUID devices are classified as "unknown".
