mod cache;
mod session_store;

use std::io::{self, Write};
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Duration as StdDuration;

use cache::{
    CacheHandle, CacheLookup, CacheSpec, OcppLogQuery as CacheOcppLogQuery,
    SessionQuery as CacheSessionQuery,
};
use chrono::{DateTime, Days, Duration, SecondsFormat, TimeZone, Utc};
use clap::{Parser, Subcommand};
use mobie_api::{AccessContext, MobieApiError, MobieClient, OcppLogFilters, SessionFilters};
use mobie_models::{LocationDetail, LocationSummary, OcppLogEntry, Session, TokenInfo};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use session_store::{KeyringSessionStore, SessionStore, StoredSession};
use thiserror::Error;

const ORDERING_CACHE_VERSION: &str = "oldest-first-v1";
const SESSION_RECENT_LOOKBACK_DAYS: i64 = 3;

fn current_epoch_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheResourceStrategy {
    CanonicalRecords,
    SnapshotEntries,
}

fn cache_resource_strategy(resource: &str) -> CacheResourceStrategy {
    match resource {
        "sessions" | "logs" => CacheResourceStrategy::CanonicalRecords,
        _ => CacheResourceStrategy::SnapshotEntries,
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionQueryOrder {
    OldestFirst,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionLocalQuery {
    location_id: String,
    limit: i64,
    date_from: Option<String>,
    date_to: Option<String>,
    order: SessionQueryOrder,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionRefreshStrategy {
    RollingRecent,
    ExplicitRange,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRefreshWindow {
    window_start: String,
    window_end: String,
    strategy: SessionRefreshStrategy,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionQueryPlan {
    scope: String,
    query: SessionLocalQuery,
    refresh: SessionRefreshWindow,
}

#[derive(Debug, Parser)]
#[command(name = "mobie", version)]
struct Cli {
    /// MOBIE API base URL. Can also be injected with dotenvx via MOBIE_BASE_URL.
    #[arg(long, env = "MOBIE_BASE_URL", default_value = "https://pgm.mobie.pt")]
    base_url: String,

    /// MOBIE account email. Can also be injected with dotenvx via MOBIE_EMAIL.
    #[arg(long, env = "MOBIE_EMAIL", hide_env_values = true)]
    email: Option<String>,

    /// MOBIE account password from MOBIE_PASSWORD or dotenvx. Passing --password is rejected.
    #[arg(long, env = "MOBIE_PASSWORD", hide_env_values = true)]
    password: Option<String>,

    /// Emit JSON for scripts and integrations.
    #[arg(long, global = true, conflicts_with_all = ["markdown", "toon"])]
    json: bool,

    /// Emit raw Markdown for copy/paste or document export.
    #[arg(long, global = true, conflicts_with_all = ["json", "toon"])]
    markdown: bool,

    /// Emit TOON, the preferred structured format for agents.
    #[arg(long, global = true, conflicts_with_all = ["json", "markdown"])]
    toon: bool,

    #[arg(long, global = true)]
    pretty: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Entities {
        #[command(subcommand)]
        command: EntityCommand,
    },
    Roles {
        #[command(subcommand)]
        command: RoleCommand,
    },
    Locations {
        #[command(subcommand)]
        command: LocationCommand,
    },
    Ords {
        #[command(subcommand)]
        command: OrdCommand,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Tokens {
        #[command(subcommand)]
        command: TokenCommand,
    },
    Logs {
        #[command(subcommand)]
        command: LogCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    Check,
    Login,
    Status,
    Logout,
}

#[derive(Debug, Subcommand)]
enum LocationCommand {
    List,
    Get {
        #[arg(long)]
        location: String,
    },
    Analytics,
    Geojson,
}

#[derive(Debug, Subcommand)]
enum EntityCommand {
    Get {
        #[arg(long)]
        code: String,
    },
}

#[derive(Debug, Subcommand)]
enum RoleCommand {
    Get {
        #[arg(long)]
        role: String,
    },
}

#[derive(Debug, Subcommand)]
enum OrdCommand {
    List,
    Statistics,
    CpesIntegrated,
    CpesToIntegrate,
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    List {
        #[arg(long)]
        location: String,

        #[arg(long, default_value_t = 200)]
        limit: i64,

        /// Inclusive range start: year (2026), date (2026-03-02 or 02-03-2026), or RFC3339 timestamp.
        #[arg(long)]
        from: Option<String>,

        /// Exclusive range end for timestamps; date/year values expand to the next day/year.
        #[arg(long)]
        to: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum TokenCommand {
    List {
        #[arg(long, default_value_t = 200)]
        limit: i64,
    },
}

#[derive(Debug, Subcommand)]
enum LogCommand {
    List {
        #[arg(long, default_value_t = 200)]
        limit: i64,

        #[arg(long)]
        location: Option<String>,

        #[arg(long)]
        message_type: Option<String>,

        /// Inclusive range start: year (2026), date (2026-03-02 or 02-03-2026), or RFC3339 timestamp.
        #[arg(long)]
        from: Option<String>,

        /// Inclusive range end for dates; RFC3339 timestamps are used as-is.
        #[arg(long)]
        to: Option<String>,

        #[arg(long, default_value_t = false)]
        error_only: bool,
    },
    Ocpi {
        #[arg(long, default_value_t = 200)]
        limit: i64,
    },
}

#[derive(Debug, Error)]
enum AppError {
    #[error(transparent)]
    Api(#[from] MobieApiError),

    #[error(
        "missing {0} (set --{1}, export {0}, run via dotenvx, or create a stored session with `mobie auth login`)"
    )]
    MissingCredential(&'static str, &'static str),

    #[error("{0}")]
    InvalidInput(String),

    #[error("{0}")]
    SecretStore(String),

    #[error("{0}")]
    InteractiveInput(String),
}

#[derive(Debug, Serialize)]
struct AuthResponse {
    email: String,
    profile: String,
    source: &'static str,
    base_url: String,
    has_refresh_token: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_epoch_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct AuthStatusResponse {
    stored: bool,
    base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    has_refresh_token: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_epoch_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct AuthLogoutResponse {
    removed: bool,
    base_url: String,
}

enum Output {
    Auth(AuthResponse),
    AuthStatus(AuthStatusResponse),
    AuthLogout(AuthLogoutResponse),
    JsonObject(&'static str, Value),
    JsonArray(&'static str, Vec<Value>),
    Locations(Vec<LocationSummary>),
    Location(LocationDetail),
    Sessions(Vec<Session>, Option<FreshnessMeta>),
    Tokens(Vec<TokenInfo>),
    Logs(Vec<OcppLogEntry>, Option<FreshnessMeta>),
}

#[derive(Debug, Serialize)]
struct SuccessEnvelope<'a, T> {
    ok: bool,
    resource: &'a str,
    data: &'a T,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<Meta>,
}

#[derive(Debug, Serialize)]
struct Meta {
    count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    freshness: Option<FreshnessMeta>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FreshnessMeta {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    as_of_epoch_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refreshed_at_epoch_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stale_after_epoch_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope<'a> {
    ok: bool,
    error: ErrorPayload<'a>,
}

#[derive(Debug, Serialize)]
struct ErrorPayload<'a> {
    kind: &'a str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
}

fn success_envelope<'a, T: Serialize>(
    resource: &'a str,
    data: &'a T,
    count: usize,
    freshness: Option<FreshnessMeta>,
) -> SuccessEnvelope<'a, T> {
    SuccessEnvelope {
        ok: true,
        resource,
        data,
        meta: Some(Meta { count, freshness }),
    }
}

fn output_freshness(output: &Output) -> Option<&FreshnessMeta> {
    match output {
        Output::Sessions(_, freshness) | Output::Logs(_, freshness) => freshness.as_ref(),
        _ => None,
    }
}

fn freshness_from_window(
    window: Option<&cache::SyncWindowRecord>,
    ttl: StdDuration,
    scope: String,
    now_epoch_ms: i64,
) -> Option<FreshnessMeta> {
    let window = window?;
    let ttl_ms = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
    let as_of_epoch_ms = window.last_success_epoch_ms;
    let stale_after_epoch_ms = as_of_epoch_ms.map(|ts| ts.saturating_add(ttl_ms));
    let state = if window.is_fresh_at(now_epoch_ms, ttl) {
        "fresh"
    } else {
        "stale"
    };
    let detail = match window.status.as_str() {
        "success" => None,
        other => Some(other.to_string()),
    };

    Some(FreshnessMeta {
        state,
        source: Some("cache"),
        as_of_epoch_ms,
        refreshed_at_epoch_ms: window
            .last_attempt_epoch_ms
            .or(window.last_success_epoch_ms),
        stale_after_epoch_ms,
        scope: Some(scope),
        detail,
    })
}

fn freshness_from_windows(
    windows: &[cache::SyncWindowRecord],
    ttl: StdDuration,
    scope: String,
    now_epoch_ms: i64,
) -> Option<FreshnessMeta> {
    if windows.is_empty() {
        return None;
    }

    let ttl_ms = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
    let as_of_epoch_ms = windows
        .iter()
        .filter_map(|window| window.last_success_epoch_ms)
        .min();
    let refreshed_at_epoch_ms = windows
        .iter()
        .filter_map(|window| {
            window
                .last_attempt_epoch_ms
                .or(window.last_success_epoch_ms)
        })
        .max();
    let stale_after_epoch_ms = windows
        .iter()
        .filter_map(|window| window.last_success_epoch_ms)
        .map(|ts| ts.saturating_add(ttl_ms))
        .min();
    let detail = windows
        .iter()
        .find(|window| window.status != "success")
        .map(|window| window.status.clone());

    Some(FreshnessMeta {
        state: if windows
            .iter()
            .all(|window| window.is_fresh_at(now_epoch_ms, ttl))
        {
            "fresh"
        } else {
            "stale"
        },
        source: Some("cache"),
        as_of_epoch_ms,
        refreshed_at_epoch_ms,
        stale_after_epoch_ms,
        scope: Some(scope),
        detail,
    })
}

#[derive(Debug)]
struct AuthenticatedClient {
    client: MobieClient,
    source: AuthSource,
}

#[derive(Debug, Clone, Copy)]
enum AuthSource {
    Credentials,
    StoredSession,
}

impl AuthResponse {
    fn from_access(base_url: &str, source: &'static str, access: &AccessContext) -> Self {
        Self {
            email: access.user_email.clone(),
            profile: access.profile.clone(),
            source,
            base_url: canonical_base_url(base_url),
            has_refresh_token: access.refresh_token.is_some(),
            expires_at_epoch_ms: access.expires_at_epoch_ms,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let store = KeyringSessionStore;
    let mut cache = CacheHandle::new();

    if password_supplied_via_argv() {
        render_error(
            &cli,
            &ErrorPayload {
                kind: "invalid_input",
                message: "refusing --password on the command line; use MOBIE_PASSWORD, dotenvx, or the interactive prompt".into(),
                status: None,
                url: None,
                body: None,
            },
        );
        return ExitCode::from(1);
    }

    match execute_with_store(&cli, &store, &mut cache).await {
        Ok(output) => match render_output(&cli, &output) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                render_error(
                    &cli,
                    &ErrorPayload {
                        kind: "render_error",
                        message: err.to_string(),
                        status: None,
                        url: None,
                        body: None,
                    },
                );
                ExitCode::from(1)
            }
        },
        Err(err) => {
            render_error(&cli, &error_payload(&err));
            ExitCode::from(1)
        }
    }
}

async fn execute_with_store<S: SessionStore>(
    cli: &Cli,
    store: &S,
    cache: &mut CacheHandle,
) -> Result<Output, AppError> {
    match &cli.command {
        Command::Auth { command } => execute_auth_command(cli, command, store).await,
        Command::Entities { command } => {
            let mut authed = resolve_authenticated_client(cli, store).await?;
            let output = execute_entity_command(&mut authed.client, command).await?;
            persist_session_if_needed(store, cli, &authed)?;
            Ok(output)
        }
        Command::Roles { command } => {
            let mut authed = resolve_authenticated_client(cli, store).await?;
            let output = execute_role_command(&mut authed.client, command).await?;
            persist_session_if_needed(store, cli, &authed)?;
            Ok(output)
        }
        Command::Locations { command } => {
            execute_cached_location_command(cli, store, cache, command).await
        }
        Command::Ords { command } => {
            let mut authed = resolve_authenticated_client(cli, store).await?;
            let output = execute_ord_command(&mut authed.client, command).await?;
            persist_session_if_needed(store, cli, &authed)?;
            Ok(output)
        }
        Command::Sessions { command } => {
            execute_cached_session_command(cli, store, cache, command).await
        }
        Command::Tokens { command } => {
            execute_cached_token_command(cli, store, cache, command).await
        }
        Command::Logs { command } => execute_cached_log_command(cli, store, cache, command).await,
    }
}

async fn execute_auth_command<S: SessionStore>(
    cli: &Cli,
    command: &AuthCommand,
    store: &S,
) -> Result<Output, AppError> {
    match command {
        AuthCommand::Check => {
            let mut authed = resolve_authenticated_client(cli, store).await?;
            if matches!(authed.source, AuthSource::StoredSession) {
                let _ = authed.client.list_tokens_paginated(1).await?;
                persist_session_if_needed(store, cli, &authed)?;
            }

            let access = authed.client.access_context().ok_or_else(|| {
                AppError::InvalidInput("missing access context after auth".into())
            })?;
            let source = match authed.source {
                AuthSource::Credentials => "credentials",
                AuthSource::StoredSession => "stored_session",
            };
            Ok(Output::Auth(AuthResponse::from_access(
                &cli.base_url,
                source,
                access,
            )))
        }
        AuthCommand::Login => {
            let (email, password) = collect_login_credentials(cli)?;
            let mut client = MobieClient::new(&cli.base_url)?;
            let access = client.login(&email, &password).await?;
            save_session(store, &cli.base_url, access.clone())?;
            Ok(Output::Auth(AuthResponse::from_access(
                &cli.base_url,
                "login",
                &access,
            )))
        }
        AuthCommand::Status => match store.load(&cli.base_url).map_err(AppError::SecretStore)? {
            Some(session) => Ok(Output::AuthStatus(AuthStatusResponse {
                stored: true,
                base_url: session.base_url,
                email: Some(session.access.user_email),
                profile: Some(session.access.profile),
                has_refresh_token: Some(session.access.refresh_token.is_some()),
                expires_at_epoch_ms: session.access.expires_at_epoch_ms,
            })),
            None => Ok(Output::AuthStatus(AuthStatusResponse {
                stored: false,
                base_url: canonical_base_url(&cli.base_url),
                email: None,
                profile: None,
                has_refresh_token: None,
                expires_at_epoch_ms: None,
            })),
        },
        AuthCommand::Logout => Ok(Output::AuthLogout(AuthLogoutResponse {
            removed: store.delete(&cli.base_url).map_err(AppError::SecretStore)?,
            base_url: canonical_base_url(&cli.base_url),
        })),
    }
}

async fn execute_cached_location_command<S: SessionStore>(
    cli: &Cli,
    store: &S,
    cache: &mut CacheHandle,
    command: &LocationCommand,
) -> Result<Output, AppError> {
    match command {
        LocationCommand::List => Ok(Output::Locations(
            cached_fetch(
                cli,
                store,
                cache,
                CacheSpec {
                    resource: "locations",
                    ttl: location_list_ttl(),
                    params: vec![("limit", "0".to_string()), ("offset", "0".to_string())],
                },
                |client| {
                    Box::pin(async move { client.list_locations().await.map_err(AppError::from) })
                },
            )
            .await?,
        )),
        LocationCommand::Get { location } => {
            let location = location.clone();
            Ok(Output::Location(
                cached_fetch(
                    cli,
                    store,
                    cache,
                    CacheSpec {
                        resource: "location",
                        ttl: location_detail_ttl(),
                        params: vec![("location", location.clone())],
                    },
                    move |client| {
                        Box::pin(async move {
                            client.get_location(&location).await.map_err(AppError::from)
                        })
                    },
                )
                .await?,
            ))
        }
        LocationCommand::Analytics => Ok(Output::JsonObject(
            "location_analytics",
            cached_fetch(
                cli,
                store,
                cache,
                CacheSpec {
                    resource: "location_analytics",
                    ttl: location_analytics_ttl(),
                    params: Vec::new(),
                },
                |client| {
                    Box::pin(async move {
                        client
                            .get_location_analytics()
                            .await
                            .map_err(AppError::from)
                    })
                },
            )
            .await?,
        )),
        LocationCommand::Geojson => Ok(Output::JsonObject(
            "location_geojson",
            cached_fetch(
                cli,
                store,
                cache,
                CacheSpec {
                    resource: "location_geojson",
                    ttl: location_geojson_ttl(),
                    params: Vec::new(),
                },
                |client| {
                    Box::pin(
                        async move { client.get_location_geojson().await.map_err(AppError::from) },
                    )
                },
            )
            .await?,
        )),
    }
}

async fn execute_cached_session_command<S: SessionStore>(
    cli: &Cli,
    store: &S,
    cache: &mut CacheHandle,
    command: &SessionCommand,
) -> Result<Output, AppError> {
    match command {
        SessionCommand::List {
            location,
            limit,
            from,
            to,
        } => {
            debug_assert!(matches!(
                cache_resource_strategy("sessions"),
                CacheResourceStrategy::CanonicalRecords
            ));
            let location = location.clone();
            let limit = *limit;
            let filters = parse_session_filters(from.as_deref(), to.as_deref())?;
            let plan = session_query_plan(&location, limit, &filters, Utc::now());
            let session_query = CacheSessionQuery {
                location_id: plan.query.location_id.clone(),
                from: plan.query.date_from.clone(),
                to: plan.query.date_to.clone(),
                limit: usize::try_from(plan.query.limit).unwrap_or(usize::MAX),
                oldest_first: matches!(plan.query.order, SessionQueryOrder::OldestFirst),
            };
            let spec = CacheSpec {
                resource: "sessions",
                ttl: sessions_ttl(),
                params: session_cache_params(&location, limit, &filters),
            };
            let mut lookup = cache_lookup(cli, store)?;
            if lookup.profile.is_none() {
                match cache.infer_profile(&lookup) {
                    Ok(Some(profile)) => lookup.profile = Some(profile),
                    Ok(None) => {}
                    Err(err) => cache_warn(cli, cache, &err),
                }
            }
            cache.warn_if_unavailable(!cli.json && !cli.markdown && !cli.toon);

            let needs_refresh = match (
                lookup.user_email.as_deref(),
                lookup.profile.as_deref(),
                cache.get_sync_window(
                    "sessions",
                    &plan.scope,
                    Some(&plan.refresh.window_start),
                    Some(&plan.refresh.window_end),
                ),
            ) {
                (Some(_), Some(_), Ok(Some(window))) => {
                    !window.is_fresh_at(current_epoch_ms(), sessions_ttl())
                }
                _ => true,
            };

            if needs_refresh {
                let mut authed = resolve_authenticated_client(cli, store).await?;
                match authed
                    .client
                    .sync_sessions_window(&location, limit, &filters)
                    .await
                {
                    Ok(data) => {
                        persist_session_if_needed(store, cli, &authed)?;

                        if let Some(access) = authed.client.access_context() {
                            lookup = CacheLookup {
                                base_url: canonical_base_url(&cli.base_url),
                                user_email: Some(access.user_email.clone()),
                                profile: Some(access.profile.clone()),
                            };
                            if let Err(err) = cache.put(&lookup, &spec, &data) {
                                cache_warn(cli, cache, &err);
                            }
                            if let Err(err) = cache.record_sync_success(
                                "sessions",
                                &plan.scope,
                                Some(&plan.refresh.window_start),
                                Some(&plan.refresh.window_end),
                                current_epoch_ms(),
                            ) {
                                cache_warn(cli, cache, &err);
                            }
                        }
                    }
                    Err(err) => {
                        if authed.client.access_context().is_some() {
                            let error_json =
                                serde_json::json!({ "message": err.to_string() }).to_string();
                            if let e @ Err(_) = cache.record_sync_failure(
                                "sessions",
                                &plan.scope,
                                Some(&plan.refresh.window_start),
                                Some(&plan.refresh.window_end),
                                current_epoch_ms(),
                                &error_json,
                            ) {
                                cache_warn(cli, cache, &e.unwrap_err());
                            }
                        }

                        let sessions = cache
                            .read_sessions(&lookup, &session_query)
                            .map_err(AppError::InvalidInput)?;
                        if !sessions.is_empty() {
                            let freshness = freshness_from_window(
                                cache
                                    .get_sync_window(
                                        "sessions",
                                        &plan.scope,
                                        Some(&plan.refresh.window_start),
                                        Some(&plan.refresh.window_end),
                                    )
                                    .ok()
                                    .flatten()
                                    .as_ref(),
                                sessions_ttl(),
                                plan.scope.clone(),
                                current_epoch_ms(),
                            );
                            return Ok(Output::Sessions(sessions, freshness));
                        }
                        return Err(AppError::from(err));
                    }
                }
            }

            let freshness = freshness_from_window(
                cache
                    .get_sync_window(
                        "sessions",
                        &plan.scope,
                        Some(&plan.refresh.window_start),
                        Some(&plan.refresh.window_end),
                    )
                    .ok()
                    .flatten()
                    .as_ref(),
                sessions_ttl(),
                plan.scope.clone(),
                current_epoch_ms(),
            );

            Ok(Output::Sessions(
                cache
                    .read_sessions(&lookup, &session_query)
                    .map_err(AppError::InvalidInput)?,
                freshness,
            ))
        }
    }
}

async fn execute_cached_token_command<S: SessionStore>(
    cli: &Cli,
    store: &S,
    cache: &mut CacheHandle,
    command: &TokenCommand,
) -> Result<Output, AppError> {
    match command {
        TokenCommand::List { limit } => {
            let limit = *limit;
            Ok(Output::Tokens(
                cached_fetch(
                    cli,
                    store,
                    cache,
                    CacheSpec {
                        resource: "tokens",
                        ttl: tokens_ttl(),
                        params: vec![("limit", limit.to_string())],
                    },
                    move |client| {
                        Box::pin(async move {
                            client
                                .list_tokens_paginated(limit)
                                .await
                                .map_err(AppError::from)
                        })
                    },
                )
                .await?,
            ))
        }
    }
}

async fn execute_cached_log_command<S: SessionStore>(
    cli: &Cli,
    store: &S,
    cache: &mut CacheHandle,
    command: &LogCommand,
) -> Result<Output, AppError> {
    match command {
        LogCommand::List {
            limit,
            location,
            message_type,
            from,
            to,
            error_only,
        } => {
            debug_assert!(matches!(
                cache_resource_strategy("logs"),
                CacheResourceStrategy::CanonicalRecords
            ));
            let limit = *limit;
            let error_only = *error_only;
            let plan = ocpp_log_query_plan(
                limit,
                location.as_deref(),
                message_type.as_deref(),
                from.as_deref(),
                to.as_deref(),
                error_only,
                Utc::now(),
            )?;
            let spec = CacheSpec {
                resource: "logs",
                ttl: logs_ttl(),
                params: ocpp_log_cache_params(limit, &plan.api_filters),
            };
            let mut lookup = cache_lookup(cli, store)?;
            if lookup.profile.is_none() {
                match cache.infer_profile(&lookup) {
                    Ok(Some(profile)) => lookup.profile = Some(profile),
                    Ok(None) => {}
                    Err(err) => cache_warn(cli, cache, &err),
                }
            }
            cache.warn_if_unavailable(!cli.json && !cli.markdown && !cli.toon);

            let mut cached_logs = match cache.get(&lookup, &spec) {
                Ok(cached) => cached,
                Err(err) => {
                    cache_warn(cli, cache, &err);
                    None
                }
            };
            let needs_refresh = match (
                lookup.user_email.as_deref(),
                lookup.profile.as_deref(),
                cached_logs.as_ref(),
            ) {
                (Some(_), Some(_), Some(_)) => plan.windows.iter().any(|window| {
                    !matches!(
                        cache.get_sync_window(
                            "logs",
                            &window.scope,
                            Some(&window.day_start.to_rfc3339_opts(SecondsFormat::Secs, true)),
                            Some(&window.day_end.to_rfc3339_opts(SecondsFormat::Secs, true)),
                        ),
                        Ok(Some(record)) if record.is_fresh_at(current_epoch_ms(), logs_ttl())
                    )
                }),
                _ => true,
            };

            if needs_refresh {
                let mut authed = resolve_authenticated_client(cli, store).await?;
                match authed
                    .client
                    .sync_ocpp_logs_window(limit, &plan.api_filters)
                    .await
                {
                    Ok(data) => {
                        persist_session_if_needed(store, cli, &authed)?;

                        if let Some(access) = authed.client.access_context() {
                            lookup = CacheLookup {
                                base_url: canonical_base_url(&cli.base_url),
                                user_email: Some(access.user_email.clone()),
                                profile: Some(access.profile.clone()),
                            };
                            if let Err(err) = cache.put(&lookup, &spec, &data) {
                                cache_warn(cli, cache, &err);
                            }
                            for window in &plan.windows {
                                let window_start =
                                    window.day_start.to_rfc3339_opts(SecondsFormat::Secs, true);
                                let window_end =
                                    window.day_end.to_rfc3339_opts(SecondsFormat::Secs, true);
                                if let Err(err) = cache.record_sync_success(
                                    "logs",
                                    &window.scope,
                                    Some(&window_start),
                                    Some(&window_end),
                                    current_epoch_ms(),
                                ) {
                                    cache_warn(cli, cache, &err);
                                }
                            }
                        }

                        cached_logs = Some(data);
                    }
                    Err(err) => {
                        if authed.client.access_context().is_some() {
                            for window in &plan.windows {
                                let window_start =
                                    window.day_start.to_rfc3339_opts(SecondsFormat::Secs, true);
                                let window_end =
                                    window.day_end.to_rfc3339_opts(SecondsFormat::Secs, true);
                                let error_json =
                                    serde_json::json!({ "message": err.to_string() }).to_string();
                                if let e @ Err(_) = cache.record_sync_failure(
                                    "logs",
                                    &window.scope,
                                    Some(&window_start),
                                    Some(&window_end),
                                    current_epoch_ms(),
                                    &error_json,
                                ) {
                                    cache_warn(cli, cache, &e.unwrap_err());
                                }
                            }
                        }

                        if let Some(cached) = cached_logs {
                            let logs = cache
                                .read_ocpp_logs(
                                    &lookup,
                                    &CacheOcppLogQuery {
                                        limit: usize::try_from(plan.query.limit)
                                            .unwrap_or(usize::MAX),
                                        error_only: plan.query.error_only,
                                        location_id: plan.query.location_id.clone(),
                                        message_type: plan.query.message_type.clone(),
                                        from: plan.query.from.clone(),
                                        to: plan.query.to.clone(),
                                        oldest_first: matches!(
                                            plan.query.order,
                                            LogOrder::OldestFirst
                                        ),
                                    },
                                )
                                .map_err(AppError::InvalidInput)?;
                            let freshness =
                                freshness_from_windows(
                                    &plan
                                        .windows
                                        .iter()
                                        .filter_map(|window| {
                                            cache
                                                .get_sync_window(
                                                    "logs",
                                                    &window.scope,
                                                    Some(&window.day_start.to_rfc3339_opts(
                                                        SecondsFormat::Secs,
                                                        true,
                                                    )),
                                                    Some(&window.day_end.to_rfc3339_opts(
                                                        SecondsFormat::Secs,
                                                        true,
                                                    )),
                                                )
                                                .ok()
                                                .flatten()
                                        })
                                        .collect::<Vec<_>>(),
                                    logs_ttl(),
                                    plan.windows[0].scope.clone(),
                                    current_epoch_ms(),
                                );
                            return Ok(Output::Logs(
                                if logs.is_empty() { cached } else { logs },
                                freshness,
                            ));
                        }
                        return Err(AppError::from(err));
                    }
                }
            }

            let logs = cache
                .read_ocpp_logs(
                    &lookup,
                    &CacheOcppLogQuery {
                        limit: usize::try_from(plan.query.limit).unwrap_or(usize::MAX),
                        error_only: plan.query.error_only,
                        location_id: plan.query.location_id.clone(),
                        message_type: plan.query.message_type.clone(),
                        from: plan.query.from.clone(),
                        to: plan.query.to.clone(),
                        oldest_first: matches!(plan.query.order, LogOrder::OldestFirst),
                    },
                )
                .map_err(AppError::InvalidInput)?;
            let freshness = freshness_from_windows(
                &plan
                    .windows
                    .iter()
                    .filter_map(|window| {
                        cache
                            .get_sync_window(
                                "logs",
                                &window.scope,
                                Some(&window.day_start.to_rfc3339_opts(SecondsFormat::Secs, true)),
                                Some(&window.day_end.to_rfc3339_opts(SecondsFormat::Secs, true)),
                            )
                            .ok()
                            .flatten()
                    })
                    .collect::<Vec<_>>(),
                logs_ttl(),
                plan.windows[0].scope.clone(),
                current_epoch_ms(),
            );
            Ok(Output::Logs(
                if logs.is_empty() {
                    cached_logs.unwrap_or_default()
                } else {
                    logs
                },
                freshness,
            ))
        }
        LogCommand::Ocpi { limit } => {
            let limit = *limit;
            Ok(Output::JsonArray(
                "ocpi_logs",
                cached_fetch(
                    cli,
                    store,
                    cache,
                    CacheSpec {
                        resource: "ocpi_logs",
                        ttl: logs_ttl(),
                        params: ocpi_log_cache_params(limit),
                    },
                    move |client| {
                        Box::pin(async move {
                            client
                                .list_ocpi_logs_paginated(limit)
                                .await
                                .map_err(AppError::from)
                        })
                    },
                )
                .await?,
            ))
        }
    }
}

async fn cached_fetch<S, T, F>(
    cli: &Cli,
    store: &S,
    cache: &mut CacheHandle,
    spec: CacheSpec,
    fetch: F,
) -> Result<T, AppError>
where
    S: SessionStore,
    T: Serialize + DeserializeOwned,
    F: for<'a> FnOnce(
        &'a mut MobieClient,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<T, AppError>> + 'a>>,
{
    debug_assert!(matches!(
        cache_resource_strategy(spec.resource),
        CacheResourceStrategy::SnapshotEntries
    ));
    let lookup = cache_lookup(cli, store)?;
    cache.warn_if_unavailable(!cli.json && !cli.markdown && !cli.toon);
    match cache.get(&lookup, &spec) {
        Ok(Some(cached)) => return Ok(cached),
        Ok(None) => {}
        Err(err) => cache_warn(cli, cache, &err),
    }

    let mut authed = resolve_authenticated_client(cli, store).await?;
    let data = fetch(&mut authed.client).await?;
    persist_session_if_needed(store, cli, &authed)?;

    if let Some(access) = authed.client.access_context() {
        let populated_lookup = CacheLookup {
            base_url: canonical_base_url(&cli.base_url),
            user_email: Some(access.user_email.clone()),
            profile: Some(access.profile.clone()),
        };
        if let Err(err) = cache.put(&populated_lookup, &spec, &data) {
            cache_warn(cli, cache, &err);
        }
    }

    Ok(data)
}

fn cache_lookup<S: SessionStore>(cli: &Cli, store: &S) -> Result<CacheLookup, AppError> {
    let canonical_base_url = canonical_base_url(&cli.base_url);
    let explicit_email = cli
        .email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let stored_session = store.load(&cli.base_url).map_err(AppError::SecretStore)?;
    let matching_stored_session = stored_session.filter(|session| {
        explicit_email
            .as_deref()
            .map(|email| email.eq_ignore_ascii_case(&session.access.user_email))
            .unwrap_or(true)
    });

    Ok(CacheLookup {
        base_url: canonical_base_url,
        user_email: explicit_email.or_else(|| {
            matching_stored_session
                .as_ref()
                .map(|session| session.access.user_email.clone())
        }),
        profile: matching_stored_session.map(|session| session.access.profile),
    })
}

fn cache_warn(cli: &Cli, cache: &mut CacheHandle, error: &str) {
    if !cli.json && !cli.markdown && !cli.toon {
        eprintln!("warning: cache unavailable: {error}");
    }
    cache.warn_if_unavailable(!cli.json && !cli.markdown && !cli.toon);
}

async fn execute_entity_command(
    client: &mut MobieClient,
    command: &EntityCommand,
) -> Result<Output, AppError> {
    match command {
        EntityCommand::Get { code } => {
            Ok(Output::JsonObject("entity", client.get_entity(code).await?))
        }
    }
}

async fn execute_role_command(
    client: &mut MobieClient,
    command: &RoleCommand,
) -> Result<Output, AppError> {
    match command {
        RoleCommand::Get { role } => Ok(Output::JsonObject("role", client.get_role(role).await?)),
    }
}

async fn execute_ord_command(
    client: &mut MobieClient,
    command: &OrdCommand,
) -> Result<Output, AppError> {
    match command {
        OrdCommand::List => Ok(Output::JsonArray("ords", client.list_ords().await?)),
        OrdCommand::Statistics => Ok(Output::JsonObject(
            "ord_statistics",
            client.get_ord_statistics().await?,
        )),
        OrdCommand::CpesIntegrated => Ok(Output::JsonArray(
            "ords_cpes_integrated",
            client.list_ords_cpes_integrated().await?,
        )),
        OrdCommand::CpesToIntegrate => Ok(Output::JsonArray(
            "ords_cpes_to_integrate",
            client.list_ords_cpes_to_integrate().await?,
        )),
    }
}

async fn resolve_authenticated_client<S: SessionStore>(
    cli: &Cli,
    store: &S,
) -> Result<AuthenticatedClient, AppError> {
    if has_explicit_credentials(cli) {
        let mut client = MobieClient::new(&cli.base_url)?;
        let _ = login_with_cli_credentials(&mut client, cli).await?;
        return Ok(AuthenticatedClient {
            client,
            source: AuthSource::Credentials,
        });
    }

    if let Some(session) = store.load(&cli.base_url).map_err(AppError::SecretStore)? {
        return Ok(AuthenticatedClient {
            client: MobieClient::new(&cli.base_url)?.with_access(session.access),
            source: AuthSource::StoredSession,
        });
    }

    Err(AppError::MissingCredential("MOBIE_EMAIL", "email"))
}

fn persist_session_if_needed<S: SessionStore>(
    store: &S,
    cli: &Cli,
    authed: &AuthenticatedClient,
) -> Result<(), AppError> {
    if matches!(authed.source, AuthSource::StoredSession)
        && let Some(access) = authed.client.access_context().cloned()
    {
        save_session(store, &cli.base_url, access)?;
    }

    Ok(())
}

fn save_session<S: SessionStore>(
    store: &S,
    base_url: &str,
    access: AccessContext,
) -> Result<(), AppError> {
    store
        .save(&StoredSession {
            base_url: canonical_base_url(base_url),
            access,
        })
        .map_err(AppError::SecretStore)
}

fn canonical_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let canonical = trimmed.trim_end_matches('/');
    if canonical.is_empty() {
        trimmed.to_string()
    } else {
        canonical.to_string()
    }
}

fn location_list_ttl() -> StdDuration {
    StdDuration::from_secs(60 * 60 * 24)
}

fn location_detail_ttl() -> StdDuration {
    StdDuration::from_secs(60 * 60 * 24)
}

fn location_analytics_ttl() -> StdDuration {
    StdDuration::from_secs(60 * 60 * 6)
}

fn location_geojson_ttl() -> StdDuration {
    StdDuration::from_secs(60 * 60 * 6)
}

fn sessions_ttl() -> StdDuration {
    StdDuration::from_secs(60 * 15)
}

fn tokens_ttl() -> StdDuration {
    StdDuration::from_secs(60 * 15)
}

fn logs_ttl() -> StdDuration {
    StdDuration::from_secs(60 * 15)
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct OcppLogSyncWindow {
    scope: String,
    day_start: DateTime<Utc>,
    day_end: DateTime<Utc>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogOrder {
    OldestFirst,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct OcppLogLocalQuery {
    limit: i64,
    error_only: bool,
    location_id: Option<String>,
    message_type: Option<String>,
    from: Option<String>,
    to: Option<String>,
    order: LogOrder,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct OcppLogQueryPlan {
    windows: Vec<OcppLogSyncWindow>,
    query: OcppLogLocalQuery,
    api_filters: OcppLogFilters,
}

#[allow(dead_code)]
fn ocpp_log_query_plan(
    limit: i64,
    location: Option<&str>,
    message_type: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    error_only: bool,
    now: DateTime<Utc>,
) -> Result<OcppLogQueryPlan, AppError> {
    let filters = parse_ocpp_log_filters(location, message_type, from, to, error_only, now)?;
    Ok(OcppLogQueryPlan {
        windows: ocpp_log_sync_windows(&filters),
        query: OcppLogLocalQuery {
            limit,
            error_only: filters.error_only,
            location_id: filters.location_id.clone(),
            message_type: filters.message_type.clone(),
            from: filters.start_date.clone(),
            to: filters.end_date.clone(),
            order: LogOrder::OldestFirst,
        },
        api_filters: filters,
    })
}

#[allow(dead_code)]
fn ocpp_log_sync_windows(filters: &OcppLogFilters) -> Vec<OcppLogSyncWindow> {
    let day_start = parse_rfc3339_utc(
        filters
            .start_date
            .as_deref()
            .expect("planned OCPP query start date"),
    )
    .expect("valid planned OCPP query start date");
    let day_end = parse_rfc3339_utc(
        filters
            .end_date
            .as_deref()
            .expect("planned OCPP query end date"),
    )
    .expect("valid planned OCPP query end date");
    vec![OcppLogSyncWindow {
        scope: ocpp_log_sync_scope(filters),
        day_start,
        day_end,
    }]
}

fn ocpp_log_sync_scope(filters: &OcppLogFilters) -> String {
    format!(
        "location:{}:message_type:{}:error_only:{}",
        filters.location_id.as_deref().unwrap_or("-"),
        filters.message_type.as_deref().unwrap_or("-"),
        filters.error_only
    )
}

fn session_cache_params(
    location: &str,
    limit: i64,
    filters: &SessionFilters,
) -> Vec<(&'static str, String)> {
    vec![
        ("location", location.to_string()),
        ("limit", limit.to_string()),
        ("order", ORDERING_CACHE_VERSION.to_string()),
        (
            "from",
            filters.date_from.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "to",
            filters.date_to.clone().unwrap_or_else(|| "-".to_string()),
        ),
    ]
}

#[allow(dead_code)]
fn session_sync_scope(location: &str) -> String {
    format!("location:{location}")
}

#[allow(dead_code)]
fn session_query_plan(
    location: &str,
    limit: i64,
    filters: &SessionFilters,
    now: DateTime<Utc>,
) -> SessionQueryPlan {
    let window_end = filters
        .date_to
        .clone()
        .unwrap_or_else(|| format_api_timestamp(now));
    let (window_start, strategy) = match filters.date_from.clone() {
        Some(from) => (from, SessionRefreshStrategy::ExplicitRange),
        None => (
            format_api_timestamp(now - Duration::days(SESSION_RECENT_LOOKBACK_DAYS)),
            SessionRefreshStrategy::RollingRecent,
        ),
    };

    SessionQueryPlan {
        scope: session_sync_scope(location),
        query: SessionLocalQuery {
            location_id: location.to_string(),
            limit,
            date_from: filters.date_from.clone(),
            date_to: filters.date_to.clone(),
            order: SessionQueryOrder::OldestFirst,
        },
        refresh: SessionRefreshWindow {
            window_start,
            window_end,
            strategy,
        },
    }
}

fn ocpp_log_cache_params(limit: i64, filters: &OcppLogFilters) -> Vec<(&'static str, String)> {
    vec![
        ("limit", limit.to_string()),
        ("order", ORDERING_CACHE_VERSION.to_string()),
        ("error_only", filters.error_only.to_string()),
        (
            "location",
            filters
                .location_id
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "message_type",
            filters
                .message_type
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "from",
            filters
                .start_date
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "to",
            filters.end_date.clone().unwrap_or_else(|| "-".to_string()),
        ),
    ]
}

fn ocpi_log_cache_params(limit: i64) -> Vec<(&'static str, String)> {
    vec![
        ("limit", limit.to_string()),
        ("order", ORDERING_CACHE_VERSION.to_string()),
    ]
}

async fn login_with_cli_credentials(
    client: &mut MobieClient,
    cli: &Cli,
) -> Result<AccessContext, AppError> {
    let email = cli
        .email
        .as_deref()
        .ok_or(AppError::MissingCredential("MOBIE_EMAIL", "email"))?;
    let password = cli
        .password
        .as_deref()
        .ok_or(AppError::MissingCredential("MOBIE_PASSWORD", "password"))?;
    Ok(client.login(email, password).await?)
}

fn has_explicit_credentials(cli: &Cli) -> bool {
    cli.email.is_some() || cli.password.is_some()
}

fn password_supplied_via_argv() -> bool {
    std::env::args_os().any(|arg| {
        let arg = arg.to_string_lossy();
        arg == "--password" || arg.starts_with("--password=")
    })
}

fn collect_login_credentials(cli: &Cli) -> Result<(String, String), AppError> {
    if has_explicit_credentials(cli) {
        let email = cli
            .email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(AppError::MissingCredential("MOBIE_EMAIL", "email"))?
            .to_string();
        let password = cli
            .password
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(AppError::MissingCredential("MOBIE_PASSWORD", "password"))?
            .to_string();
        return Ok((email, password));
    }

    let email = match cli
        .email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(email) => email.to_string(),
        None => prompt("Email: ")?,
    };

    let password = match cli
        .password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(password) => password.to_string(),
        None => rpassword::prompt_password("Password: ")
            .map_err(|err| AppError::InteractiveInput(format!("failed to read password: {err}")))?,
    };

    if password.is_empty() {
        return Err(AppError::InteractiveInput(
            "password cannot be empty".to_string(),
        ));
    }

    Ok((email, password))
}

fn prompt(label: &str) -> Result<String, AppError> {
    let stderr = io::stderr();
    let mut handle = stderr.lock();
    write!(handle, "{label}")
        .map_err(|err| AppError::InteractiveInput(format!("failed to write prompt: {err}")))?;
    handle
        .flush()
        .map_err(|err| AppError::InteractiveInput(format!("failed to flush prompt: {err}")))?;

    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|err| AppError::InteractiveInput(format!("failed to read input: {err}")))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(AppError::InteractiveInput(format!(
            "{}cannot be empty",
            label.to_ascii_lowercase()
        )));
    }
    Ok(value)
}

fn parse_session_filters(from: Option<&str>, to: Option<&str>) -> Result<SessionFilters, AppError> {
    let start = from.map(parse_date_start).transpose()?;
    let end = to.map(parse_date_end).transpose()?;

    if let (Some(start), Some(end)) = (start, end)
        && start >= end
    {
        return Err(AppError::InvalidInput(format!(
            "invalid range: --from {start} must be earlier than --to {end}"
        )));
    }

    Ok(SessionFilters {
        date_from: start.map(format_api_timestamp),
        date_to: end.map(format_api_timestamp),
    })
}

fn parse_ocpp_log_filters(
    location: Option<&str>,
    message_type: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    error_only: bool,
    now: DateTime<Utc>,
) -> Result<OcppLogFilters, AppError> {
    let end = match to {
        Some(input) => parse_date_end(input)?,
        None => end_of_previous_millisecond(
            now.date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .checked_add_days(Days::new(1))
                .expect("valid next-day boundary"),
            "today",
        )?,
    };

    let start = match from {
        Some(input) => parse_date_start(input)?,
        None => end
            .checked_sub_signed(Duration::days(7))
            .ok_or_else(|| AppError::InvalidInput("invalid derived OCPP log start date".into()))?,
    };

    if start > end {
        return Err(AppError::InvalidInput(format!(
            "invalid range: --from {start} must be earlier than or equal to --to {end}"
        )));
    }

    if end.signed_duration_since(start) > Duration::days(7) {
        return Err(AppError::InvalidInput(
            "invalid OCPP log range: the interval between --from and --to must be 7 days or less"
                .to_string(),
        ));
    }

    Ok(OcppLogFilters {
        start_date: Some(format_api_timestamp(start)),
        end_date: Some(format_api_timestamp(end)),
        location_id: location.map(str::to_string),
        message_type: message_type.map(str::to_string),
        error_only,
    })
}

fn parse_date_start(input: &str) -> Result<DateTime<Utc>, AppError> {
    let s = input.trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    if let Ok(year) = s.parse::<i32>()
        && (1900..=2100).contains(&year)
    {
        return utc_datetime(year, 1, 1).map_err(AppError::InvalidInput);
    }

    let (year, month, day) = parse_date_components(s)?;
    utc_datetime(year, month, day).map_err(AppError::InvalidInput)
}

fn parse_date_end(input: &str) -> Result<DateTime<Utc>, AppError> {
    let s = input.trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    if let Ok(year) = s.parse::<i32>()
        && (1900..=2100).contains(&year)
    {
        return end_of_previous_millisecond(
            utc_datetime(year + 1, 1, 1).map_err(AppError::InvalidInput)?,
            s,
        );
    }

    let (year, month, day) = parse_date_components(s)?;
    let date = utc_datetime(year, month, day).map_err(AppError::InvalidInput)?;
    let next_day = date
        .checked_add_days(Days::new(1))
        .ok_or_else(|| AppError::InvalidInput(format!("invalid date: {s}")))?;
    end_of_previous_millisecond(next_day, s)
}

fn parse_date_components(input: &str) -> Result<(i32, u32, u32), AppError> {
    let parts: Vec<&str> = input.split('-').collect();
    if parts.len() != 3 {
        return Err(AppError::InvalidInput(format!(
            "invalid date/year: {input} (expected YYYY, YYYY-MM-DD, DD-MM-YYYY, or RFC3339)"
        )));
    }

    let (year, month, day) = if parts[0].len() == 4 {
        (
            parts[0].parse::<i32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        )
    } else if parts[2].len() == 4 {
        (
            parts[2].parse::<i32>(),
            parts[1].parse::<u32>(),
            parts[0].parse::<u32>(),
        )
    } else {
        return Err(AppError::InvalidInput(format!(
            "invalid date format: {input} (expected YYYY-MM-DD or DD-MM-YYYY)"
        )));
    };

    let (year, month, day) = match (year, month, day) {
        (Ok(year), Ok(month), Ok(day)) => (year, month, day),
        _ => return Err(AppError::InvalidInput(format!("invalid date: {input}"))),
    };

    Ok((year, month, day))
}

fn utc_datetime(year: i32, month: u32, day: u32) -> Result<DateTime<Utc>, String> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .ok_or_else(|| format!("invalid date: {year:04}-{month:02}-{day:02}"))
}

fn end_of_previous_millisecond(
    dt: DateTime<Utc>,
    original: &str,
) -> Result<DateTime<Utc>, AppError> {
    dt.checked_sub_signed(Duration::milliseconds(1))
        .ok_or_else(|| AppError::InvalidInput(format!("invalid date: {original}")))
}

fn format_api_timestamp(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_rfc3339_utc(input: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(input)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| AppError::InvalidInput(format!("invalid RFC3339 timestamp {input}: {err}")))
}

fn render_output(cli: &Cli, output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    if cli.json {
        let freshness = output_freshness(output).cloned();
        match output {
            Output::Auth(data) => write_json("auth", data, 1, cli.pretty)?,
            Output::AuthStatus(data) => write_json("auth_status", data, 1, cli.pretty)?,
            Output::AuthLogout(data) => write_json("auth_logout", data, 1, cli.pretty)?,
            Output::JsonObject(resource, data) => write_json(resource, data, 1, cli.pretty)?,
            Output::JsonArray(resource, data) => {
                write_json(resource, data, data.len(), cli.pretty)?
            }
            Output::Locations(data) => write_json("locations", data, data.len(), cli.pretty)?,
            Output::Location(data) => write_json("location", data, 1, cli.pretty)?,
            Output::Sessions(data, _) => {
                write_json_with_freshness("sessions", data, data.len(), freshness, cli.pretty)?
            }
            Output::Tokens(data) => write_json("tokens", data, data.len(), cli.pretty)?,
            Output::Logs(data, _) => {
                write_json_with_freshness("logs", data, data.len(), freshness, cli.pretty)?
            }
        }
        return Ok(());
    }

    if cli.toon {
        return render_toon_output(output, cli.pretty);
    }

    if cli.markdown {
        return render_markdown_output(output);
    }

    render_terminal_output(output)
}

fn render_markdown_output(output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    match output {
        Output::Auth(data) => {
            println!("# Authentication\n");
            render_markdown_key_values(&[
                ("email", data.email.clone()),
                ("profile", data.profile.clone()),
                ("source", data.source.to_string()),
                ("base_url", data.base_url.clone()),
                ("has_refresh_token", data.has_refresh_token.to_string()),
                (
                    "expires_at_epoch_ms",
                    data.expires_at_epoch_ms
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                ),
            ]);
        }
        Output::AuthStatus(data) => {
            println!("# Auth Status\n");
            render_markdown_key_values(&[
                ("stored", data.stored.to_string()),
                ("base_url", data.base_url.clone()),
                ("email", data.email.clone().unwrap_or_else(|| "-".into())),
                (
                    "profile",
                    data.profile.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "has_refresh_token",
                    data.has_refresh_token
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "expires_at_epoch_ms",
                    data.expires_at_epoch_ms
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                ),
            ]);
        }
        Output::AuthLogout(data) => {
            println!("# Auth Logout\n");
            render_markdown_key_values(&[
                ("removed", data.removed.to_string()),
                ("base_url", data.base_url.clone()),
            ]);
        }
        Output::JsonObject(_, data) => {
            render_markdown_json_object(output_resource(output), data);
        }
        Output::JsonArray(resource, data) => {
            render_markdown_json_array(resource, data);
        }
        Output::Locations(data) => {
            println!("# Locations\n");
            println!("Count: {}\n", data.len());
            render_markdown_table(
                &["location_id"],
                data.iter()
                    .map(|location| vec![location.location_id.clone()])
                    .collect(),
            )
        }
        Output::Location(data) => {
            println!("# Location\n");
            render_markdown_json_object(
                "location",
                &serde_json::to_value(data).map_err(Box::<dyn std::error::Error>::from)?,
            );
        }
        Output::Sessions(data, freshness) => {
            println!("# Sessions\n");
            println!("Count: {}\n", data.len());
            if let Some(freshness) = freshness {
                println!("Freshness: {}\n", freshness.state);
            }
            render_markdown_table(
                &[
                    "id",
                    "start_date_time",
                    "end_date_time",
                    "status",
                    "kwh",
                    "location_id",
                    "token_uid",
                ],
                data.iter()
                    .map(|session| {
                        vec![
                            session.id.clone(),
                            session.start_date_time.clone(),
                            session.end_date_time.clone().unwrap_or_else(|| "-".into()),
                            session.status.clone().unwrap_or_else(|| "-".into()),
                            session
                                .kwh
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".into()),
                            session.location_id.clone().unwrap_or_else(|| "-".into()),
                            session
                                .cdr_token
                                .as_ref()
                                .and_then(|token| token.uid.clone())
                                .unwrap_or_else(|| "-".into()),
                        ]
                    })
                    .collect(),
            )
        }
        Output::Tokens(data) => {
            println!("# Tokens\n");
            println!("Count: {}\n", data.len());
            render_markdown_table(
                &["token_uid"],
                data.iter()
                    .map(|token| vec![token.token_uid.clone().unwrap_or_else(|| "-".into())])
                    .collect(),
            )
        }
        Output::Logs(data, freshness) => {
            println!("# OCPP Logs\n");
            println!("Count: {}\n", data.len());
            if let Some(freshness) = freshness {
                println!("Freshness: {}\n", freshness.state);
            }
            render_markdown_table(
                &["timestamp", "message_type", "direction", "location_id"],
                data.iter()
                    .map(|log| {
                        vec![
                            log.timestamp.clone().unwrap_or_else(|| "-".into()),
                            log.message_type.clone().unwrap_or_else(|| "-".into()),
                            log.direction.clone().unwrap_or_else(|| "-".into()),
                            log.id.clone().unwrap_or_else(|| "-".into()),
                        ]
                    })
                    .collect(),
            )
        }
    }

    Ok(())
}

fn render_terminal_output(output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    match output {
        Output::Auth(data) => {
            println!("Authentication");
            render_plain_key_values(&[
                ("email", data.email.clone()),
                ("profile", data.profile.clone()),
                ("source", data.source.to_string()),
                ("base_url", data.base_url.clone()),
                ("has_refresh_token", data.has_refresh_token.to_string()),
            ]);
        }
        Output::AuthStatus(data) => {
            println!("Auth Status");
            render_plain_key_values(&[
                ("stored", data.stored.to_string()),
                ("base_url", data.base_url.clone()),
                ("email", data.email.clone().unwrap_or_else(|| "-".into())),
                (
                    "profile",
                    data.profile.clone().unwrap_or_else(|| "-".into()),
                ),
            ]);
        }
        Output::AuthLogout(data) => {
            println!("Auth Logout");
            render_plain_key_values(&[
                ("removed", data.removed.to_string()),
                ("base_url", data.base_url.clone()),
            ]);
        }
        Output::Locations(data) => {
            println!("Locations ({})", data.len());
            render_plain_table(
                &["location_id"],
                data.iter()
                    .map(|location| vec![location.location_id.clone()])
                    .collect(),
            );
        }
        Output::Location(data) => {
            println!("Location");
            render_terminal_json_object(
                "location",
                &serde_json::to_value(data).map_err(Box::<dyn std::error::Error>::from)?,
            );
        }
        Output::Sessions(data, freshness) => {
            println!("Sessions ({})", data.len());
            if let Some(freshness) = freshness {
                println!("freshness: {}", freshness.state);
            }
            render_plain_table(
                &[
                    "id",
                    "start_date_time",
                    "end_date_time",
                    "status",
                    "kwh",
                    "location_id",
                    "token_uid",
                ],
                data.iter()
                    .map(|session| {
                        vec![
                            session.id.clone(),
                            session.start_date_time.clone(),
                            session.end_date_time.clone().unwrap_or_else(|| "-".into()),
                            session.status.clone().unwrap_or_else(|| "-".into()),
                            session
                                .kwh
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".into()),
                            session.location_id.clone().unwrap_or_else(|| "-".into()),
                            session
                                .cdr_token
                                .as_ref()
                                .and_then(|token| token.uid.clone())
                                .unwrap_or_else(|| "-".into()),
                        ]
                    })
                    .collect(),
            );
        }
        Output::Tokens(data) => {
            println!("Tokens ({})", data.len());
            render_plain_table(
                &["token_uid"],
                data.iter()
                    .map(|token| vec![token.token_uid.clone().unwrap_or_else(|| "-".into())])
                    .collect(),
            );
        }
        Output::Logs(data, freshness) => {
            println!("OCPP Logs ({})", data.len());
            if let Some(freshness) = freshness {
                println!("freshness: {}", freshness.state);
            }
            render_plain_table(
                &["timestamp", "message_type", "direction", "location_id"],
                data.iter()
                    .map(|log| {
                        vec![
                            log.timestamp.clone().unwrap_or_else(|| "-".into()),
                            log.message_type.clone().unwrap_or_else(|| "-".into()),
                            log.direction.clone().unwrap_or_else(|| "-".into()),
                            log.id.clone().unwrap_or_else(|| "-".into()),
                        ]
                    })
                    .collect(),
            );
        }
        Output::JsonObject(resource, data) => render_terminal_json_object(resource, data),
        Output::JsonArray(resource, values) => render_terminal_json_array(resource, values),
    }

    Ok(())
}

fn output_resource(output: &Output) -> &'static str {
    match output {
        Output::Auth(_) => "auth",
        Output::AuthStatus(_) => "auth_status",
        Output::AuthLogout(_) => "auth_logout",
        Output::JsonObject(resource, _) => resource,
        Output::JsonArray(resource, _) => resource,
        Output::Locations(_) => "locations",
        Output::Location(_) => "location",
        Output::Sessions(_, _) => "sessions",
        Output::Tokens(_) => "tokens",
        Output::Logs(_, _) => "logs",
    }
}

fn render_plain_table(headers: &[&str], rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        println!("No results.");
        return;
    }

    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in &rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.len());
        }
    }

    println!("{}", format_plain_row(headers.iter().copied(), &widths));
    println!(
        "{}",
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!(
            "{}",
            format_plain_row(row.iter().map(String::as_str), &widths)
        );
    }
}

fn format_plain_row<'a>(cells: impl Iterator<Item = &'a str>, widths: &[usize]) -> String {
    cells
        .enumerate()
        .map(|(idx, cell)| format!("{cell:<width$}", width = widths[idx]))
        .collect::<Vec<_>>()
        .join("  ")
}

fn render_plain_key_values(rows: &[(&str, String)]) {
    let key_width = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    for (key, value) in rows {
        println!("{key:<width$}  {value}", width = key_width);
    }
}

fn render_markdown_table(headers: &[&str], rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        println!("_No results._");
        return;
    }

    println!(
        "| {} |",
        headers
            .iter()
            .map(|header| escape_markdown_cell(header))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!(
        "| {} |",
        headers
            .iter()
            .map(|_| "---".to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    for row in rows {
        println!(
            "| {} |",
            row.iter()
                .map(|cell| escape_markdown_cell(cell))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
}

fn render_markdown_key_values(rows: &[(&str, String)]) {
    println!("| field | value |");
    println!("| --- | --- |");
    for (field, value) in rows {
        println!(
            "| {} | {} |",
            escape_markdown_cell(field),
            escape_markdown_cell(value)
        );
    }
}

fn render_markdown_json_object(resource: &str, value: &Value) {
    match resource {
        "location_geojson" => render_geojson_summary(value),
        "location_analytics" => render_location_analytics(value),
        _ => {
            println!("# {}\n", humanize_key(resource));
            render_generic_json_object(value);
        }
    }
}

fn render_markdown_json_array(resource: &str, values: &[Value]) {
    let title = match resource {
        "ords" => "ORDs",
        "ords_cpes_integrated" => "Integrated CPEs",
        "ords_cpes_to_integrate" => "CPEs To Integrate",
        "ocpi_logs" => "OCPI Logs",
        _ => resource,
    };
    println!("# {}\n", title);
    println!("Count: {}\n", values.len());

    match resource {
        "ords" | "ords_cpes_integrated" | "ords_cpes_to_integrate" => {
            render_json_object_array_table(
                values,
                &[
                    "cpe",
                    "cpeStatus",
                    "location_id",
                    "integrationDate",
                    "entityCode",
                ],
            );
        }
        "ocpi_logs" => {
            render_json_object_array_table(
                values,
                &["timestamp", "messageType", "direction", "id"],
            );
        }
        _ => render_generic_json_array(values),
    }
}

fn render_terminal_json_object(resource: &str, value: &Value) {
    let title = match resource {
        "entity" => "Entity",
        "role" => "Role",
        "location_analytics" => "Location Analytics",
        "location_geojson" => "Location GeoJSON",
        "ord_statistics" => "ORD Statistics",
        _ => resource,
    };
    println!("{title}");

    match resource {
        "location_geojson" => render_terminal_geojson_summary(value),
        "location_analytics" => render_terminal_location_analytics(value),
        _ => render_terminal_generic_json_object(value),
    }
}

fn render_terminal_json_array(resource: &str, values: &[Value]) {
    let title = match resource {
        "ords" => "ORDs",
        "ords_cpes_integrated" => "Integrated CPEs",
        "ords_cpes_to_integrate" => "CPEs To Integrate",
        "ocpi_logs" => "OCPI Logs",
        _ => resource,
    };
    println!("{title} ({})", values.len());

    match resource {
        "ords" | "ords_cpes_integrated" | "ords_cpes_to_integrate" => {
            render_json_object_array_plain_table(
                values,
                &[
                    "cpe",
                    "cpeStatus",
                    "location_id",
                    "integrationDate",
                    "entityCode",
                ],
            );
        }
        "ocpi_logs" => {
            render_json_object_array_plain_table(
                values,
                &["timestamp", "messageType", "direction", "id"],
            );
        }
        _ => render_terminal_generic_json_array(values),
    }
}

fn render_json_object_array_table(values: &[Value], columns: &[&str]) {
    let rows = values
        .iter()
        .map(|value| {
            columns
                .iter()
                .map(|column| lookup_json_value(value, column))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    render_markdown_table(columns, rows);
}

fn render_json_object_array_plain_table(values: &[Value], columns: &[&str]) {
    let rows = values
        .iter()
        .map(|value| {
            columns
                .iter()
                .map(|column| lookup_json_value(value, column))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    render_plain_table(columns, rows);
}

fn render_location_analytics(value: &Value) {
    println!("# Location Analytics\n");
    if let Some(obj) = value.as_object() {
        let summary = [
            "locationsTotalCount",
            "evsesTotalCount",
            "connectorsTotalCount",
            "locsInUseTotalCount",
            "evsesInUseTotalCount",
        ]
        .iter()
        .filter_map(|key| obj.get(*key).map(|v| (*key, json_scalar_to_string(v))))
        .collect::<Vec<_>>();
        render_markdown_key_values(&summary);

        if let Some(items) = obj.get("locationsByConnectivity").and_then(Value::as_array) {
            println!("\n## Locations By Connectivity\n");
            render_json_object_array_table(items, &["_id", "count"]);
        }
        if let Some(items) = obj.get("evsesByStatus").and_then(Value::as_array) {
            println!("\n## EVSEs By Status\n");
            render_json_object_array_table(items, &["_id", "count"]);
        }
    } else {
        render_generic_json_object(value);
    }
}

fn render_terminal_location_analytics(value: &Value) {
    if let Some(obj) = value.as_object() {
        let summary = [
            "locationsTotalCount",
            "evsesTotalCount",
            "connectorsTotalCount",
            "locsInUseTotalCount",
            "evsesInUseTotalCount",
        ]
        .iter()
        .filter_map(|key| obj.get(*key).map(|v| (*key, json_scalar_to_string(v))))
        .collect::<Vec<_>>();
        render_plain_key_values(&summary);

        if let Some(items) = obj.get("locationsByConnectivity").and_then(Value::as_array) {
            println!("\nLocations By Connectivity");
            render_json_object_array_plain_table(items, &["_id", "count"]);
        }
        if let Some(items) = obj.get("evsesByStatus").and_then(Value::as_array) {
            println!("\nEVSEs By Status");
            render_json_object_array_plain_table(items, &["_id", "count"]);
        }
    } else {
        render_terminal_generic_json_object(value);
    }
}

fn render_geojson_summary(value: &Value) {
    println!("# Location GeoJSON\n");
    if let Some(obj) = value.as_object() {
        let feature_count = obj
            .get("features")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        let geometry_types = obj
            .get("features")
            .and_then(Value::as_array)
            .map(|items| {
                let mut types = items
                    .iter()
                    .filter_map(|item| item.get("geometry"))
                    .filter_map(|geometry| geometry.get("type"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                types.sort();
                types.dedup();
                if types.is_empty() {
                    "-".to_string()
                } else {
                    types.join(", ")
                }
            })
            .unwrap_or_else(|| "-".into());
        let top_level_keys = obj.keys().cloned().collect::<Vec<_>>().join(", ");

        render_markdown_key_values(&[
            ("type", lookup_json_value(value, "type")),
            ("feature_count", feature_count.to_string()),
            ("geometry_types", geometry_types),
            ("top_level_keys", top_level_keys),
        ]);
    } else {
        render_generic_json_object(value);
    }
}

fn render_terminal_geojson_summary(value: &Value) {
    if let Some(obj) = value.as_object() {
        let feature_count = obj
            .get("features")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        let geometry_types = obj
            .get("features")
            .and_then(Value::as_array)
            .map(|items| {
                let mut types = items
                    .iter()
                    .filter_map(|item| item.get("geometry"))
                    .filter_map(|geometry| geometry.get("type"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                types.sort();
                types.dedup();
                if types.is_empty() {
                    "-".to_string()
                } else {
                    types.join(", ")
                }
            })
            .unwrap_or_else(|| "-".into());
        let top_level_keys = obj.keys().cloned().collect::<Vec<_>>().join(", ");
        render_plain_key_values(&[
            ("type", lookup_json_value(value, "type")),
            ("feature_count", feature_count.to_string()),
            ("geometry_types", geometry_types),
            ("top_level_keys", top_level_keys),
        ]);
    } else {
        render_terminal_generic_json_object(value);
    }
}

fn render_generic_json_array(values: &[Value]) {
    if values.is_empty() {
        println!("_No results._");
        return;
    }

    if let Some(first_obj) = values.first().and_then(Value::as_object) {
        let columns = first_obj
            .iter()
            .filter_map(|(key, val)| is_scalar(val).then_some(key.as_str()))
            .take(6)
            .collect::<Vec<_>>();
        if !columns.is_empty() {
            render_json_object_array_table(values, &columns);
            return;
        }
    }

    println!("```json");
    println!(
        "{}",
        serde_json::to_string_pretty(values).unwrap_or_else(|_| "[]".into())
    );
    println!("```");
}

fn render_terminal_generic_json_array(values: &[Value]) {
    if values.is_empty() {
        println!("No results.");
        return;
    }

    if let Some(first_obj) = values.first().and_then(Value::as_object) {
        let columns = first_obj
            .iter()
            .filter_map(|(key, val)| is_scalar(val).then_some(key.as_str()))
            .take(6)
            .collect::<Vec<_>>();
        if !columns.is_empty() {
            render_json_object_array_plain_table(values, &columns);
            return;
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(values).unwrap_or_else(|_| "[]".into())
    );
}

fn render_generic_json_object(value: &Value) {
    if let Some(obj) = value.as_object() {
        let scalar_rows = obj
            .iter()
            .filter_map(|(key, val)| {
                is_scalar(val).then_some((key.as_str(), json_scalar_to_string(val)))
            })
            .collect::<Vec<_>>();
        if !scalar_rows.is_empty() {
            render_markdown_key_values(&scalar_rows);
        }

        for (key, nested) in obj.iter().filter(|(_, val)| !is_scalar(val)) {
            println!("\n## {}\n", humanize_key(key));
            match nested {
                Value::Array(items) => {
                    if items.is_empty() {
                        println!("_No results._");
                    } else if items.iter().all(Value::is_object) {
                        let columns = collect_scalar_columns(items, 6);
                        if columns.is_empty() {
                            println!("- items: {}", items.len());
                        } else {
                            render_json_object_array_table(items, &columns);
                        }
                    } else {
                        println!("- items: {}", items.len());
                    }
                }
                Value::Object(map) => {
                    let nested_rows = map
                        .iter()
                        .filter_map(|(nested_key, nested_val)| {
                            is_scalar(nested_val)
                                .then_some((nested_key.as_str(), json_scalar_to_string(nested_val)))
                        })
                        .collect::<Vec<_>>();
                    if nested_rows.is_empty() {
                        println!(
                            "- keys: {}",
                            map.keys().cloned().collect::<Vec<_>>().join(", ")
                        );
                    } else {
                        render_markdown_key_values(&nested_rows);
                    }
                }
                _ => {}
            }
        }
    } else {
        println!("```json");
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
        );
        println!("```");
    }
}

fn render_terminal_generic_json_object(value: &Value) {
    if let Some(obj) = value.as_object() {
        let scalar_rows = obj
            .iter()
            .filter_map(|(key, val)| {
                is_scalar(val).then_some((key.as_str(), json_scalar_to_string(val)))
            })
            .collect::<Vec<_>>();
        if !scalar_rows.is_empty() {
            render_plain_key_values(&scalar_rows);
        }

        for (key, nested) in obj.iter().filter(|(_, val)| !is_scalar(val)) {
            println!("\n{}", humanize_key(key));
            match nested {
                Value::Array(items) => {
                    if items.is_empty() {
                        println!("No results.");
                    } else if items.iter().all(Value::is_object) {
                        let columns = collect_scalar_columns(items, 6);
                        if columns.is_empty() {
                            println!("items  {}", items.len());
                        } else {
                            render_json_object_array_plain_table(items, &columns);
                        }
                    } else {
                        println!("items  {}", items.len());
                    }
                }
                Value::Object(map) => {
                    let nested_rows = map
                        .iter()
                        .filter_map(|(nested_key, nested_val)| {
                            is_scalar(nested_val)
                                .then_some((nested_key.as_str(), json_scalar_to_string(nested_val)))
                        })
                        .collect::<Vec<_>>();
                    if nested_rows.is_empty() {
                        println!(
                            "keys   {}",
                            map.keys().cloned().collect::<Vec<_>>().join(", ")
                        );
                    } else {
                        render_plain_key_values(&nested_rows);
                    }
                }
                _ => {}
            }
        }
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
        );
    }
}

fn collect_scalar_columns(items: &[Value], limit: usize) -> Vec<&str> {
    items
        .first()
        .and_then(Value::as_object)
        .map(|first_obj| {
            first_obj
                .iter()
                .filter_map(|(key, val)| is_scalar(val).then_some(key.as_str()))
                .take(limit)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn lookup_json_value(value: &Value, key: &str) -> String {
    value
        .as_object()
        .and_then(|obj| obj.get(key))
        .map(json_scalar_to_string)
        .unwrap_or_else(|| "-".into())
}

fn json_scalar_to_string(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "-".into()),
    }
}

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn humanize_key(key: &str) -> String {
    key.replace('_', " ")
}

fn escape_markdown_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br>")
}

fn write_json<T: Serialize>(
    resource: &'static str,
    data: &T,
    count: usize,
    pretty: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    write_json_with_freshness(resource, data, count, None, pretty)
}

fn write_json_with_freshness<T: Serialize>(
    resource: &'static str,
    data: &T,
    count: usize,
    freshness: Option<FreshnessMeta>,
    pretty: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let envelope = success_envelope(resource, data, count, freshness);
    if pretty {
        serde_json::to_writer_pretty(&mut handle, &envelope)?;
    } else {
        serde_json::to_writer(&mut handle, &envelope)?;
    }
    writeln!(handle)?;
    Ok(())
}

fn render_toon_output(output: &Output, _pretty: bool) -> Result<(), Box<dyn std::error::Error>> {
    let freshness = output_freshness(output).cloned();
    match output {
        Output::Auth(data) => write_toon("auth", data, 1)?,
        Output::AuthStatus(data) => write_toon("auth_status", data, 1)?,
        Output::AuthLogout(data) => write_toon("auth_logout", data, 1)?,
        Output::JsonObject(resource, data) => write_toon(resource, data, 1)?,
        Output::JsonArray(resource, data) => write_toon(resource, data, data.len())?,
        Output::Locations(data) => write_toon("locations", data, data.len())?,
        Output::Location(data) => write_toon("location", data, 1)?,
        Output::Sessions(data, _) => {
            write_toon_with_freshness("sessions", data, data.len(), freshness)?
        }
        Output::Tokens(data) => write_toon("tokens", data, data.len())?,
        Output::Logs(data, _) => write_toon_with_freshness("logs", data, data.len(), freshness)?,
    }
    Ok(())
}

fn write_toon<T: Serialize>(
    resource: &'static str,
    data: &T,
    count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    write_toon_with_freshness(resource, data, count, None)
}

fn write_toon_with_freshness<T: Serialize>(
    resource: &'static str,
    data: &T,
    count: usize,
    freshness: Option<FreshnessMeta>,
) -> Result<(), Box<dyn std::error::Error>> {
    let envelope = success_envelope(resource, data, count, freshness);
    let value = serde_json::to_value(&envelope)?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    write!(handle, "{}", encode_toon_document(&value))?;
    writeln!(handle)?;
    Ok(())
}

fn encode_toon_document(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut lines = Vec::new();
            encode_toon_object_fields(map, 0, &mut lines);
            lines.join("\n")
        }
        Value::Array(items) => {
            let mut lines = Vec::new();
            encode_toon_array_body(items, 0, &mut lines);
            lines.join("\n")
        }
        _ => encode_toon_scalar(value, false),
    }
}

fn encode_toon_object_fields(
    map: &serde_json::Map<String, Value>,
    indent: usize,
    lines: &mut Vec<String>,
) {
    for (key, value) in map {
        encode_toon_field(key, value, indent, lines);
    }
}

fn encode_toon_field(key: &str, value: &Value, indent: usize, lines: &mut Vec<String>) {
    let prefix = "  ".repeat(indent);
    match value {
        Value::Object(map) => {
            lines.push(format!("{prefix}{key}:"));
            encode_toon_object_fields(map, indent + 1, lines);
        }
        Value::Array(items) => {
            if let Some(fields) = tabular_fields(items) {
                lines.push(format!(
                    "{prefix}{key}[{}]{{{}}}:",
                    items.len(),
                    fields.join(",")
                ));
                for item in items {
                    let row = fields
                        .iter()
                        .map(|field| {
                            item.as_object()
                                .and_then(|obj| obj.get(field))
                                .map(|v| encode_toon_scalar(v, true))
                                .unwrap_or_else(|| "null".into())
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    lines.push(format!("{}{}", "  ".repeat(indent + 1), row));
                }
            } else if items.iter().all(is_scalar) {
                let values = items
                    .iter()
                    .map(|item| encode_toon_scalar(item, true))
                    .collect::<Vec<_>>()
                    .join(",");
                lines.push(format!("{prefix}{key}[{}]: {values}", items.len()));
            } else {
                lines.push(format!("{prefix}{key}[{}]:", items.len()));
                encode_toon_array_body(items, indent + 1, lines);
            }
        }
        _ => lines.push(format!(
            "{prefix}{key}: {}",
            encode_toon_scalar(value, false)
        )),
    }
}

fn encode_toon_array_body(items: &[Value], indent: usize, lines: &mut Vec<String>) {
    for item in items {
        encode_toon_array_item(item, indent, lines);
    }
}

fn encode_toon_array_item(item: &Value, indent: usize, lines: &mut Vec<String>) {
    let prefix = "  ".repeat(indent);
    match item {
        Value::Object(map) => {
            if map.is_empty() {
                lines.push(format!("{prefix}-"));
                return;
            }
            let mut iter = map.iter();
            if let Some((first_key, first_value)) = iter.next() {
                match first_value {
                    Value::Object(child) => {
                        lines.push(format!("{prefix}- {first_key}:"));
                        encode_toon_object_fields(child, indent + 2, lines);
                    }
                    Value::Array(items) => {
                        if let Some(fields) = tabular_fields(items) {
                            lines.push(format!(
                                "{prefix}- {first_key}[{}]{{{}}}:",
                                items.len(),
                                fields.join(",")
                            ));
                            for row_item in items {
                                let row = fields
                                    .iter()
                                    .map(|field| {
                                        row_item
                                            .as_object()
                                            .and_then(|obj| obj.get(field))
                                            .map(|v| encode_toon_scalar(v, true))
                                            .unwrap_or_else(|| "null".into())
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",");
                                lines.push(format!("{}{}", "  ".repeat(indent + 2), row));
                            }
                        } else if items.iter().all(is_scalar) {
                            let values = items
                                .iter()
                                .map(|v| encode_toon_scalar(v, true))
                                .collect::<Vec<_>>()
                                .join(",");
                            lines.push(format!("{prefix}- {first_key}[{}]: {values}", items.len()));
                        } else {
                            lines.push(format!("{prefix}- {first_key}[{}]:", items.len()));
                            encode_toon_array_body(items, indent + 2, lines);
                        }
                    }
                    _ => {
                        lines.push(format!(
                            "{prefix}- {first_key}: {}",
                            encode_toon_scalar(first_value, false)
                        ));
                    }
                }
            }
            for (key, value) in iter {
                encode_toon_field(key, value, indent + 1, lines);
            }
        }
        Value::Array(items) => {
            if items.iter().all(is_scalar) {
                let values = items
                    .iter()
                    .map(|v| encode_toon_scalar(v, true))
                    .collect::<Vec<_>>()
                    .join(",");
                lines.push(format!("{prefix}- [{}]: {values}", items.len()));
            } else {
                lines.push(format!("{prefix}- [{}]:", items.len()));
                encode_toon_array_body(items, indent + 1, lines);
            }
        }
        _ => lines.push(format!("{prefix}- {}", encode_toon_scalar(item, false))),
    }
}

fn tabular_fields(items: &[Value]) -> Option<Vec<String>> {
    let first = items.first()?.as_object()?;
    let fields = first.keys().cloned().collect::<Vec<_>>();
    if fields.is_empty() {
        return None;
    }
    let ok = items.iter().all(|item| {
        let obj = match item.as_object() {
            Some(obj) => obj,
            None => return false,
        };
        obj.len() == fields.len()
            && fields.iter().all(|field| obj.contains_key(field))
            && obj.values().all(is_scalar)
    });
    ok.then_some(fields)
}

fn encode_toon_scalar(value: &Value, in_row: bool) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => encode_toon_string(v, in_row),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
    }
}

fn encode_toon_string(value: &str, in_row: bool) -> String {
    if value.is_empty() {
        return "\"\"".into();
    }

    let needs_quotes = value.chars().any(|c| {
        c == '\n' || c == '\r' || c == '"' || (in_row && c == ',') || (!in_row && c == '#')
    }) || value.starts_with(' ')
        || value.ends_with(' ')
        || value == "null"
        || value == "true"
        || value == "false";

    if needs_quotes {
        serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
    } else {
        value.to_string()
    }
}

fn render_error(cli: &Cli, payload: &ErrorPayload<'_>) {
    if cli.json {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let envelope = ErrorEnvelope {
            ok: false,
            error: ErrorPayload {
                kind: payload.kind,
                message: payload.message.clone(),
                status: payload.status,
                url: payload.url,
                body: payload.body,
            },
        };
        let write_result = if cli.pretty {
            serde_json::to_writer_pretty(&mut handle, &envelope)
        } else {
            serde_json::to_writer(&mut handle, &envelope)
        };
        if write_result.is_ok() {
            let _ = writeln!(handle);
        }
        return;
    }

    if cli.toon {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let envelope = ErrorEnvelope {
            ok: false,
            error: ErrorPayload {
                kind: payload.kind,
                message: payload.message.clone(),
                status: payload.status,
                url: payload.url,
                body: payload.body,
            },
        };
        if let Ok(value) = serde_json::to_value(&envelope) {
            let _ = write!(handle, "{}", encode_toon_document(&value));
            let _ = writeln!(handle);
        }
        return;
    }

    eprintln!("error: {}", payload.message);
}

fn error_payload(err: &AppError) -> ErrorPayload<'_> {
    match err {
        AppError::Api(api_err) => match api_err {
            MobieApiError::Http(_) => ErrorPayload {
                kind: "transport_error",
                message: api_err.to_string(),
                status: None,
                url: None,
                body: None,
            },
            MobieApiError::InvalidBaseUrl(_) => ErrorPayload {
                kind: "invalid_input",
                message: api_err.to_string(),
                status: None,
                url: None,
                body: None,
            },
            MobieApiError::Unexpected(_) => ErrorPayload {
                kind: "unexpected_error",
                message: api_err.to_string(),
                status: None,
                url: None,
                body: None,
            },
            MobieApiError::LoginFailed { status, url, .. } => ErrorPayload {
                kind: "login_failed",
                message: format_api_error_message("login failed", *status, url),
                status: Some(*status),
                url: Some(url.as_str()),
                body: None,
            },
            MobieApiError::RequestFailed { status, url, .. } => ErrorPayload {
                kind: "request_failed",
                message: format_api_error_message("request failed", *status, url),
                status: Some(*status),
                url: Some(url.as_str()),
                body: None,
            },
            MobieApiError::Unauthorized { status, url, .. } => ErrorPayload {
                kind: "unauthorized",
                message: format_api_error_message("unauthorized", *status, url),
                status: Some(*status),
                url: Some(url.as_str()),
                body: None,
            },
            MobieApiError::RateLimited { status, url, .. } => ErrorPayload {
                kind: "rate_limited",
                message: format_api_error_message("rate limited", *status, url),
                status: Some(*status),
                url: Some(url.as_str()),
                body: None,
            },
            MobieApiError::ServerError { status, url, .. } => ErrorPayload {
                kind: "server_error",
                message: format_api_error_message("server error", *status, url),
                status: Some(*status),
                url: Some(url.as_str()),
                body: None,
            },
        },
        AppError::MissingCredential(_, _) => ErrorPayload {
            kind: "missing_credentials",
            message: err.to_string(),
            status: None,
            url: None,
            body: None,
        },
        AppError::InvalidInput(_) => ErrorPayload {
            kind: "invalid_input",
            message: err.to_string(),
            status: None,
            url: None,
            body: None,
        },
        AppError::SecretStore(_) => ErrorPayload {
            kind: "secret_store_error",
            message: err.to_string(),
            status: None,
            url: None,
            body: None,
        },
        AppError::InteractiveInput(_) => ErrorPayload {
            kind: "interactive_input_error",
            message: err.to_string(),
            status: None,
            url: None,
            body: None,
        },
    }
}

fn format_api_error_message(kind: &str, status: u16, url: &str) -> String {
    format!("{kind}: {status} {url}")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Default)]
    struct MockStore {
        session: RefCell<Option<StoredSession>>,
    }

    impl MockStore {
        fn with_session(session: StoredSession) -> Self {
            Self {
                session: RefCell::new(Some(session)),
            }
        }
    }

    impl SessionStore for MockStore {
        fn load(&self, _base_url: &str) -> Result<Option<StoredSession>, String> {
            Ok(self.session.borrow().clone())
        }

        fn save(&self, session: &StoredSession) -> Result<(), String> {
            *self.session.borrow_mut() = Some(session.clone());
            Ok(())
        }

        fn delete(&self, _base_url: &str) -> Result<bool, String> {
            Ok(self.session.borrow_mut().take().is_some())
        }
    }

    fn cli(base_url: &str, command: Command) -> Cli {
        Cli {
            base_url: base_url.to_string(),
            email: None,
            password: None,
            json: true,
            markdown: false,
            toon: false,
            pretty: false,
            command,
        }
    }

    fn stored_session(base_url: &str, token: &str, refresh_token: Option<&str>) -> StoredSession {
        StoredSession {
            base_url: base_url.to_string(),
            access: AccessContext {
                user_email: "user@example.com".into(),
                profile: "DPC".into(),
                access_token: token.into(),
                refresh_token: refresh_token.map(str::to_string),
                expires_at_epoch_ms: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_else(|_| Duration::from_secs(0))
                        .as_millis() as u64
                        + 60_000,
                ),
            },
        }
    }

    #[tokio::test]
    async fn auth_status_reports_stored_session_metadata() {
        let store = MockStore::with_session(stored_session(
            "https://pgm.mobie.pt",
            "token-1",
            Some("refresh-1"),
        ));
        let mut cache = CacheHandle::new();

        let output = execute_with_store(
            &cli(
                "https://pgm.mobie.pt",
                Command::Auth {
                    command: AuthCommand::Status,
                },
            ),
            &store,
            &mut cache,
        )
        .await
        .unwrap();

        match output {
            Output::AuthStatus(data) => {
                assert!(data.stored);
                assert_eq!(data.email.as_deref(), Some("user@example.com"));
                assert_eq!(data.profile.as_deref(), Some("DPC"));
                assert_eq!(data.has_refresh_token, Some(true));
            }
            _ => panic!("unexpected output"),
        }
    }

    #[tokio::test]
    async fn auth_logout_removes_stored_session() {
        let store = MockStore::with_session(stored_session(
            "https://pgm.mobie.pt",
            "token-1",
            Some("refresh-1"),
        ));
        let mut cache = CacheHandle::new();

        let output = execute_with_store(
            &cli(
                "https://pgm.mobie.pt",
                Command::Auth {
                    command: AuthCommand::Logout,
                },
            ),
            &store,
            &mut cache,
        )
        .await
        .unwrap();

        match output {
            Output::AuthLogout(data) => assert!(data.removed),
            _ => panic!("unexpected output"),
        }
        assert!(store.load("https://pgm.mobie.pt").unwrap().is_none());
    }

    #[tokio::test]
    async fn auth_check_prefers_explicit_credentials_over_stored_session() {
        let server = MockServer::start().await;
        let store = MockStore::with_session(stored_session(
            &server.uri(),
            "stored-token",
            Some("refresh-1"),
        ));
        let mut cache = CacheHandle::new();

        Mock::given(method("POST"))
            .and(path("/api/login"))
            .and(body_json(serde_json::json!({
                "email": "user@example.com",
                "password": "secret"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "bearer": {"access_token": "token-1", "refresh_token": "refresh-1", "expires_in": 3600},
                    "user": {"email": "user@example.com", "roles": [{"profile": "DPC"}]}
                },
                "status_code": 1000,
                "status_message": "Success",
                "timestamp": "2025-01-01T00:00:00Z"
            })))
            .mount(&server)
            .await;

        let output = execute_with_store(
            &Cli {
                base_url: server.uri(),
                email: Some("user@example.com".into()),
                password: Some("secret".into()),
                json: true,
                markdown: false,
                toon: false,
                pretty: false,
                command: Command::Auth {
                    command: AuthCommand::Check,
                },
            },
            &store,
            &mut cache,
        )
        .await
        .unwrap();

        match output {
            Output::Auth(data) => {
                assert_eq!(data.source, "credentials");
                assert_eq!(data.email, "user@example.com");
            }
            _ => panic!("unexpected output"),
        }
    }

    #[tokio::test]
    async fn auth_check_uses_stored_session_and_persists_refresh() {
        let server = MockServer::start().await;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_millis() as u64;
        let store = MockStore::with_session(StoredSession {
            base_url: server.uri(),
            access: AccessContext {
                user_email: "user@example.com".into(),
                profile: "DPC".into(),
                access_token: "old-token".into(),
                refresh_token: Some("refresh-1".into()),
                expires_at_epoch_ms: Some(now_ms - 1_000),
            },
        });
        let mut cache = CacheHandle::new();

        Mock::given(method("POST"))
            .and(path("/api/refresh"))
            .and(body_json(serde_json::json!({"refresh_token": "refresh-1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "bearer": {"access_token": "new-token", "refresh_token": "refresh-2", "expires_in": 3600},
                    "user": {"email": "user@example.com", "roles": [{"profile": "DPC"}]}
                },
                "status_code": 1000,
                "status_message": "Success",
                "timestamp": "2025-01-01T00:00:00Z"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/tokens"))
            .and(query_param("limit", "1"))
            .and(query_param("offset", "0"))
            .and(header("authorization", "Bearer new-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [],
                "status_code": 1000,
                "status_message": "Success",
                "timestamp": "2025-01-01T00:00:00Z"
            })))
            .mount(&server)
            .await;

        let output = execute_with_store(
            &cli(
                &server.uri(),
                Command::Auth {
                    command: AuthCommand::Check,
                },
            ),
            &store,
            &mut cache,
        )
        .await
        .unwrap();

        match output {
            Output::Auth(data) => {
                assert_eq!(data.source, "stored_session");
                assert_eq!(data.email, "user@example.com");
            }
            _ => panic!("unexpected output"),
        }

        let persisted = store.load(&server.uri()).unwrap().unwrap();
        assert_eq!(persisted.access.access_token, "new-token");
        assert_eq!(persisted.access.refresh_token.as_deref(), Some("refresh-2"));
    }

    #[test]
    fn auth_login_with_partial_explicit_credentials_is_non_interactive_and_fails() {
        let err = collect_login_credentials(&Cli {
            base_url: "https://pgm.mobie.pt".into(),
            email: Some("user@example.com".into()),
            password: None,
            json: true,
            markdown: false,
            toon: false,
            pretty: false,
            command: Command::Auth {
                command: AuthCommand::Login,
            },
        })
        .unwrap_err();

        match err {
            AppError::MissingCredential(name, flag) => {
                assert_eq!(name, "MOBIE_PASSWORD");
                assert_eq!(flag, "password");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn session_cache_params_include_ordering_version() {
        let filters = SessionFilters {
            date_from: Some("2025-01-01T00:00:00.000Z".into()),
            date_to: Some("2025-01-02T00:00:00.000Z".into()),
        };

        let params = session_cache_params("EVSE-1", 50, &filters);

        assert!(
            params
                .iter()
                .any(|(key, value)| { *key == "order" && value == ORDERING_CACHE_VERSION })
        );
    }

    #[test]
    fn session_query_plan_without_date_filters_uses_recent_rolling_window() {
        let filters = SessionFilters::default();
        let now = Utc.with_ymd_and_hms(2026, 3, 6, 12, 0, 0).unwrap();

        let plan = session_query_plan("EVSE-1", 25, &filters, now);

        assert_eq!(plan.scope, "location:EVSE-1");
        assert_eq!(
            plan.query,
            SessionLocalQuery {
                location_id: "EVSE-1".into(),
                limit: 25,
                date_from: None,
                date_to: None,
                order: SessionQueryOrder::OldestFirst,
            }
        );
        assert_eq!(
            plan.refresh,
            SessionRefreshWindow {
                window_start: "2026-03-03T12:00:00.000Z".into(),
                window_end: "2026-03-06T12:00:00.000Z".into(),
                strategy: SessionRefreshStrategy::RollingRecent,
            }
        );
    }

    #[test]
    fn session_query_plan_with_from_only_uses_explicit_range_to_now() {
        let filters = SessionFilters {
            date_from: Some("2026-03-01T00:00:00.000Z".into()),
            date_to: None,
        };
        let now = Utc.with_ymd_and_hms(2026, 3, 6, 12, 0, 0).unwrap();

        let plan = session_query_plan("EVSE-1", 50, &filters, now);

        assert_eq!(plan.scope, "location:EVSE-1");
        assert_eq!(
            plan.query.date_from.as_deref(),
            Some("2026-03-01T00:00:00.000Z")
        );
        assert_eq!(plan.query.date_to, None);
        assert_eq!(
            plan.refresh,
            SessionRefreshWindow {
                window_start: "2026-03-01T00:00:00.000Z".into(),
                window_end: "2026-03-06T12:00:00.000Z".into(),
                strategy: SessionRefreshStrategy::ExplicitRange,
            }
        );
    }

    #[test]
    fn session_query_plan_with_explicit_range_preserves_bounds() {
        let filters = SessionFilters {
            date_from: Some("2026-03-01T00:00:00.000Z".into()),
            date_to: Some("2026-03-04T00:00:00.000Z".into()),
        };
        let now = Utc.with_ymd_and_hms(2026, 3, 6, 12, 0, 0).unwrap();

        let plan = session_query_plan("EVSE-1", 10, &filters, now);

        assert_eq!(plan.scope, "location:EVSE-1");
        assert_eq!(
            plan.query,
            SessionLocalQuery {
                location_id: "EVSE-1".into(),
                limit: 10,
                date_from: Some("2026-03-01T00:00:00.000Z".into()),
                date_to: Some("2026-03-04T00:00:00.000Z".into()),
                order: SessionQueryOrder::OldestFirst,
            }
        );
        assert_eq!(
            plan.refresh,
            SessionRefreshWindow {
                window_start: "2026-03-01T00:00:00.000Z".into(),
                window_end: "2026-03-04T00:00:00.000Z".into(),
                strategy: SessionRefreshStrategy::ExplicitRange,
            }
        );
    }

    #[test]
    fn log_cache_params_include_ordering_version() {
        let ocpp_filters = OcppLogFilters {
            start_date: Some("2026-03-01T00:00:00.000Z".into()),
            end_date: Some("2026-03-07T23:59:59.999Z".into()),
            location_id: Some("EVSE-1".into()),
            message_type: Some("Heartbeat".into()),
            error_only: true,
        };
        let ocpp_params = ocpp_log_cache_params(25, &ocpp_filters);
        let ocpi_params = ocpi_log_cache_params(25);

        assert!(
            ocpp_params
                .iter()
                .any(|(key, value)| { *key == "order" && value == ORDERING_CACHE_VERSION })
        );
        assert!(
            ocpi_params
                .iter()
                .any(|(key, value)| { *key == "order" && value == ORDERING_CACHE_VERSION })
        );
    }

    #[test]
    fn ocpp_log_sync_windows_use_query_range_and_scope() {
        let anchor = Utc.with_ymd_and_hms(2026, 3, 6, 19, 45, 53).unwrap();
        let filters =
            parse_ocpp_log_filters(Some("EVSE-1"), Some("Heartbeat"), None, None, false, anchor)
                .unwrap();
        let windows = ocpp_log_sync_windows(&filters);

        assert_eq!(
            windows,
            vec![OcppLogSyncWindow {
                scope: "location:EVSE-1:message_type:Heartbeat:error_only:false".to_string(),
                day_start: parse_rfc3339_utc("2026-02-27T23:59:59.999Z").unwrap(),
                day_end: parse_rfc3339_utc("2026-03-06T23:59:59.999Z").unwrap(),
            }]
        );
    }

    #[test]
    fn ocpp_log_query_plan_preserves_filters_and_ordering() {
        let anchor = Utc.with_ymd_and_hms(2026, 3, 6, 19, 45, 53).unwrap();

        let plan = ocpp_log_query_plan(
            25,
            Some("EVSE-1"),
            Some("Heartbeat"),
            Some("2026-03-01"),
            Some("2026-03-03"),
            true,
            anchor,
        )
        .unwrap();

        assert_eq!(
            plan.query,
            OcppLogLocalQuery {
                limit: 25,
                error_only: true,
                location_id: Some("EVSE-1".into()),
                message_type: Some("Heartbeat".into()),
                from: Some("2026-03-01T00:00:00.000Z".into()),
                to: Some("2026-03-03T23:59:59.999Z".into()),
                order: LogOrder::OldestFirst,
            }
        );
        assert_eq!(plan.windows.len(), 1);
        assert_eq!(
            plan.windows[0].scope,
            "location:EVSE-1:message_type:Heartbeat:error_only:true"
        );
    }

    #[test]
    fn parse_ocpp_log_filters_defaults_to_last_seven_days_ending_today() {
        let anchor = Utc.with_ymd_and_hms(2026, 3, 6, 19, 45, 53).unwrap();

        let filters = parse_ocpp_log_filters(None, None, None, None, false, anchor).unwrap();

        assert_eq!(
            filters.start_date.as_deref(),
            Some("2026-02-27T23:59:59.999Z")
        );
        assert_eq!(
            filters.end_date.as_deref(),
            Some("2026-03-06T23:59:59.999Z")
        );
    }

    #[test]
    fn parse_ocpp_log_filters_with_from_only_uses_end_of_today() {
        let anchor = Utc.with_ymd_and_hms(2026, 3, 6, 19, 45, 53).unwrap();

        let filters =
            parse_ocpp_log_filters(None, None, Some("2026-03-01"), None, false, anchor).unwrap();

        assert_eq!(
            filters.start_date.as_deref(),
            Some("2026-03-01T00:00:00.000Z")
        );
        assert_eq!(
            filters.end_date.as_deref(),
            Some("2026-03-06T23:59:59.999Z")
        );
    }

    #[test]
    fn parse_ocpp_log_filters_with_date_only_to_uses_inclusive_end_of_day() {
        let anchor = Utc.with_ymd_and_hms(2026, 3, 6, 19, 45, 53).unwrap();

        let filters = parse_ocpp_log_filters(
            None,
            None,
            Some("2026-03-01"),
            Some("2026-03-03"),
            false,
            anchor,
        )
        .unwrap();

        assert_eq!(
            filters.start_date.as_deref(),
            Some("2026-03-01T00:00:00.000Z")
        );
        assert_eq!(
            filters.end_date.as_deref(),
            Some("2026-03-03T23:59:59.999Z")
        );
    }

    #[test]
    fn parse_ocpp_log_filters_with_to_only_uses_previous_seven_days() {
        let anchor = Utc.with_ymd_and_hms(2026, 3, 6, 19, 45, 53).unwrap();

        let filters =
            parse_ocpp_log_filters(None, None, None, Some("2026-03-03"), false, anchor).unwrap();

        assert_eq!(
            filters.start_date.as_deref(),
            Some("2026-02-24T23:59:59.999Z")
        );
        assert_eq!(
            filters.end_date.as_deref(),
            Some("2026-03-03T23:59:59.999Z")
        );
    }

    #[test]
    fn parse_ocpp_log_filters_rejects_ranges_longer_than_seven_days() {
        let anchor = Utc.with_ymd_and_hms(2026, 3, 6, 19, 45, 53).unwrap();

        let err = parse_ocpp_log_filters(
            None,
            None,
            Some("2026-02-01"),
            Some("2026-02-10"),
            false,
            anchor,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("7 days or less"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cache_resource_strategy_splits_canonical_and_snapshot_resources() {
        assert_eq!(
            cache_resource_strategy("sessions"),
            CacheResourceStrategy::CanonicalRecords
        );
        assert_eq!(
            cache_resource_strategy("logs"),
            CacheResourceStrategy::CanonicalRecords
        );
        assert_eq!(
            cache_resource_strategy("tokens"),
            CacheResourceStrategy::SnapshotEntries
        );
        assert_eq!(
            cache_resource_strategy("ocpi_logs"),
            CacheResourceStrategy::SnapshotEntries
        );
        assert_eq!(
            cache_resource_strategy("location_analytics"),
            CacheResourceStrategy::SnapshotEntries
        );
    }

    #[test]
    fn success_envelope_json_omits_freshness_when_absent() {
        let data = vec![serde_json::json!({"id": "S-1"})];
        let envelope = success_envelope("sessions", &data, data.len(), None);

        let value = serde_json::to_value(&envelope).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["resource"], "sessions");
        assert_eq!(value["meta"]["count"], 1);
        assert!(value["meta"].get("freshness").is_none());
    }

    #[test]
    fn success_envelope_json_serializes_freshness_metadata() {
        let data = vec![serde_json::json!({"id": "S-1"})];
        let envelope = success_envelope(
            "sessions",
            &data,
            data.len(),
            Some(FreshnessMeta {
                state: "fresh",
                source: Some("cache"),
                as_of_epoch_ms: Some(1_741_254_400_000),
                refreshed_at_epoch_ms: Some(1_741_254_401_000),
                stale_after_epoch_ms: Some(1_741_257_999_000),
                scope: Some("location:EVSE-1".into()),
                detail: Some("covered by canonical session sync window".into()),
            }),
        );

        let value = serde_json::to_value(&envelope).unwrap();

        assert_eq!(value["meta"]["freshness"]["state"], "fresh");
        assert_eq!(value["meta"]["freshness"]["source"], "cache");
        assert_eq!(value["meta"]["freshness"]["scope"], "location:EVSE-1");
        assert_eq!(
            value["meta"]["freshness"]["detail"],
            "covered by canonical session sync window"
        );
    }

    #[test]
    fn success_envelope_toon_serializes_freshness_metadata() {
        let data = vec![serde_json::json!({"messageType": "BootNotification"})];
        let envelope = success_envelope(
            "logs",
            &data,
            data.len(),
            Some(FreshnessMeta {
                state: "stale",
                source: Some("cache"),
                as_of_epoch_ms: Some(1_741_254_400_000),
                refreshed_at_epoch_ms: None,
                stale_after_epoch_ms: Some(1_741_257_999_000),
                scope: Some("charger:ABC/2026-03-06".into()),
                detail: Some("refresh required before reuse".into()),
            }),
        );

        let value = serde_json::to_value(&envelope).unwrap();
        let toon = encode_toon_document(&value);

        assert!(toon.contains("freshness:"));
        assert!(toon.contains("state: stale"));
        assert!(toon.contains("source: cache"));
        assert!(toon.contains("scope: charger:ABC/2026-03-06"));
        assert!(toon.contains("detail: refresh required before reuse"));
    }
}
