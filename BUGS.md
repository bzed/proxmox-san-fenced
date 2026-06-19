# Bug Report — pve-san-fenced

All findings below were verified against AGENTS.md, PLAN.md, and general security best practices.
Each entry includes the file, line, description, and severity.

---

## WONTFIX Issues

### 1. `FileCounters` mutex contention in `mpath-mockd` (Correctness / Performance)

The `FileCounters::next_index` method in `tools/mpath-mockd/src/main.rs` locks the mutex,
reads the current index, increments it, and returns. The mutex is held for the entire
duration of the function call including the return, which means the lock is held while
the caller processes the response. This could cause blocking in high-concurrency scenarios.

**Status**: WONTFIX

**Reason**: The current pattern is acceptable for a low-concurrency test daemon. The
FileCounters is used for round-robin cycling through test responses, and the daemon
spawns a new thread per connection. The mutex is held only during the return from
`next_index()`, which is a very short operation. Adding complexity (atomic operations,
different locking strategies) doesn't justify the benefit for this test-only use case.

---

## Previously Fixed Issues

All other issues from the original BUGS.md (29 issues) have been resolved in commit a179acf.
See the commit message for a detailed list of fixes applied.
