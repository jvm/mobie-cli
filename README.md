# mobie-cli

Standalone Rust CLI for querying the reverse-engineered MOBIE API.

## Requirements

- Rust toolchain

## Build

```bash
cargo build
```

## Install With Homebrew

After the first tagged GitHub release is published, install from the tap:

```bash
brew tap jvm/tap
brew install mobie
```

Upgrade later with:

```bash
brew update
brew upgrade mobie
```

## Configure

Required for authenticated commands:

- `MOBIE_EMAIL`
- `MOBIE_PASSWORD`

Optional:

- `MOBIE_BASE_URL` (default: `https://pgm.mobie.pt`)

You can provide credentials in any of these ways:

- `--email` plus `MOBIE_PASSWORD`
- Exported environment variables
- `dotenvx run -- ...`
- A stored OS keychain session created with `mobie auth login`

Precedence for authenticated commands:

1. explicit `--email` and/or environment-provided credentials
2. exported env vars or `dotenvx`
3. stored keychain session

`mobie auth login` prompts for credentials when they are not already provided and stores the refreshable session in the platform secret backend:

- macOS Keychain
- Linux Secret Service
- Windows Credential Manager

The password is used only for the login request and is not persisted by the CLI. For local safety, `mobie` rejects `--password` on argv; use `MOBIE_PASSWORD`, `dotenvx`, or the interactive prompt instead.

### dotenvx

`mobie` reads standard process environment variables, so it works with `dotenvx` without any extra integration layer.

Example `.env`:

```dotenv
MOBIE_EMAIL=user@example.com
MOBIE_PASSWORD=super-secret
MOBIE_BASE_URL=https://pgm.mobie.pt
```

Run the CLI through `dotenvx`:

```bash
dotenvx run -- cargo run -p mobie -- auth check
dotenvx run -- cargo run -p mobie -- --json sessions list --location MOBI-XXX-00000
```

If you use the compiled binary directly:

```bash
dotenvx run -- ./target/debug/mobie auth check
```

## Usage

Human-readable output:

```bash
cargo run -p mobie -- auth check
cargo run -p mobie -- auth login
cargo run -p mobie -- auth status
cargo run -p mobie -- auth logout
cargo run -p mobie -- locations list
cargo run -p mobie -- locations get --location MOBI-XXX-00000
cargo run -p mobie -- sessions list --location MOBI-XXX-00000 --limit 200
cargo run -p mobie -- sessions list --location MOBI-XXX-00000 --from 2026-03-02 --to 2026-03-08
cargo run -p mobie -- tokens list --limit 200
cargo run -p mobie -- logs list --limit 200 --error-only
cargo run -p mobie -- logs list --location MOBI-LSB-00693 --message-type Heartbeat --from 2026-03-01 --to 2026-03-07
```

Structured output for agents:

```bash
cargo run -p mobie -- --json auth check
cargo run -p mobie -- --json auth status
cargo run -p mobie -- --json locations list
cargo run -p mobie -- --json sessions list --location MOBI-XXX-00000
```

`sessions list --from/--to` maps to the MOBIE API's `dateFrom` / `dateTo` query params. Date-only values include the full day.

`logs list` targets the richer OCPP log surface used by the portal search form. It supports optional `--location`, `--message-type`, `--from`, and `--to`.

Default OCPP log window behavior:

- if `--to` is omitted, `mobie` uses the end of today
- if both `--from` and `--to` are omitted, `mobie` uses the last 7 days
- if both are provided, `mobie` rejects ranges longer than 7 days

Date-only values for `logs list` include the full day.

## Cache Behavior

`mobie` uses a hybrid local SQLite cache.

- `sessions list` and OCPP `logs list` are canonical-record-backed.
- `tokens list`, `logs ocpi`, `locations analytics`, and `locations geojson` remain response-cache-backed.
- `locations list` and `locations get` still use the simpler snapshot cache path.

For canonical resources:

- full raw API documents are stored locally alongside indexed columns for querying
- `sessions list` reads from canonical session rows after refresh
- OCPP `logs list` reads from canonical log rows after refresh
- OCPP log cache scopes are keyed by location, message type, error filter, and requested time window
- `--json` and `--toon` include structured freshness metadata
- terminal and Markdown output show a concise freshness line when available

OCPP logs do not expose a durable per-entry id in the API payload, so the cache uses a synthetic local fingerprint for identity plus a deterministic sort key for ordered reads.

Existing cache databases are migrated in place on open. Legacy `cache_entries` rows are backfilled into canonical tables when possible.

## Release Flow

Homebrew releases are driven by Git tags:

1. Update `apps/mobie/Cargo.toml` to the release version.
2. Commit the change and push it.
3. Create and push a matching tag like `v0.3.1`.

That tag triggers GitHub Actions to:

- build macOS Apple Silicon and Intel release archives
- create a GitHub release in `jvm/mobie-cli`
- dispatch release metadata to `jvm/homebrew-tap`
- update `Formula/mobie.rb` in the tap with the new asset URLs and SHA-256 values

Required repository secrets:

- In `jvm/mobie-cli`: `HOMEBREW_TAP_DISPATCH_TOKEN`

The token only needs permission to dispatch workflows to `jvm/homebrew-tap` and push commits there.
