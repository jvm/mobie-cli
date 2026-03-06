# Cache Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace response-blob-first caching for hot session and OCPP log queries with a hybrid cache built around canonical record documents, query coverage metadata, and derived snapshots.

**Architecture:** Keep SQLite as the local store, but make canonical record tables the durable source of truth for sessions, locations, and OCPP logs. Add explicit sync coverage metadata and resource-specific planners, then move `sessions list` and `logs list` to local SQLite query execution after refresh. Keep snapshot-style caching only where record semantics are still uncertain or where a hot derived result is worth memoizing.

**Tech Stack:** Rust, `rusqlite`, SQLite, `reqwest`, existing `mobie_api` and `mobie` crates, existing CLI and integration test suites

---

### Task 1: Add cache version and metadata schema

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`

**Step 1: Write the failing schema test**

Add a test in `apps/mobie/tests/cache.rs` that opens `cache.db` after any cached command and asserts the existence of:

- `cache_meta`
- `sync_windows`

Also assert that existing tables still exist:

- `cache_entries`
- `locations`
- `sessions`
- `ocpp_logs`

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie --test cache schema_contains_cache_meta_and_sync_windows -v`

Expected: FAIL because the tables do not exist.

**Step 3: Write minimal schema implementation**

Modify `apps/mobie/src/cache.rs` schema setup to create:

- `cache_meta(key TEXT PRIMARY KEY, value_json TEXT NOT NULL)`
- `sync_windows(resource TEXT NOT NULL, scope TEXT NOT NULL, window_start TEXT, window_end TEXT, last_success_epoch_ms INTEGER, last_attempt_epoch_ms INTEGER, status TEXT NOT NULL, error_json TEXT, PRIMARY KEY(resource, scope, window_start, window_end))`

Also insert or upsert a cache schema version row in `cache_meta`.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie --test cache schema_contains_cache_meta_and_sync_windows -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs apps/mobie/tests/cache.rs
git commit -m "feat: add cache metadata tables"
```

### Task 2: Introduce canonical record document columns explicitly

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`

**Step 1: Write the failing schema-column test**

Add a test asserting the `sessions`, `locations`, and `ocpp_logs` tables expose explicit canonical-document columns:

- `payload_json`
- `fetched_at`
- `expires_at`

For `ocpp_logs`, also assert columns for:

- `fingerprint`
- `sort_key`

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie --test cache canonical_tables_include_document_and_identity_columns -v`

Expected: FAIL because `fingerprint` and `sort_key` do not exist.

**Step 3: Write minimal schema implementation**

Update `apps/mobie/src/cache.rs` schema and indexes so `ocpp_logs` stores:

- `fingerprint TEXT NOT NULL`
- `sort_key TEXT NOT NULL`

Add indexes for:

- `(scope, timestamp)`
- `(scope, sort_key)`
- `(base_url, user_email, profile, fingerprint)` uniqueness

Keep `payload_json` as the full source document.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie --test cache canonical_tables_include_document_and_identity_columns -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs apps/mobie/tests/cache.rs
git commit -m "feat: prepare canonical record schemas"
```

### Task 3: Add log fingerprint and sort-key helpers

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/src/cache.rs`

**Step 1: Write the failing unit tests**

Add unit tests in `apps/mobie/src/cache.rs` for helper functions that:

- generate identical fingerprints for identical log records
- generate different fingerprints when payload or direction changes
- generate deterministic sort keys from day, timestamp, and ordinal

Use concrete sample JSON values from the live shape:

```json
{
  "id": "MOBI-LSB-00693",
  "messageType": "Heartbeat",
  "direction": "Response",
  "timestamp": "2026-03-06T19:45:53.176Z",
  "logs": "{\"currentTime\":\"2026-03-06T19:45:53.176Z\"}"
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie log_fingerprint -v`

Expected: FAIL because helpers do not exist.

**Step 3: Write minimal implementation**

In `apps/mobie/src/cache.rs`, add helpers that:

- normalize the date bucket from the timestamp
- hash the tuple `(timestamp, direction, messageType, charger_id, payload_json_or_logs_field)`
- derive a lexical `sort_key` like `2026-03-06T19:45:53.176Z#0001`

Keep the functions private to `cache.rs` for now.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie log_fingerprint -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs
git commit -m "feat: add OCPP log identity helpers"
```

### Task 4: Replace current OCPP log row identity with canonical fingerprint identity

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`

**Step 1: Write the failing dedupe test**

Add an integration test that seeds two overlapping `logs list` cache writes containing repeated log records and asserts the database ends with one row per fingerprint, not one row per fetch-position.

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie --test cache overlapping_ocpp_log_fetches_dedupe_by_fingerprint -v`

Expected: FAIL because the current code uses `log_key` derived from timestamp or index.

**Step 3: Write minimal implementation**

Update OCPP log sync logic in `apps/mobie/src/cache.rs` so:

- rows upsert by fingerprint identity
- `sort_key` is stored separately
- overlapping fetches no longer require preserving page index identity

Do not yet change CLI reads.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie --test cache overlapping_ocpp_log_fetches_dedupe_by_fingerprint -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs apps/mobie/tests/cache.rs
git commit -m "feat: dedupe OCPP logs by fingerprint"
```

### Task 5: Extract sync coverage metadata primitives

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Modify: `apps/mobie/src/main.rs`
- Test: `apps/mobie/src/cache.rs`

**Step 1: Write the failing unit tests**

Add tests for cache coverage helpers that:

- record a successful sync window
- record a failed sync attempt
- report whether a requested window is fresh enough

Use concrete examples such as:

- resource `sessions`
- scope `location:MOBI-LSB-00693`
- window `2026-03-01T00:00:00Z` to `2026-03-07T00:00:00Z`

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie sync_window -v`

Expected: FAIL because helpers do not exist.

**Step 3: Write minimal implementation**

Add `cache.rs` functions or methods to:

- upsert sync-window metadata
- load sync-window metadata
- evaluate freshness from `last_success_epoch_ms` and policy inputs

Keep this logic independent of CLI commands.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie sync_window -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs apps/mobie/src/main.rs
git commit -m "feat: add sync coverage metadata primitives"
```

### Task 6: Extract session cache query-planning helpers

**Files:**
- Modify: `apps/mobie/src/main.rs`
- Test: `apps/mobie/src/main.rs`

**Step 1: Write the failing unit tests**

Add unit tests for helpers that convert `sessions list` arguments into:

- a local query shape
- a sync scope string
- a refresh window policy

Test at least:

- no date filters
- explicit `--from`
- explicit `--from` and `--to`

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie session_query_plan -v`

Expected: FAIL because helpers do not exist.

**Step 3: Write minimal implementation**

In `apps/mobie/src/main.rs`, extract pure helpers that build:

- a scope key like `location:<id>`
- a bounded or rolling time window
- a query descriptor carrying ordering and limit semantics

Do not yet wire them into the live cache path.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie session_query_plan -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/main.rs
git commit -m "refactor: extract session query planning helpers"
```

### Task 7: Add local SQLite session query reader

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`

**Step 1: Write the failing query test**

Add a test that seeds session rows directly or via cache sync and asserts a local reader returns:

- filtered by `location_id`
- ordered oldest first
- limited deterministically

Use at least three sessions with distinct timestamps.

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie --test cache session_query_reader_filters_and_orders_results -v`

Expected: FAIL because there is no local session query reader.

**Step 3: Write minimal implementation**

Add a session read function in `apps/mobie/src/cache.rs` that queries `sessions` by:

- scope or location
- optional time range
- order by `start_date_time ASC`
- `LIMIT`

Deserialize rows from `payload_json` back into session documents.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie --test cache session_query_reader_filters_and_orders_results -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs apps/mobie/tests/cache.rs
git commit -m "feat: add local session query reader"
```

### Task 8: Make `sessions list` read through sync metadata and local session queries

**Files:**
- Modify: `apps/mobie/src/main.rs`
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`
- Test: `apps/mobie/tests/cli.rs`

**Step 1: Write the failing integration tests**

Add tests that prove:

- a fresh local session window is served without an API call
- a stale session window triggers a refresh and then reads locally
- overlapping queries reuse canonical rows instead of storing separate whole-response blobs as the primary truth

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie sessions_list_ -v`

Expected: FAIL because `sessions list` still depends on whole-response caching.

**Step 3: Write minimal implementation**

Wire `execute_cached_session_command` in `apps/mobie/src/main.rs` so it:

- computes the session query plan
- checks sync-window freshness
- refreshes via API if needed
- upserts canonical session rows
- answers from the local session query reader

Keep the current response-cache fallback only if migration is blocked by missing canonical data.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie sessions_list_ -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/main.rs apps/mobie/src/cache.rs apps/mobie/tests/cache.rs apps/mobie/tests/cli.rs
git commit -m "feat: serve session lists from canonical cache"
```

### Task 9: Extract OCPP log query-planning helpers

**Files:**
- Modify: `apps/mobie/src/main.rs`
- Test: `apps/mobie/src/main.rs`

**Step 1: Write the failing unit tests**

Add tests for helpers that convert `logs list` arguments into:

- charger/day sync windows
- local query shape
- deterministic ordering

Cover at least:

- no explicit charger constraint
- day-bucketed windows
- `error_only = true`

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie log_query_plan -v`

Expected: FAIL because helpers do not exist.

**Step 3: Write minimal implementation**

Add pure planning helpers in `apps/mobie/src/main.rs` that:

- derive the relevant day buckets
- define the sync scope
- encode ordering semantics

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie log_query_plan -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/main.rs
git commit -m "refactor: extract OCPP log query planning helpers"
```

### Task 10: Add local SQLite OCPP log query reader

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`

**Step 1: Write the failing log-query test**

Add a test that seeds OCPP log rows and asserts a local reader returns:

- ordered oldest first
- deduped by fingerprint
- filtered by `error_only` when markers are available
- limited deterministically

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie --test cache ocpp_log_query_reader_orders_and_limits_results -v`

Expected: FAIL because there is no local OCPP log query reader.

**Step 3: Write minimal implementation**

Add a log read function in `apps/mobie/src/cache.rs` that queries `ocpp_logs` ordered by:

- `timestamp ASC`
- `sort_key ASC`

Deserialize `payload_json` back into the CLI output shape.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie --test cache ocpp_log_query_reader_orders_and_limits_results -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs apps/mobie/tests/cache.rs
git commit -m "feat: add local OCPP log query reader"
```

### Task 11: Make `logs list` read through sync metadata and local log queries

**Files:**
- Modify: `apps/mobie/src/main.rs`
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`
- Test: `apps/mobie/tests/cli.rs`

**Step 1: Write the failing integration tests**

Add tests that prove:

- a fresh local OCPP log window is served without an API call
- a stale log window refreshes the relevant day bucket and then serves local results
- overlapping fetches do not duplicate log records

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie logs_list_ -v`

Expected: FAIL because `logs list` still depends on whole-response caching.

**Step 3: Write minimal implementation**

Wire `execute_cached_log_command` for OCPP logs so it:

- computes day-bucket sync windows
- refreshes stale buckets
- upserts canonical log rows
- answers from the local log query reader

Keep `ocpi_logs` on the current snapshot-style response cache.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie logs_list_ -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/main.rs apps/mobie/src/cache.rs apps/mobie/tests/cache.rs apps/mobie/tests/cli.rs
git commit -m "feat: serve OCPP logs from canonical cache"
```

### Task 12: Add structured freshness metadata to outputs

**Files:**
- Modify: `apps/mobie/src/main.rs`
- Test: `apps/mobie/tests/cli.rs`
- Test: `apps/mobie/tests/cli_json.rs`

**Step 1: Write the failing output tests**

Add tests asserting:

- JSON includes freshness fields for cache-backed reads
- terminal output warns explicitly when stale local data is served after refresh failure
- TOON preserves the same freshness semantics as JSON

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie freshness -v`

Expected: FAIL because output modes do not expose this metadata yet.

**Step 3: Write minimal implementation**

Add an output metadata structure in `apps/mobie/src/main.rs` carrying fields such as:

- `cache_source`
- `freshness_status`
- `last_success_epoch_ms`
- `stale_reason` when relevant

Ensure:

- `--json` and `--toon` include structured freshness info
- default terminal mode prints concise warnings only when relevant

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie freshness -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/main.rs apps/mobie/tests/cli.rs apps/mobie/tests/cli_json.rs
git commit -m "feat: expose cache freshness metadata"
```

### Task 13: Restrict response-cache-first behavior to uncertain resources

**Files:**
- Modify: `apps/mobie/src/main.rs`
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`

**Step 1: Write the failing regression test**

Add a test that verifies:

- `sessions` and OCPP `logs` are no longer primarily satisfied by whole-response blob reuse
- `tokens`, `ocpi_logs`, and analytics-style resources still use the simpler response-cache path

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie response_cache_policy -v`

Expected: FAIL because the policy split is not explicit yet.

**Step 3: Write minimal implementation**

Refactor cache entry points so the resource strategy is explicit:

- canonical-record-first for `sessions`
- canonical-record-first for OCPP `logs`
- response-cache-first for uncertain resources

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie response_cache_policy -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/main.rs apps/mobie/src/cache.rs apps/mobie/tests/cache.rs
git commit -m "refactor: split canonical and snapshot cache policies"
```

### Task 14: Add migration and compatibility coverage

**Files:**
- Modify: `apps/mobie/src/cache.rs`
- Test: `apps/mobie/tests/cache.rs`

**Step 1: Write the failing migration tests**

Add tests that simulate opening an existing cache DB and assert:

- new tables and columns are created safely
- old response-cache rows remain readable where still needed
- session and log canonical queries can operate after migration without manual cleanup

**Step 2: Run test to verify it fails**

Run: `cargo test -p mobie migration_ -v`

Expected: FAIL because migration behavior is not explicit enough.

**Step 3: Write minimal implementation**

In `apps/mobie/src/cache.rs`, make schema initialization and backfill idempotent for preexisting DBs. Avoid destructive resets.

**Step 4: Run test to verify it passes**

Run: `cargo test -p mobie migration_ -v`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/mobie/src/cache.rs apps/mobie/tests/cache.rs
git commit -m "feat: add cache schema migration coverage"
```

### Task 15: Run full verification and document behavior

**Files:**
- Modify: `README.md`
- Modify: `docs/mobie-api.md`
- Test: `apps/mobie/tests/cache.rs`
- Test: `apps/mobie/tests/cli.rs`
- Test: `apps/mobie/tests/cli_json.rs`
- Test: `crates/mobie_api/tests/*.rs`

**Step 1: Write the failing documentation checklist**

Add a small checklist to the PR or task notes covering:

- sessions canonical cache behavior
- OCPP log synthetic identity
- freshness metadata in outputs
- remaining snapshot-only resources

**Step 2: Run full test suite before docs**

Run: `cargo test`

Expected: PASS for the full workspace.

**Step 3: Update docs minimally**

Document:

- which resources are canonical-record-backed
- how freshness behaves in CLI mode
- why OCPP logs use synthetic local identity
- which resources still remain response-cache-first

**Step 4: Run full verification again**

Run: `cargo test`

Expected: PASS

**Step 5: Commit**

```bash
git add README.md docs/mobie-api.md apps/mobie/tests/cache.rs apps/mobie/tests/cli.rs apps/mobie/tests/cli_json.rs crates/mobie_api/tests
git commit -m "docs: describe hybrid cache behavior"
```
