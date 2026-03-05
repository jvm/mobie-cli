# mobie-cli

Standalone Rust CLI for querying the reverse-engineered MOBIE API.

## Requirements

- Rust toolchain

## Build

```bash
cargo build
```

## Configure

Required for authenticated commands:

- `MOBIE_EMAIL`
- `MOBIE_PASSWORD`

Optional:

- `MOBIE_BASE_URL` (default: `https://pgm.mobie.pt`)

You can provide credentials in any of these ways:

- CLI flags like `--email` and `--password`
- Exported environment variables
- `dotenvx run -- ...`
- A stored OS keychain session created with `mobie auth login`

Precedence for authenticated commands:

1. `--email` / `--password`
2. exported env vars or `dotenvx`
3. stored keychain session

`mobie auth login` prompts for credentials when they are not already provided and stores the refreshable session in the platform secret backend:

- macOS Keychain
- Linux Secret Service
- Windows Credential Manager

The password is used only for the login request and is not persisted by the CLI.

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
```

Structured output for agents:

```bash
cargo run -p mobie -- --json auth check
cargo run -p mobie -- --json auth status
cargo run -p mobie -- --json locations list
cargo run -p mobie -- --json sessions list --location MOBI-XXX-00000
```

`sessions list --from/--to` maps to the MOBIE API's `dateFrom` / `dateTo` query params. Date-only values include the full day.
