# Repository Agent Notes

## CLI Output Rules

When changing or adding `mobie` CLI commands:

- `--json` must be the only mode that emits machine-readable JSON.
- `--markdown` must emit raw Markdown intended for copy/paste or export.
- `--toon` must emit TOON and should be treated as the preferred structured format for agents.
- Without `--json` and without `--markdown`, commands must emit terminal-friendly formatted output.
- Do not print raw JSON in default terminal mode.
- `--json`, `--markdown`, and `--toon` are explicit alternatives and must remain mutually exclusive.

## Markdown Rendering Rules

Prefer these formats in Markdown mode:

- Flat list outputs: Markdown tables.
- Single-object outputs: short Markdown key/value sections.
- Nested but still readable payloads: key/value section first, then additional subsections for nested arrays or nested objects.
- Large or irregular payloads: concise Markdown summary first.
- Extremely irregular payloads: use fenced JSON only as a last resort.

Specific expectations:

- `locations list`, `sessions list`, `tokens list`, `logs list`, `ords list`, and similar list commands should use Markdown tables.
- `auth check`, `auth status`, `auth logout`, `entities get`, `roles get`, `locations get`, and `ords statistics` should use Markdown key/value sections.
- `locations geojson` should render a concise Markdown summary instead of dumping the full payload.

## Consistency Rules

- New commands must match the existing `--json` envelope shape.
- Default terminal mode should optimize for scanability and stable formatting.
- `--markdown` should preserve a document-friendly structure equivalent to the terminal view.
- `--toon` should preserve the same response envelope semantics as `--json`, encoded as TOON.
- Favor deterministic columns and ordering over clever formatting.

## Release Workflow

- Homebrew releases are driven by Git tags in `jvm/mobie-cli`.
- The release version source of truth is `apps/mobie/Cargo.toml`.
- Release tags must match that version exactly, using the `vX.Y.Z` format.
- Pushing a matching tag triggers `.github/workflows/release.yml`.
- That workflow builds macOS `aarch64-apple-darwin` and `x86_64-apple-darwin` archives, creates the GitHub release, and dispatches the asset metadata to `jvm/homebrew-tap`.
- The helper scripts for that flow are `scripts/mobie-version.sh` and `scripts/package-release.sh`.
- `jvm/homebrew-tap` listens for the `mobie_release` repository dispatch event and regenerates `Formula/mobie.rb`.
- `HOMEBREW_TAP_DISPATCH_TOKEN` must exist in the `jvm/mobie-cli` repository secrets for the dispatch step to work.
- When changing release packaging, keep the archive naming stable: `mobie-v<version>-<target>.tar.gz`.
