# Cache Design

Date: 2026-03-06

## Goal

Design a state-of-the-art local cache for `mobie` that:

- speeds up interactive CLI usage
- reduces load on the MOBIE API
- provides a durable local data foundation for future features such as browsing sessions, charge graphs, and other local views

This document is intentionally limited to design. It does not define the implementation plan.

## Current State

Today the CLI uses a SQLite-backed cache with two layers:

- a whole-response cache keyed by resource and query params
- side tables that partially index records such as `locations`, `sessions`, `tokens`, and `ocpp_logs`

The active read path is mostly whole-response caching. The side tables are populated during cache writes but are not yet the primary query engine for `sessions list` and `logs list`.

That model is simple, but it has two structural weaknesses:

- semantic changes to query behavior can require cache-version busting even when the underlying records did not change
- overlapping queries cannot reuse cached records efficiently because correctness depends on exact cached response blobs

## Live API Findings

Design decisions in this document are based on the current codebase plus live API inspection on 2026-03-06.

### Sessions

- session records expose a stable natural key via `id`
- session payloads are rich documents, including nested arrays such as `charging_periods`
- sessions appear mutable by phase rather than immutable immediately after first observation
- fields such as `status`, `end_date_time`, `last_updated`, and CDR-related metadata can change or be completed later

Conclusion: sessions should be treated as canonical document records with indexed projections.

### OCPP Logs

- live log payloads do not expose a natural unique record id
- the `id` field is the charger or location identifier, not a log-entry identifier
- multiple log records can share the same `id` and the same timestamp, while differing by direction, message type, or payload
- API responses appear ordered, and logs are naturally modeled as time-window queries

Conclusion: logs require synthetic local identity. The design must not assume an API-provided primary key.

### Tokens And OCPI Logs

- current live account/profile sampling returned `401` for `tokens` and `ocpi logs`
- their record identity and freshness semantics remain unknown

Conclusion: these resources should stay on the simpler response-cache path until live sampling under a valid profile confirms stronger assumptions.

## Recommended Architecture

Use a hybrid cache with three layers.

### 1. Canonical Record Layer

Persist durable per-record documents in SQLite.

Initial canonical resources:

- `sessions`
- `locations`
- `ocpp_logs`

Each canonical record stores:

- full raw JSON document
- extracted indexed columns used for filtering, ordering, and future app views
- freshness metadata at the record level

The full raw document is the source of truth. Indexed columns are projections for efficient querying.

### 2. Materialized Query Layer

Maintain small, derived query snapshots only for hot CLI paths.

Examples:

- `sessions list --location ... --from ... --to ... --limit ...`
- `logs list --limit ... --error-only`

These snapshots are:

- derived from canonical records, not from raw API response blobs
- versioned by query semantics such as ordering
- optional accelerators, not the durable truth

### 3. Refresh Metadata Layer

Track sync state independently from record payloads.

This layer stores:

- last successful sync time
- last attempted sync time
- covered scope or window
- staleness policy inputs
- last error state if refresh failed

This is the basis for synchronous CLI refresh today and stale-while-revalidate in future app surfaces.

## Canonical Data Model

### Sessions

Primary key:

- `session.id`

Storage model:

- full session JSON document
- indexed fields such as `start_date_time`, `end_date_time`, `status`, `location_id`, `evse_uid`, `connector_id`, `token_uid`, `last_updated`

Behavior:

- sessions are mutable until clearly terminal
- even terminal sessions may receive lower-frequency follow-up refreshes because derived billing or CDR fields can appear later

### OCPP Logs

Canonical identity:

- strong synthetic fingerprint derived from:
  - `timestamp`
  - `direction`
  - `messageType`
  - charger or location id
  - payload hash

Sortable local key:

- API-order-derived key within bounded sync windows
- modeled as day-relative ordering with stable intra-timestamp ordinal semantics

Storage model:

- full raw log JSON document
- indexed fields such as `timestamp`, `direction`, `message_type`, charger or location id, and error-related markers if available

Behavior:

- treat logs as append-mostly records
- dedupe by fingerprint
- preserve deterministic local ordering via the sortable key

### Locations

Primary key:

- `location_id`

Storage model:

- full raw location JSON document
- indexed fields such as coordinates, status, speed, state

Behavior:

- refresh less aggressively than logs and recent sessions

## Query Model

The cache must support first-class local queries over canonical data.

### Sessions

Local query predicates should support:

- `location_id`
- time range
- deterministic ordering
- `limit`
- future status filters

The CLI should answer `sessions list` from SQLite after required refresh, not by replaying cached response blobs.

### Logs

Local query predicates should support:

- time window
- deterministic ordering
- `limit`
- optional filters such as `error_only`

The CLI should answer `logs list` from SQLite after required refresh, with ordering derived locally from canonical records.

## Freshness Model

The default product direction is a mixed model:

- synchronous refresh behavior for CLI today
- architecture that supports stale-while-revalidate later

### Sessions

Use an adaptive freshness strategy:

- recent sessions: aggressively refreshed rolling window
- older sessions: explicitly refreshed by requested date range, then treated as mostly cold

This matches how sessions are queried and how they evolve over time.

### OCPP Logs

Use bounded sync windows:

- charger-by-day windows when possible

This keeps refresh scopes deterministic and makes API-ordered synthesis practical.

### Other Resources

- `locations`: medium TTL and periodic refresh
- analytics or geojson-style resources: retain snapshot-oriented caching unless and until record modeling becomes useful
- `tokens` and `ocpi_logs`: keep current simpler response-cache model until live data supports stronger assumptions

## Sync Planning

Introduce resource-specific sync planners.

A sync planner decides:

- which API requests to issue
- which local scope or time window the response covers
- how returned records merge into canonical tables
- how coverage metadata is updated

This design separates two concepts that must not be conflated:

- record freshness: whether an individual record is current enough
- coverage freshness: whether a requested query window is known complete enough

Having some sessions for a month is not the same as knowing the month is completely synced.

## CLI Read Path

For a hot query such as `sessions list`:

1. inspect local coverage metadata for the requested scope or window
2. if the window is fresh enough, answer locally from canonical records or an existing materialized snapshot
3. if the window is stale, synchronously refresh the relevant scope or window
4. upsert canonical records transactionally
5. invalidate or rebuild affected query snapshots
6. answer locally from SQLite

For future web or app surfaces, the same metadata can drive stale-while-revalidate instead of blocking reads.

## Error Handling

- refresh failures must not corrupt canonical records
- writes must remain transactional
- if refresh fails but local data exists, terminal output may return stale local data with explicit staleness warnings
- structured modes such as JSON and TOON should expose freshness metadata rather than relying only on stderr warnings
- query snapshots must be invalidated or versioned on semantic changes

## Why Hybrid Is Preferred

Compared with a pure response cache:

- it reduces API pressure by reusing overlapping record sets
- it avoids correctness drift tied to old response blobs
- it creates a durable local dataset that can power future browsing and graph features

Compared with a pure normalized-record-only system:

- it still allows small query snapshots for hot CLI paths
- it preserves room for stable, deterministic CLI behavior without reconstructing every expensive response from scratch

## Non-Goals For This Design

This design does not yet define:

- the exact migration sequence from the current cache schema
- the exact background refresh mechanism for future app surfaces
- a final model for `tokens` or `ocpi_logs`
- UI or web app features

Those belong in the implementation plan.

## Recommended Next Step

Create an implementation plan that stages the work as follows:

1. establish canonical record tables and refresh metadata as the primary abstraction
2. move `sessions list` to canonical-record reads
3. move `logs list` to canonical-record reads using synthetic fingerprint identity and bounded sync windows
4. keep response-cache fallback only where record semantics remain unknown
5. add structured freshness metadata to outputs
