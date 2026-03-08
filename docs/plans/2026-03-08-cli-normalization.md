# CLI Normalization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the current mixed CLI grammar with a smaller, more deterministic command language that is simpler for humans and easier for agentic loops to synthesize correctly on the first try.

**Architecture:** Flatten commands that only support one meaningful action, move primary resource identifiers to positional arguments, and reserve subcommands for domains that truly have multiple behaviors. Keep output envelopes and rendering semantics unchanged so only command parsing and dispatch change.

**Tech Stack:** Rust, clap, tokio, existing integration tests with assert_cmd and wiremock

---

### Task 1: Normalize the command tree

**Files:**
- Modify: `apps/mobie/src/main.rs`

**Step 1: Replace redundant verb layers**

- Change `entities get --code <CODE>` to `entity <CODE>`
- Change `roles get --role <ROLE>` to `role <ROLE>`
- Change `sessions list --location <LOCATION>` to `sessions [LOCATION]`
- Change `tokens list --limit <LIMIT>` to `tokens [--limit <LIMIT>]`
- Introduce `location [LOCATION]` for single-location lookup
- Remove `locations get`

**Step 2: Keep only real multi-action domains as subcommands**

- Keep `auth check|login|status|logout`
- Keep `locations list|analytics|geojson`
- Keep `ords list|statistics|cpes-integrated|cpes-to-integrate`
- Keep `logs list|ocpi`

**Step 3: Use positional identifiers consistently**

- `entity <CODE>`
- `role <ROLE>`
- `location [LOCATION]` with `default_location` fallback
- `sessions [LOCATION]` with `default_location` fallback

**Step 4: Preserve output behavior**

- Do not change JSON envelope shapes
- Do not change Markdown/default/TOON renderers beyond command name routing

### Task 2: Refactor execution dispatch

**Files:**
- Modify: `apps/mobie/src/main.rs`

**Step 1: Update `Command` enum and helper enums/structs**

- Replace wrapper enums that only exist to hold `get`/`list`
- Add direct command variants for flattened commands

**Step 2: Update `execute_with_store`**

- Route new command variants directly
- Remove no-longer-needed wrapper handlers where appropriate

**Step 3: Reuse existing business logic**

- Keep API/caching/rendering logic intact
- Extract or reuse helper functions instead of rewriting fetch logic

### Task 3: Make help and argument semantics agent-friendly

**Files:**
- Modify: `apps/mobie/src/main.rs`
- Test: `apps/mobie/tests/cli.rs`
- Test: `apps/mobie/tests/cli_json.rs`

**Step 1: Add meaningful clap docs for fallback-driven positionals**

- Document that `location` and `sessions` fall back to `default_location`
- Ensure help output matches actual behavior

**Step 2: Stop exposing dead-end grammar**

- Remove parser forms that are no longer valid
- Update tests to the new canonical syntax only

### Task 4: Rewrite tests to the normalized grammar

**Files:**
- Modify: `apps/mobie/tests/cli.rs`
- Modify: `apps/mobie/tests/cli_json.rs`
- Modify: `apps/mobie/tests/cache.rs`

**Step 1: Update invocation forms**

- Replace old `entities get --code ...`
- Replace old `roles get --role ...`
- Replace old `sessions list ...`
- Replace old `tokens list ...`
- Replace old `locations get ...`

**Step 2: Keep behavioral assertions identical**

- Same payload assertions
- Same cache behavior assertions
- Same structured error assertions

### Task 5: Validate the full binary surface

**Files:**
- Modify if needed: `apps/mobie/src/main.rs`

**Step 1: Format**

Run: `cargo fmt`

**Step 2: Run crate tests**

Run: `cargo test -p mobie`

**Step 3: Spot-check help output**

Run:

```bash
target/debug/mobie --help
target/debug/mobie entity --help
target/debug/mobie role --help
target/debug/mobie location --help
target/debug/mobie sessions --help
target/debug/mobie tokens --help
```

Expected:
- Flattened commands appear
- Removed grammar no longer appears
- Positionals are visible in help output
