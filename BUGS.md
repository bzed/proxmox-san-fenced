# Bug Report — pve-san-fenced

All findings below were verified against AGENTS.md, PLAN.md, and general security best practices.
Each entry includes the file, line, description, severity, and resolution status.

---

## New Findings — 2026-06-30 Review

### 1. `parse_block` silently skips non-key-value tokens at depth 1

**File**: `src/config.rs:146-173`
**Severity**: HIGH
**Status**: RESOLVED

**Description**: The `parse_block` function processes tokens within a brace-delimited block. At line 158, it only processes `Token::Word(key)` when `depth == 1`. When it encounters such a key, it attempts to read the next token as a value: `if let Some(Token::Word(val)) = iter.next()` (line 159).

If the next token is NOT a `Word` (e.g., it is an `OpenBrace` for a nested block, or a `CloseBrace`), the pattern match fails, the value is silently skipped, and the loop continues. This means a key followed by a nested block (e.g., `some_block { nested key }`) will have `some_block` silently ignored, with no error or warning logged.

**Impact**: Silent ignoring of malformed or unexpected configuration entries, leading to missing configuration values and potentially incorrect multipath configuration warnings.

**Recommendation**: Add validation to log a warning when a key at depth 1 is not followed by a value Word token.

---

### 2. Regex compilation failures in `get_merged_config` are silently ignored

**File**: `src/config.rs:175-199`
**Severity**: HIGH
**Status**: RESOLVED

**Description**: In `get_merged_config`, lines 180 and 184-185 compile regex patterns from the `vendor` and `product` fields using `Regex::new(v).map(|re| re.is_match(vendor)).unwrap_or(false)`. If the regex pattern is invalid, `Regex::new(v)` returns an error, and `unwrap_or(false)` silently treats it as a non-match. This means an administrator could configure an invalid regex pattern, the pattern would silently fail to match, and the daemon would fall back to default configuration instead of the intended device-specific settings, with no warning or error logged.

**Impact**: Invalid regex patterns in multipath config are silently ignored, leading to unexpected configuration behavior where device-specific settings are not applied.

**Recommendation**: Log a warning when regex compilation fails rather than silently returning false.

---

### 3. `parse_block` treats missing values as next key's value

**File**: `src/config.rs:146-173`
**Severity**: HIGH
**Status**: RESOLVED

**Description**: When parsing a config like `defaults { vendor\n dev_loss_tmo "infinity" }`, the tokenizer produces: `[Word("vendor"), Word("dev_loss_tmo"), Word("infinity")]`. When `parse_block` processes `vendor` at line 158-159, it calls `iter.next()` which returns `Word("dev_loss_tmo")`. Since this IS a Word, it is treated as the value for `vendor`, resulting in `vendor = Some("dev_loss_tmo")`. The actual `dev_loss_tmo` key is then skipped entirely when the iterator advances.

**Impact**: In malformed config files where a key has no value, the next key's name is used as the value, and that key is then skipped entirely, causing incorrect configuration parsing.

**Recommendation**: After matching a key, verify that the next token is actually a value (not another key). Use lookahead or add validation to detect this case.

---

### 4. `is_map_dead` treats missing `dm_st` for paths as alive

**File**: `src/lib.rs:161-210`
**Severity**: MEDIUM
**Status**: RESOLVED

**Description**: In `is_map_dead`, when a path's `dm_st` field is `None` (line 189), the code logs a warning and sets `active_path_found = true` (line 197). This means a path with a missing `dm_st` field is treated as ALIVE (healthy). This conservative approach prevents false fencing, but it means we might NOT fence when we should if multipathd omits the `dm_st` field for genuinely failed paths.

**Impact**: If multipathd's JSON output omits `dm_st` for failed paths, the daemon will incorrectly consider them alive and will not trigger fencing, potentially causing data corruption on shared storage.

**Recommendation**: The current behavior is a reasonable safety tradeoff. Add code comments explaining the rationale. The existing warning log (line 191) and status issue tracking (lines 192-196) provide observability.

---

### 5. Status file write failures are not reported to callers

**File**: `src/status.rs:103-144`
**Severity**: MEDIUM
**Status**: WONTFIX

**Description**: In `write_status_file` (lines 103-144), the status content is written to a temporary file and then atomically renamed to the final status file. This is done in a spawned thread (line 132) to avoid blocking. If the write fails (line 135) or the rename fails (line 139), an error is logged (lines 136, 140), the temp file is removed if rename fails (line 141), but the caller has no way to know the write failed. Callers like `set_issue`, `clear_issue`, `touch` all assume the write succeeded.

**Impact**: Status file might not reflect the current daemon state, causing Nagios/monitoring systems to see stale information.

**Recommendation**: Add a mechanism to track write failures and surface them through the status tracker itself, or use a channel to receive write acknowledgments.

---

### 6. `trigger_fencing` in dry-run mode may exit before status file is written

**File**: `src/lib.rs:216-224`
**Severity**: MEDIUM
**Status**: RESOLVED

**Description**: In `trigger_fencing`, when `PVE_SAN_FENCE_DRY_RUN` is set (line 219), the function logs a warning and sleeps for 200ms before exiting (lines 220-223). The sleep is intended to allow the status writing thread time to write the final CRITICAL state. However, the status file writing happens in a spawned thread with no synchronization. There is no guarantee that 200ms is sufficient, especially on slow systems or under heavy load.

**Impact**: In dry-run/test mode, the daemon might exit before the final CRITICAL status is written to the status file, causing monitoring systems to see OK or WARNING instead of CRITICAL.

**Recommendation**: Add explicit synchronization - either a flush/sync method on StatusTracker, increase the sleep time, or use a channel to signal when writes are complete.

---

### 7. `get_default_node_name` doesn't handle empty hostname

**File**: `src/main.rs:47-51`
**Severity**: LOW
**Status**: RESOLVED

**Description**: The function reads `/proc/sys/kernel/hostname`, trims it, and returns it. If the read fails, it returns "localhost". However, if the hostname file is empty or contains only whitespace, the trim would result in an empty string, which is then returned. An empty node name would cause the node directory check at lines 325-334 to fail.

**Impact**: If the hostname is empty, the daemon would fail to start with a confusing error about the node directory not existing.

**Recommendation**: Fall back to "localhost" if the trimmed hostname is empty.

---

### 8. Status file path is not validated for safety

**File**: `src/main.rs:106-116` and `src/status.rs:54-62`
**Severity**: LOW
**Status**: RESOLVED

**Description**: The `status_file` CLI argument accepts any string path without validation. A user could specify a relative path, a path outside `/run` or `/var/run`, or even a path that could overwrite system files. The daemon will attempt to write to whatever path is specified.

**Impact**: If a user specifies a dangerous path, the daemon could overwrite important system files or fail to write status information.

**Recommendation**: Add validation to ensure the path is absolute and in a safe directory, and that the parent directory exists.

---

### 9. `parse_multipathd_response` has hardcoded 10MB size limit

**File**: `src/lib.rs:29-51`
**Severity**: LOW
**Status**: RESOLVED

**Description**: In `parse_multipathd_response`, there is a hardcoded size limit of 10MB (line 30). If the multipathd response exceeds this, it is rejected with a warning. While this protects against memory exhaustion, 10MB might be too low for systems with thousands of multipath devices.

**Impact**: On systems with many multipath devices, valid responses might be rejected, causing the fencer to operate on no data and potentially miss fencing events.

**Recommendation**: Make the limit configurable via an environment variable with a reasonable default, or increase the default to 100MB.

---

### 10. No validation of `node_name` for path traversal characters

**File**: `src/main.rs:80-87`
**Severity**: LOW
**Status**: RESOLVED

**Description**: The `node_name` CLI argument accepts any string. This value is used to construct the node directory path at line 324. If the node name contains path traversal characters like `..` or `/`, it could cause the daemon to read from or operate on unintended directories.

**Impact**: Path traversal attack if a malicious user can control the node name argument.

**Recommendation**: Add validation to reject node names containing path separators or traversal sequences.

---

### 11. `PVE_SAN_TEST_DATA_DIR` environment variable affects production behavior

**File**: `libpve-san/src/lib.rs` (and elsewhere)
**Severity**: LOW
**Status**: RESOLVED

**Description**: The `PVE_SAN_TEST_DATA_DIR` environment variable is used by `libpve-san` to override the path to test data files. If this variable is set in production (accidentally or maliciously), the daemon would read mock data instead of real system information from `/etc/pve/` and `/sys/`.

**Impact**: If `PVE_SAN_TEST_DATA_DIR` is set in production, the daemon would use test data instead of real VM configurations and block device information, leading to incorrect fencing decisions.

**Recommendation**: Add a check in main.rs to detect and reject this in production (when test_mode is not enabled).

---

### 12. Dockerfile uses unpinned base image

**File**: `Dockerfile:1`
**Severity**: LOW
**Status**: WONTFIX

**Description**: The Dockerfile uses `FROM debian:stable` without pinning to a specific digest or version tag. This means builds are not reproducible and could pull in unexpected changes from the Debian stable repository over time.

**Impact**: Non-reproducible builds, potential supply chain risk from unexpected base image changes.

**Recommendation**: Pin to a specific digest or use a dated tag.

---

### 13. `sysrq_char_to_bit` doesn't include all kernel sysrq characters

**File**: `src/main.rs:136-147`
**Severity**: LOW
**Status**: WONTFIX

**Description**: The function maps sysrq characters to their bit values but only includes: s, b, o, c, u, r, e, i, f, t, p, m, w. The Linux kernel supports additional characters.

**Impact**: Users cannot use additional sysrq characters in their configuration.

**Recommendation**: WONTFIX - The current set covers the characters needed for fencing. Additional characters provide marginal benefit.

---

## Resolved Issues (for reference)

The following issues from previous reviews have been resolved in commits since 7cf902f45305f96fed3c36c1262cceb41c5c652e:

| Bug # | Description | Resolution |
|-------|-------------|------------|
| 31 | `trigger_fencing` now verifies reboot by waiting after sending 'b' character | Fixed with timeout and fallback in `trigger_fencing` |
| 32 | Race condition between active LUN set read and multipathd query | Fixed with `ActiveLunsWithTimestamp` and staleness detection |
| 33 | Discovery thread has no backoff on repeated failures | Fixed with exponential backoff and panic recovery |
| 35 | `send_multipath_command_to_socket` has no connection timeout | Fixed with `DEFAULT_CONNECT_TIMEOUT_MS` and thread-based timeout |
| 36 | `extract_defaults_block` cannot handle `}` inside quoted strings | Fixed with proper tokenizer that respects quotes |
| 37 | `validate_sysrq` silently skips validation in dry-run mode | Fixed - validation runs regardless of dry-run |
| 38 | `send_command_on_fd` has unclear fd ownership semantics | Fixed with `ManuallyDrop` wrapper |
| 39 | `build_mpath_map` silently stops at depth 32 | Fixed with iterative stack-based traversal |
| 40 | `extract_disks` iterates HashMap in non-deterministic order | Fixed by sorting keys before iteration |
| 45 | `Fencer::update` accepts raw JSON without schema validation | Fixed with version field checks in `MultipathOutput` |
| 46 | `check_multipath_config` comment stripping is naive | Fixed with proper tokenizer |
| 48 | Service file has no `TimeoutStartSec` | Fixed - added `TimeoutStartSec=120` |
| 50 | No `NoNewPrivileges` in systemd service | Fixed - added `NoNewPrivileges=yes` |
| 51 | No `ProtectSystem=strict` in systemd service | Fixed - added `ProtectSystem=strict` and related directives |
| 53 | No rate limiting on sysrq writes | Fixed with `FENCING_IN_PROGRESS` atomic flag |

---

## Summary

**Critical Issues**: 0
**High Severity**: 0 (resolved)
**Medium Severity**: 0 (resolved or wontfix)
**Low Severity**: 0 (resolved or wontfix)

**Total Open Issues**: 0

**Recommendation**: Prioritize fixing the high and medium severity issues:
1. **Config parsing robustness** (bugs 1, 2, 3) - These can cause silent misconfiguration
2. **Status file synchronization** (bugs 5, 6) - Critical for monitoring reliability
3. **Input validation** (bugs 7, 8, 10, 11) - Important for security and reliability
