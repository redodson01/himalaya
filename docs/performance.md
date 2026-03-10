# Performance Optimizations

Tracking document for performance improvements to Himalaya.

## Status Legend

- **Proposed** — identified but not yet started
- **In Progress** — actively being worked on
- **Done** — implemented and merged

---

## 1. Parallelize multi-account loading

**Status:** Proposed
**Impact:** High | **Effort:** Low | **Complexity:** Low

### Problem

`run_all_accounts()` in `src/tui/mod.rs` loads each account backend
sequentially in a `for` loop. With N IMAP accounts, TUI startup time equals
the sum of all connection times. The same pattern exists in the CLI's
`execute_all()` (`src/cli.rs`) and the default `--all` path in `main.rs`.

### Solution

Use `futures::future::join_all()` or similar to build all backends
concurrently. Startup time becomes `max(times)` instead of `sum(times)`.

### Files

- `src/tui/mod.rs` — `run_all_accounts()`
- `src/cli.rs` — `execute_all()`
- `src/main.rs` — default `--all` envelope list path

---

## 2. Background mark-as-read

**Status:** Proposed
**Impact:** Medium | **Effort:** Low | **Complexity:** Low

### Problem

When opening a message in the TUI (`src/tui/mod.rs`, `ReadMessage` handler),
the `add_flags(Seen)` call runs sequentially after fetching the message body.
The user waits for both operations before seeing the message content. The
result of `add_flags` is already discarded (`let _ = ...`).

### Solution

Display the message immediately after fetch, then fire off the `add_flags`
call in the background via `tokio::spawn`. The flag update happens
asynchronously while the user is already reading.

### Files

- `src/tui/mod.rs` — `ReadMessage` handler

---

## 3. Prefetch adjacent messages in TUI

**Status:** Proposed
**Impact:** Medium | **Effort:** Medium | **Complexity:** Medium

### Problem

Every press of Enter triggers a full network round-trip to fetch the message
body. Users who read messages sequentially experience a loading delay on each
one.

### Solution

After displaying a message, spawn a background task to fetch the next (and
optionally previous) message body. Cache results in a `HashMap<String, String>`
on the App struct. On `ReadMessage`, check the cache before hitting the
backend. Invalidate cache entries when messages are deleted/archived.

### Files

- `src/tui/mod.rs` — `ReadMessage` handler, event loop
- `src/tui/app.rs` — add cache field to `App`

---

## Evaluated and Deferred

These were evaluated but are not worth pursuing at this time.

### Config cloning before `into_account_configs`

Every command calls `config.clone().into_account_configs(...)` because the
method consumes `self`. The clone is typically <1KB and takes microseconds.
Not worth a workaround unless pimalaya-tui exposes a borrow-based API.

### TUI render allocations

`render_envelope_list` rebuilds `Vec<Row>` and mapping vectors every frame
(~10 fps). This is standard immediate-mode rendering and is negligible for
typical envelope counts (<1000). Revisit if envelope counts reach 10,000+.
