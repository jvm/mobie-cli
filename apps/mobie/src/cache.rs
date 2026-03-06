use std::collections::BTreeMap;
use std::fs;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const CACHE_ENV_DIR: &str = "MOBIE_CACHE_DIR";
const CACHE_DB_NAME: &str = "cache.db";

#[derive(Debug, Clone)]
pub struct CacheLookup {
    pub base_url: String,
    pub user_email: Option<String>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CacheSpec {
    pub resource: &'static str,
    pub ttl: Duration,
    pub params: Vec<(&'static str, String)>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct CacheEntryMeta {
    pub fetched_at_epoch_ms: i64,
    pub expires_at_epoch_ms: i64,
}

#[cfg(test)]
impl CacheEntryMeta {
    pub fn is_stale_at(&self, now_epoch_ms: i64) -> bool {
        now_epoch_ms >= self.expires_at_epoch_ms
    }
}

#[derive(Debug)]
pub struct CacheHandle {
    store: Option<CacheStore>,
    unavailable_reason: Option<String>,
    warned_unavailable: bool,
}

impl CacheHandle {
    pub fn new() -> Self {
        match CacheStore::open_default() {
            Ok(store) => Self {
                store: Some(store),
                unavailable_reason: None,
                warned_unavailable: false,
            },
            Err(err) => Self {
                store: None,
                unavailable_reason: Some(err),
                warned_unavailable: false,
            },
        }
    }

    pub fn get<T>(&mut self, lookup: &CacheLookup, spec: &CacheSpec) -> Result<Option<T>, String>
    where
        T: DeserializeOwned,
    {
        let Some(store) = self.store.as_mut() else {
            return Ok(None);
        };

        store.get(lookup, spec)
    }

    pub fn put<T>(
        &mut self,
        lookup: &CacheLookup,
        spec: &CacheSpec,
        value: &T,
    ) -> Result<(), String>
    where
        T: Serialize,
    {
        let Some(store) = self.store.as_mut() else {
            return Ok(());
        };

        store.put(lookup, spec, value)
    }

    pub fn warn_if_unavailable(&mut self, emit_warning: bool) {
        if emit_warning && !self.warned_unavailable {
            if let Some(reason) = self.unavailable_reason.as_deref() {
                eprintln!("warning: cache disabled: {reason}");
                self.warned_unavailable = true;
            }
        }
    }
}

#[derive(Debug)]
struct CacheStore {
    conn: Connection,
}

impl CacheStore {
    fn open_default() -> Result<Self, String> {
        let path = default_cache_db_path()?;
        Self::open_at(path)
    }

    fn open_at(path: PathBuf) -> Result<Self, String> {
        let parent = path
            .parent()
            .ok_or_else(|| format!("invalid cache database path: {}", path.display()))?;
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create cache directory {}: {err}",
                parent.display()
            )
        })?;

        let conn = Connection::open(&path)
            .map_err(|err| format!("failed to open cache database {}: {err}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS cache_entries (
                 key TEXT PRIMARY KEY,
                 resource TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 etag_or_version TEXT
             );
             CREATE TABLE IF NOT EXISTS locations (
                 base_url TEXT NOT NULL,
                 user_email TEXT NOT NULL,
                 profile TEXT NOT NULL,
                 location_id TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 latitude TEXT,
                 longitude TEXT,
                 status TEXT,
                 speed TEXT,
                 state TEXT,
                 PRIMARY KEY (base_url, user_email, profile, location_id)
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 base_url TEXT NOT NULL,
                 user_email TEXT NOT NULL,
                 profile TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 start_date_time TEXT,
                 end_date_time TEXT,
                 status TEXT,
                 location_id TEXT,
                 evse_uid TEXT,
                 connector_id TEXT,
                 token_uid TEXT,
                 kwh REAL,
                 PRIMARY KEY (base_url, user_email, profile, session_id)
             );
             CREATE TABLE IF NOT EXISTS tokens (
                 base_url TEXT NOT NULL,
                 user_email TEXT NOT NULL,
                 profile TEXT NOT NULL,
                 token_key TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 token_uid TEXT,
                 PRIMARY KEY (base_url, user_email, profile, token_key)
             );
             CREATE TABLE IF NOT EXISTS ocpp_logs (
                 base_url TEXT NOT NULL,
                 user_email TEXT NOT NULL,
                 profile TEXT NOT NULL,
                 log_key TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 log_id TEXT,
                 timestamp TEXT,
                 message_type TEXT,
                 direction TEXT,
                 PRIMARY KEY (base_url, user_email, profile, log_key)
             );
             CREATE TABLE IF NOT EXISTS json_resources (
                 base_url TEXT NOT NULL,
                 user_email TEXT NOT NULL,
                 profile TEXT NOT NULL,
                 resource TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 PRIMARY KEY (base_url, user_email, profile, resource, scope)
             );
             CREATE INDEX IF NOT EXISTS idx_cache_entries_scope_resource
                 ON cache_entries(scope, resource);
             CREATE INDEX IF NOT EXISTS idx_locations_scope ON locations(scope);
             CREATE INDEX IF NOT EXISTS idx_sessions_scope_start ON sessions(scope, start_date_time);
             CREATE INDEX IF NOT EXISTS idx_tokens_scope ON tokens(scope);
             CREATE INDEX IF NOT EXISTS idx_ocpp_logs_scope_timestamp ON ocpp_logs(scope, timestamp);
             CREATE INDEX IF NOT EXISTS idx_json_resources_scope ON json_resources(scope);",
        )
        .map_err(|err| format!("failed to initialize cache schema: {err}"))?;

        Ok(Self { conn })
    }

    fn get<T>(&mut self, lookup: &CacheLookup, spec: &CacheSpec) -> Result<Option<T>, String>
    where
        T: DeserializeOwned,
    {
        let Some(user_email) = lookup.user_email.as_deref() else {
            return Ok(None);
        };

        let params = normalize_params(&spec.params);
        let scope = scope_string(&lookup.base_url, user_email, spec.resource, &params)?;

        let row = if let Some(profile) = lookup.profile.as_deref() {
            let key = key_string(&scope, profile)?;
            self.conn
                .query_row(
                    "SELECT key, payload_json, fetched_at, expires_at
                     FROM cache_entries
                     WHERE key = ?1",
                    params![key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(|err| format!("failed to read cache entry: {err}"))?
        } else {
            self.conn
                .query_row(
                    "SELECT key, payload_json, fetched_at, expires_at
                     FROM cache_entries
                     WHERE scope = ?1 AND resource = ?2
                     ORDER BY fetched_at DESC
                     LIMIT 1",
                    params![scope, spec.resource],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(|err| format!("failed to read cache entry: {err}"))?
        };

        let Some((key, payload_json, fetched_at_epoch_ms, expires_at_epoch_ms)) = row else {
            return Ok(None);
        };

        match serde_json::from_str(&payload_json) {
            Ok(value) => {
                let _ = fetched_at_epoch_ms;
                let _ = expires_at_epoch_ms;
                Ok(Some(value))
            }
            Err(_) => {
                self.conn
                    .execute("DELETE FROM cache_entries WHERE key = ?1", params![key])
                    .map_err(|err| format!("failed to remove corrupt cache entry: {err}"))?;
                Ok(None)
            }
        }
    }

    fn put<T>(&mut self, lookup: &CacheLookup, spec: &CacheSpec, value: &T) -> Result<(), String>
    where
        T: Serialize,
    {
        let Some(user_email) = lookup.user_email.as_deref() else {
            return Ok(());
        };
        let Some(profile) = lookup.profile.as_deref() else {
            return Ok(());
        };

        let params_map = normalize_params(&spec.params);
        let scope = scope_string(&lookup.base_url, user_email, spec.resource, &params_map)?;
        let key = key_string(&scope, profile)?;
        let payload_json = serde_json::to_string(value)
            .map_err(|err| format!("failed to serialize cache payload: {err}"))?;
        let payload_value: Value = serde_json::from_str(&payload_json)
            .map_err(|err| format!("failed to reparse cache payload: {err}"))?;
        let fetched_at_epoch_ms = now_epoch_ms();
        let ttl_ms = i64::try_from(spec.ttl.as_millis()).unwrap_or(i64::MAX);
        let expires_at_epoch_ms = fetched_at_epoch_ms.saturating_add(ttl_ms);
        let tx = self
            .conn
            .transaction()
            .map_err(|err| format!("failed to start cache transaction: {err}"))?;

        tx.execute(
            "INSERT INTO cache_entries (
                    key, resource, scope, payload_json, fetched_at, expires_at, etag_or_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
                 ON CONFLICT(key) DO UPDATE SET
                    resource = excluded.resource,
                    scope = excluded.scope,
                    payload_json = excluded.payload_json,
                    fetched_at = excluded.fetched_at,
                    expires_at = excluded.expires_at,
                    etag_or_version = excluded.etag_or_version",
            params![
                key,
                spec.resource,
                scope,
                payload_json,
                fetched_at_epoch_ms,
                expires_at_epoch_ms
            ],
        )
        .map_err(|err| format!("failed to write cache entry: {err}"))?;
        sync_domain_tables(
            &tx,
            lookup,
            spec.resource,
            &scope,
            &payload_value,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        )?;
        tx.commit()
            .map_err(|err| format!("failed to commit cache transaction: {err}"))?;

        Ok(())
    }
}

fn sync_domain_tables(
    tx: &rusqlite::Transaction<'_>,
    lookup: &CacheLookup,
    resource: &str,
    scope: &str,
    payload: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    let Some(user_email) = lookup.user_email.as_deref() else {
        return Ok(());
    };
    let Some(profile) = lookup.profile.as_deref() else {
        return Ok(());
    };
    let base_url = lookup.base_url.as_str();

    match resource {
        "locations" => sync_locations(
            tx,
            base_url,
            user_email,
            profile,
            scope,
            payload,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        ),
        "location" => sync_single_location(
            tx,
            base_url,
            user_email,
            profile,
            scope,
            payload,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        ),
        "sessions" => sync_sessions(
            tx,
            base_url,
            user_email,
            profile,
            scope,
            payload,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        ),
        "tokens" => sync_tokens(
            tx,
            base_url,
            user_email,
            profile,
            scope,
            payload,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        ),
        "logs" => sync_ocpp_logs(
            tx,
            base_url,
            user_email,
            profile,
            scope,
            payload,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        ),
        "location_analytics" | "location_geojson" | "ocpi_logs" => sync_json_resource(
            tx,
            base_url,
            user_email,
            profile,
            resource,
            scope,
            payload,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        ),
        _ => Ok(()),
    }
}

fn sync_locations(
    tx: &rusqlite::Transaction<'_>,
    base_url: &str,
    user_email: &str,
    profile: &str,
    scope: &str,
    payload: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    let items = payload
        .as_array()
        .ok_or_else(|| "locations payload was not an array".to_string())?;
    tx.execute("DELETE FROM locations WHERE scope = ?1", params![scope])
        .map_err(|err| format!("failed to clear locations scope: {err}"))?;

    for item in items {
        upsert_location(
            tx,
            base_url,
            user_email,
            profile,
            scope,
            item,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
        )?;
    }
    Ok(())
}

fn sync_single_location(
    tx: &rusqlite::Transaction<'_>,
    base_url: &str,
    user_email: &str,
    profile: &str,
    scope: &str,
    payload: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    upsert_location(
        tx,
        base_url,
        user_email,
        profile,
        scope,
        payload,
        fetched_at_epoch_ms,
        expires_at_epoch_ms,
    )
}

fn upsert_location(
    tx: &rusqlite::Transaction<'_>,
    base_url: &str,
    user_email: &str,
    profile: &str,
    scope: &str,
    item: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    let location_id = item
        .get("location_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "location entry missing id".to_string())?;
    let coordinates = item.get("coordinates").and_then(Value::as_object);
    let latitude = coordinates
        .and_then(|v| v.get("latitude"))
        .and_then(json_scalar_to_string);
    let longitude = coordinates
        .and_then(|v| v.get("longitude"))
        .and_then(json_scalar_to_string);
    let status = item.get("status").and_then(json_scalar_to_string);
    let speed = item.get("speed").and_then(json_scalar_to_string);
    let state = item.get("state").and_then(json_scalar_to_string);
    let payload_json = serde_json::to_string(item)
        .map_err(|err| format!("failed to serialize location row: {err}"))?;

    tx.execute(
        "INSERT INTO locations (
            base_url, user_email, profile, location_id, scope, payload_json, fetched_at, expires_at,
            latitude, longitude, status, speed, state
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(base_url, user_email, profile, location_id) DO UPDATE SET
            scope = excluded.scope,
            payload_json = excluded.payload_json,
            fetched_at = excluded.fetched_at,
            expires_at = excluded.expires_at,
            latitude = excluded.latitude,
            longitude = excluded.longitude,
            status = excluded.status,
            speed = excluded.speed,
            state = excluded.state",
        params![
            base_url,
            user_email,
            profile,
            location_id,
            scope,
            payload_json,
            fetched_at_epoch_ms,
            expires_at_epoch_ms,
            latitude,
            longitude,
            status,
            speed,
            state
        ],
    )
    .map_err(|err| format!("failed to upsert location row: {err}"))?;
    Ok(())
}

fn sync_sessions(
    tx: &rusqlite::Transaction<'_>,
    base_url: &str,
    user_email: &str,
    profile: &str,
    scope: &str,
    payload: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    let items = payload
        .as_array()
        .ok_or_else(|| "sessions payload was not an array".to_string())?;
    tx.execute("DELETE FROM sessions WHERE scope = ?1", params![scope])
        .map_err(|err| format!("failed to clear sessions scope: {err}"))?;

    for item in items {
        let session_id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "session entry missing id".to_string())?;
        let payload_json = serde_json::to_string(item)
            .map_err(|err| format!("failed to serialize session row: {err}"))?;
        let token_uid = item
            .get("cdr_token")
            .and_then(|token| token.get("uid"))
            .and_then(json_scalar_to_string);
        let kwh = item.get("kwh").and_then(Value::as_f64);

        tx.execute(
            "INSERT INTO sessions (
                base_url, user_email, profile, session_id, scope, payload_json, fetched_at, expires_at,
                start_date_time, end_date_time, status, location_id, evse_uid, connector_id, token_uid, kwh
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(base_url, user_email, profile, session_id) DO UPDATE SET
                scope = excluded.scope,
                payload_json = excluded.payload_json,
                fetched_at = excluded.fetched_at,
                expires_at = excluded.expires_at,
                start_date_time = excluded.start_date_time,
                end_date_time = excluded.end_date_time,
                status = excluded.status,
                location_id = excluded.location_id,
                evse_uid = excluded.evse_uid,
                connector_id = excluded.connector_id,
                token_uid = excluded.token_uid,
                kwh = excluded.kwh",
            params![
                base_url,
                user_email,
                profile,
                session_id,
                scope,
                payload_json,
                fetched_at_epoch_ms,
                expires_at_epoch_ms,
                item.get("start_date_time").and_then(json_scalar_to_string),
                item.get("end_date_time").and_then(json_scalar_to_string),
                item.get("status").and_then(json_scalar_to_string),
                item.get("location_id").and_then(json_scalar_to_string),
                item.get("evse_uid").and_then(json_scalar_to_string),
                item.get("connector_id").and_then(json_scalar_to_string),
                token_uid,
                kwh
            ],
        )
        .map_err(|err| format!("failed to upsert session row: {err}"))?;
    }
    Ok(())
}

fn sync_tokens(
    tx: &rusqlite::Transaction<'_>,
    base_url: &str,
    user_email: &str,
    profile: &str,
    scope: &str,
    payload: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    let items = payload
        .as_array()
        .ok_or_else(|| "tokens payload was not an array".to_string())?;
    tx.execute("DELETE FROM tokens WHERE scope = ?1", params![scope])
        .map_err(|err| format!("failed to clear tokens scope: {err}"))?;

    for (index, item) in items.iter().enumerate() {
        let token_uid = item.get("token_uid").and_then(json_scalar_to_string);
        let token_key = token_uid
            .clone()
            .unwrap_or_else(|| format!("scope:{scope}:index:{index}"));
        let payload_json = serde_json::to_string(item)
            .map_err(|err| format!("failed to serialize token row: {err}"))?;

        tx.execute(
            "INSERT INTO tokens (
                base_url, user_email, profile, token_key, scope, payload_json, fetched_at, expires_at, token_uid
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(base_url, user_email, profile, token_key) DO UPDATE SET
                scope = excluded.scope,
                payload_json = excluded.payload_json,
                fetched_at = excluded.fetched_at,
                expires_at = excluded.expires_at,
                token_uid = excluded.token_uid",
            params![
                base_url,
                user_email,
                profile,
                token_key,
                scope,
                payload_json,
                fetched_at_epoch_ms,
                expires_at_epoch_ms,
                token_uid
            ],
        )
        .map_err(|err| format!("failed to upsert token row: {err}"))?;
    }
    Ok(())
}

fn sync_ocpp_logs(
    tx: &rusqlite::Transaction<'_>,
    base_url: &str,
    user_email: &str,
    profile: &str,
    scope: &str,
    payload: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    let items = payload
        .as_array()
        .ok_or_else(|| "logs payload was not an array".to_string())?;
    tx.execute("DELETE FROM ocpp_logs WHERE scope = ?1", params![scope])
        .map_err(|err| format!("failed to clear logs scope: {err}"))?;

    for (index, item) in items.iter().enumerate() {
        let log_id = item.get("id").and_then(json_scalar_to_string);
        let timestamp = item.get("timestamp").and_then(json_scalar_to_string);
        let log_key = log_id
            .clone()
            .or_else(|| timestamp.clone())
            .map(|base| format!("{base}:{index}"))
            .unwrap_or_else(|| format!("scope:{scope}:index:{index}"));
        let payload_json = serde_json::to_string(item)
            .map_err(|err| format!("failed to serialize log row: {err}"))?;

        tx.execute(
            "INSERT INTO ocpp_logs (
                base_url, user_email, profile, log_key, scope, payload_json, fetched_at, expires_at,
                log_id, timestamp, message_type, direction
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(base_url, user_email, profile, log_key) DO UPDATE SET
                scope = excluded.scope,
                payload_json = excluded.payload_json,
                fetched_at = excluded.fetched_at,
                expires_at = excluded.expires_at,
                log_id = excluded.log_id,
                timestamp = excluded.timestamp,
                message_type = excluded.message_type,
                direction = excluded.direction",
            params![
                base_url,
                user_email,
                profile,
                log_key,
                scope,
                payload_json,
                fetched_at_epoch_ms,
                expires_at_epoch_ms,
                log_id,
                timestamp,
                item.get("message_type").and_then(json_scalar_to_string),
                item.get("direction").and_then(json_scalar_to_string)
            ],
        )
        .map_err(|err| format!("failed to upsert log row: {err}"))?;
    }
    Ok(())
}

fn sync_json_resource(
    tx: &rusqlite::Transaction<'_>,
    base_url: &str,
    user_email: &str,
    profile: &str,
    resource: &str,
    scope: &str,
    payload: &Value,
    fetched_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
) -> Result<(), String> {
    let payload_json = serde_json::to_string(payload)
        .map_err(|err| format!("failed to serialize json resource row: {err}"))?;
    tx.execute(
        "INSERT INTO json_resources (
            base_url, user_email, profile, resource, scope, payload_json, fetched_at, expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(base_url, user_email, profile, resource, scope) DO UPDATE SET
            payload_json = excluded.payload_json,
            fetched_at = excluded.fetched_at,
            expires_at = excluded.expires_at",
        params![
            base_url,
            user_email,
            profile,
            resource,
            scope,
            payload_json,
            fetched_at_epoch_ms,
            expires_at_epoch_ms
        ],
    )
    .map_err(|err| format!("failed to upsert json resource row: {err}"))?;
    Ok(())
}

fn json_scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(v) => Some(v.clone()),
        Value::Number(v) => Some(v.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }
}

fn normalize_params(params: &[(&'static str, String)]) -> BTreeMap<&'static str, String> {
    let mut normalized = BTreeMap::new();
    for (key, value) in params {
        normalized.insert(*key, value.clone());
    }
    normalized
}

fn scope_string(
    base_url: &str,
    user_email: &str,
    resource: &str,
    params: &BTreeMap<&'static str, String>,
) -> Result<String, String> {
    serde_json::to_string(&serde_json::json!({
        "version": 1,
        "base_url": base_url,
        "user_email": user_email,
        "resource": resource,
        "params": params,
    }))
    .map_err(|err| format!("failed to encode cache scope: {err}"))
}

fn key_string(scope: &str, profile: &str) -> Result<String, String> {
    serde_json::to_string(&serde_json::json!({
        "scope": scope,
        "profile": profile,
    }))
    .map_err(|err| format!("failed to encode cache key: {err}"))
}

pub fn default_cache_db_path() -> Result<PathBuf, String> {
    if let Some(override_dir) = std::env::var_os(CACHE_ENV_DIR) {
        return Ok(PathBuf::from(override_dir).join(CACHE_DB_NAME));
    }

    let project_dirs = ProjectDirs::from("", "", "mobie")
        .ok_or_else(|| "failed to determine cache directory for mobie".to_string())?;
    Ok(project_dirs.cache_dir().join(CACHE_DB_NAME))
}

#[cfg(test)]
pub fn default_cache_db_path_with_env(
    env_value: Option<&Path>,
    fallback: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(path) = env_value {
        return Ok(path.join(CACHE_DB_NAME));
    }

    fallback()
        .map(|path| path.join(CACHE_DB_NAME))
        .ok_or_else(|| "failed to determine cache directory for mobie".to_string())
}

fn now_epoch_ms() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn cache_path_uses_override_directory_when_present() {
        let tmp = tempdir().unwrap();
        let path = default_cache_db_path_with_env(Some(tmp.path()), || None).unwrap();
        assert_eq!(path, tmp.path().join("cache.db"));
    }

    #[test]
    fn cache_path_defaults_to_mobie_cache_db() {
        let path =
            default_cache_db_path_with_env(None, || Some(PathBuf::from("/tmp/mobie"))).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/mobie/cache.db"));
    }

    #[test]
    fn cache_meta_reports_staleness_against_expiry() {
        let meta = CacheEntryMeta {
            fetched_at_epoch_ms: 100,
            expires_at_epoch_ms: 200,
        };

        assert_eq!(meta.fetched_at_epoch_ms, 100);
        assert!(!meta.is_stale_at(199));
        assert!(meta.is_stale_at(200));
    }

    #[test]
    fn cache_scopes_are_normalized_by_parameter_name() {
        let left = scope_string(
            "https://pgm.mobie.pt",
            "user@example.com",
            "sessions",
            &normalize_params(&[
                ("limit", "200".to_string()),
                ("location", "LOC-1".to_string()),
            ]),
        )
        .unwrap();
        let right = scope_string(
            "https://pgm.mobie.pt",
            "user@example.com",
            "sessions",
            &normalize_params(&[
                ("location", "LOC-1".to_string()),
                ("limit", "200".to_string()),
            ]),
        )
        .unwrap();

        assert_eq!(left, right);
    }

    #[test]
    fn cache_entries_are_namespaced_by_base_url_and_user() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("cache.db");
        let mut store = CacheStore::open_at(path).unwrap();
        let spec = CacheSpec {
            resource: "locations",
            ttl: Duration::from_secs(60),
            params: vec![("limit", "0".to_string())],
        };

        let alice = CacheLookup {
            base_url: "https://one.example".to_string(),
            user_email: Some("alice@example.com".to_string()),
            profile: Some("DPC".to_string()),
        };
        let bob = CacheLookup {
            base_url: "https://two.example".to_string(),
            user_email: Some("bob@example.com".to_string()),
            profile: Some("DPC".to_string()),
        };

        store
            .put(
                &alice,
                &spec,
                &vec![serde_json::json!({"location_id": "alice-location"})],
            )
            .unwrap();
        store
            .put(
                &bob,
                &spec,
                &vec![serde_json::json!({"location_id": "bob-location"})],
            )
            .unwrap();

        let alice_value = store.get::<Vec<Value>>(&alice, &spec).unwrap().unwrap();
        let bob_value = store.get::<Vec<Value>>(&bob, &spec).unwrap().unwrap();

        assert_eq!(alice_value[0]["location_id"], "alice-location");
        assert_eq!(bob_value[0]["location_id"], "bob-location");
    }

    #[test]
    fn corrupt_cache_rows_are_ignored_and_deleted() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("cache.db");
        let mut store = CacheStore::open_at(path).unwrap();
        let spec = CacheSpec {
            resource: "locations",
            ttl: Duration::from_secs(60),
            params: vec![("limit", "0".to_string())],
        };
        let lookup = CacheLookup {
            base_url: "https://pgm.mobie.pt".to_string(),
            user_email: Some("user@example.com".to_string()),
            profile: Some("DPC".to_string()),
        };
        let scope = scope_string(
            &lookup.base_url,
            lookup.user_email.as_deref().unwrap(),
            spec.resource,
            &normalize_params(&spec.params),
        )
        .unwrap();
        let key = key_string(&scope, lookup.profile.as_deref().unwrap()).unwrap();

        store
            .conn
            .execute(
                "INSERT INTO cache_entries (
                    key, resource, scope, payload_json, fetched_at, expires_at, etag_or_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                params![
                    key.clone(),
                    spec.resource,
                    scope,
                    "{invalid",
                    10_i64,
                    20_i64
                ],
            )
            .unwrap();

        let value = store.get::<Vec<String>>(&lookup, &spec).unwrap();
        assert!(value.is_none());

        let remaining: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM cache_entries WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn domain_tables_are_populated_for_sessions_and_locations() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("cache.db");
        let mut store = CacheStore::open_at(path).unwrap();
        let lookup = CacheLookup {
            base_url: "https://pgm.mobie.pt".to_string(),
            user_email: Some("user@example.com".to_string()),
            profile: Some("DPC".to_string()),
        };

        store
            .put(
                &lookup,
                &CacheSpec {
                    resource: "locations",
                    ttl: Duration::from_secs(60),
                    params: vec![("limit", "0".to_string())],
                },
                &vec![serde_json::json!({
                    "location_id": "LOC-1",
                    "coordinates": {"latitude": "1.0", "longitude": "2.0"},
                    "status": "AVAILABLE"
                })],
            )
            .unwrap();

        store
            .put(
                &lookup,
                &CacheSpec {
                    resource: "sessions",
                    ttl: Duration::from_secs(60),
                    params: vec![("location", "LOC-1".to_string())],
                },
                &vec![serde_json::json!({
                    "id": "sess-1",
                    "start_date_time": "2026-03-01T00:00:00Z",
                    "status": "COMPLETED",
                    "location_id": "LOC-1",
                    "cdr_token": {"uid": "token-1"},
                    "kwh": 12.5
                })],
            )
            .unwrap();

        let locations: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM locations", [], |row| row.get(0))
            .unwrap();
        let sessions: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        let token_uid: String = store
            .conn
            .query_row(
                "SELECT token_uid FROM sessions WHERE session_id = 'sess-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(locations, 1);
        assert_eq!(sessions, 1);
        assert_eq!(token_uid, "token-1");
    }
}
