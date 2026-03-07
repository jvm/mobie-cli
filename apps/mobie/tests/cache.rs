use std::process::Command;

use rusqlite::Connection;
use tempfile::tempdir;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn login_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "data": {
            "bearer": {
                "access_token": "secret-access-token",
                "refresh_token": "secret-refresh-token",
                "expires_in": 3600
            },
            "user": {
                "email": "user@example.com",
                "roles": [{ "profile": "DPC" }]
            }
        },
        "status_code": 1000,
        "status_message": "Success",
        "timestamp": "2025-01-01T00:00:00Z"
    }))
}

async fn mock_login(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/login"))
        .and(body_json(serde_json::json!({
            "email": "user@example.com",
            "password": "password"
        })))
        .respond_with(login_response())
        .mount(server)
        .await;
}

fn cli(base_url: &str, cache_dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("mobie"));
    cmd.env("MOBIE_BASE_URL", base_url);
    cmd.env("MOBIE_EMAIL", "user@example.com");
    cmd.env("MOBIE_PASSWORD", "password");
    cmd.env("MOBIE_CACHE_DIR", cache_dir);
    cmd
}

fn cli_without_credentials(base_url: &str, cache_dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("mobie"));
    cmd.env("MOBIE_BASE_URL", base_url);
    cmd.env("MOBIE_CACHE_DIR", cache_dir);
    cmd
}

#[tokio::test]
async fn cache_miss_writes_sqlite_and_offline_hit_reuses_same_json_output() {
    let cache_dir = tempdir().unwrap();
    let server = MockServer::start().await;
    let base_url = server.uri();
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                { "id": "LOC-1" },
                { "id": "LOC-2" }
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let first = cli(&base_url, cache_dir.path())
        .args(["--json", "locations", "list"])
        .output()
        .expect("first run");
    assert!(first.status.success());

    let db_path = cache_dir.path().join("cache.db");
    assert!(db_path.exists());

    drop(server);

    let second = cli(&base_url, cache_dir.path())
        .args(["--json", "locations", "list"])
        .output()
        .expect("second run");
    assert!(second.status.success());
    assert_eq!(first.stdout, second.stdout);
}

#[tokio::test]
async fn schema_contains_cache_meta_and_sync_windows() {
    let cache_dir = tempdir().unwrap();
    let server = MockServer::start().await;
    let base_url = server.uri();
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "LOC-1" }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&base_url, cache_dir.path())
        .args(["--json", "locations", "list"])
        .output()
        .expect("seed run");
    assert!(output.status.success());

    let conn = Connection::open(cache_dir.path().join("cache.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap();
    let table_names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    for expected in [
        "cache_entries",
        "locations",
        "sessions",
        "ocpp_logs",
        "cache_meta",
        "sync_windows",
    ] {
        assert!(
            table_names.iter().any(|name| name == expected),
            "expected table {expected} to exist, tables were {table_names:?}"
        );
    }

    let mut columns_stmt = conn.prepare("PRAGMA table_info(ocpp_logs)").unwrap();
    let columns = columns_stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    for expected in [
        "payload_json",
        "fetched_at",
        "expires_at",
        "fingerprint",
        "sort_key",
    ] {
        assert!(
            columns.iter().any(|column| column == expected),
            "expected ocpp_logs column {expected} to exist, columns were {columns:?}"
        );
    }
}

#[tokio::test]
async fn different_query_parameters_create_distinct_cache_entries() {
    let cache_dir = tempdir().unwrap();
    let server = MockServer::start().await;
    let base_url = server.uri();
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "LOC-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "sess-1", "start_date_time": "2025-01-01T10:00:00Z" }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .and(query_param("locationId", "LOC-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "LOC-1"))
        .and(query_param("dateFrom", "2025-01-02T00:00:00.000Z"))
        .and(query_param("dateTo", "2025-01-02T23:59:59.999Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "sess-2", "start_date_time": "2025-01-02T10:00:00Z" }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .and(query_param("locationId", "LOC-1"))
        .and(query_param("dateFrom", "2025-01-02T00:00:00.000Z"))
        .and(query_param("dateTo", "2025-01-02T23:59:59.999Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let first = cli(&base_url, cache_dir.path())
        .args([
            "--json",
            "sessions",
            "list",
            "--location",
            "LOC-1",
            "--limit",
            "2",
        ])
        .output()
        .expect("first query");
    assert!(first.status.success());

    let second = cli(&base_url, cache_dir.path())
        .args([
            "--json",
            "sessions",
            "list",
            "--location",
            "LOC-1",
            "--limit",
            "2",
            "--from",
            "2025-01-02",
            "--to",
            "2025-01-02",
        ])
        .output()
        .expect("second query");
    assert!(second.status.success());

    let conn = Connection::open(cache_dir.path().join("cache.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cache_entries WHERE resource = 'sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn cache_entries_do_not_cross_user_or_base_url_boundaries() {
    let cache_dir = tempdir().unwrap();
    let server = MockServer::start().await;
    let base_url = server.uri();
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "LOC-1" }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let seeded = cli(&base_url, cache_dir.path())
        .args(["--json", "locations", "list"])
        .output()
        .expect("seed run");
    assert!(seeded.status.success());

    drop(server);

    let different_user = Command::new(assert_cmd::cargo::cargo_bin!("mobie"))
        .env("MOBIE_BASE_URL", &base_url)
        .env("MOBIE_EMAIL", "other@example.com")
        .env("MOBIE_PASSWORD", "password")
        .env("MOBIE_CACHE_DIR", cache_dir.path())
        .args(["--json", "locations", "list"])
        .output()
        .expect("different user run");
    assert!(!different_user.status.success());

    let different_base = Command::new(assert_cmd::cargo::cargo_bin!("mobie"))
        .env("MOBIE_BASE_URL", "http://127.0.0.1:9")
        .env("MOBIE_EMAIL", "user@example.com")
        .env("MOBIE_PASSWORD", "password")
        .env("MOBIE_CACHE_DIR", cache_dir.path())
        .args(["--json", "locations", "list"])
        .output()
        .expect("different base run");
    assert!(!different_base.status.success());
}

#[tokio::test]
async fn unwritable_cache_directory_degrades_to_live_fetch_with_terminal_warning() {
    let blocked_parent = tempdir().unwrap();
    let blocked_path = blocked_parent.path().join("not-a-dir");
    std::fs::write(&blocked_path, "blocked").unwrap();

    let server = MockServer::start().await;
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "LOC-1" }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = Command::new(assert_cmd::cargo::cargo_bin!("mobie"))
        .env("MOBIE_BASE_URL", server.uri())
        .env("MOBIE_EMAIL", "user@example.com")
        .env("MOBIE_PASSWORD", "password")
        .env("MOBIE_CACHE_DIR", &blocked_path)
        .args(["locations", "list"])
        .output()
        .expect("warning run");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: cache disabled:"));
}

#[tokio::test]
async fn response_cache_policy_keeps_sessions_and_logs_canonical_state_after_snapshot_deletion() {
    let cache_dir = tempdir().unwrap();
    let server = MockServer::start().await;
    let base_url = server.uri();
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "sess-1",
                "start_date_time": "2025-01-01T10:00:00Z",
                "end_date_time": "2025-01-01T11:00:00Z",
                "status": "COMPLETED"
            }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/logs/ocpp"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("error", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "MOBI-AAA-00001",
                "messageType": "Heartbeat",
                "direction": "Response",
                "timestamp": "2025-01-03T10:00:00Z",
                "logs": "{\"currentTime\":\"2025-01-03T10:00:00Z\"}"
            }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/logs/ocpp"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .and(query_param("error", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let sessions_seed = cli(&base_url, cache_dir.path())
        .args([
            "--json",
            "sessions",
            "list",
            "--location",
            "EVSE-1",
            "--limit",
            "2",
        ])
        .output()
        .expect("seed sessions");
    assert!(sessions_seed.status.success());

    let logs_seed = cli(&base_url, cache_dir.path())
        .args(["--json", "logs", "list", "--limit", "2", "--error-only"])
        .output()
        .expect("seed logs");
    assert!(logs_seed.status.success());

    let conn = Connection::open(cache_dir.path().join("cache.db")).unwrap();
    conn.execute(
        "DELETE FROM cache_entries WHERE resource IN ('sessions', 'logs')",
        [],
    )
    .unwrap();

    let snapshot_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cache_entries WHERE resource IN ('sessions', 'logs')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let session_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    let log_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM ocpp_logs", [], |row| row.get(0))
        .unwrap();
    let sync_window_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_windows WHERE resource IN ('sessions', 'logs')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(snapshot_rows, 0);
    assert_eq!(session_rows, 1);
    assert_eq!(log_rows, 1);
    assert_eq!(sync_window_rows, 2);
}

#[tokio::test]
async fn stale_session_cache_is_reused_when_refresh_fails() {
    let cache_dir = tempdir().unwrap();
    let server = MockServer::start().await;
    let base_url = server.uri();
    mock_login(&server).await;

    let login = cli(&base_url, cache_dir.path())
        .args(["auth", "login"])
        .output()
        .expect("seed stored session");
    assert!(login.status.success());

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "sess-1",
                "start_date_time": "2025-01-01T10:00:00Z",
                "end_date_time": "2025-01-01T11:00:00Z",
                "status": "COMPLETED",
                "location_id": "EVSE-1"
            }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let seeded = cli_without_credentials(&base_url, cache_dir.path())
        .args([
            "--json",
            "sessions",
            "list",
            "--location",
            "EVSE-1",
            "--limit",
            "2",
        ])
        .output()
        .expect("seed sessions");
    assert!(seeded.status.success());

    let conn = Connection::open(cache_dir.path().join("cache.db")).unwrap();
    conn.execute(
        "UPDATE sync_windows
         SET last_success_epoch_ms = 0
         WHERE resource = 'sessions'",
        [],
    )
    .unwrap();
    drop(conn);
    server.reset().await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream unavailable"))
        .expect(3)
        .mount(&server)
        .await;

    let offline = cli_without_credentials(&base_url, cache_dir.path())
        .args([
            "--json",
            "sessions",
            "list",
            "--location",
            "EVSE-1",
            "--limit",
            "2",
        ])
        .output()
        .expect("stale cache reuse after refresh failure");

    assert!(
        offline.status.success(),
        "expected stale canonical cache fallback, stderr was: {}",
        String::from_utf8_lossy(&offline.stderr)
    );

    let body: serde_json::Value = serde_json::from_slice(&offline.stdout).unwrap();
    assert_eq!(body["data"][0]["id"], "sess-1");
    assert_eq!(body["meta"]["freshness"]["state"], "stale");
}

#[tokio::test]
async fn canonical_refresh_preserves_other_profiles_rows_for_same_scope() {
    let cache_dir = tempdir().unwrap();
    let server = MockServer::start().await;
    let base_url = server.uri();
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "sess-dpc",
                "start_date_time": "2025-01-01T10:00:00Z",
                "end_date_time": "2025-01-01T11:00:00Z",
                "status": "COMPLETED",
                "location_id": "EVSE-1"
            }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(2)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .expect(2)
        .mount(&server)
        .await;

    let seeded = cli(&base_url, cache_dir.path())
        .args([
            "--json",
            "sessions",
            "list",
            "--location",
            "EVSE-1",
            "--limit",
            "2",
        ])
        .output()
        .expect("seed canonical sessions");
    assert!(seeded.status.success());

    let conn = Connection::open(cache_dir.path().join("cache.db")).unwrap();
    let scope: String = conn
        .query_row(
            "SELECT scope FROM sessions WHERE profile = 'DPC' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let mop_payload = serde_json::json!({
        "id": "sess-mop",
        "start_date_time": "2025-01-01T12:00:00Z",
        "end_date_time": "2025-01-01T13:00:00Z",
        "status": "COMPLETED",
        "location_id": "EVSE-1"
    })
    .to_string();

    conn.execute(
        "INSERT INTO sessions (
            base_url, user_email, profile, session_id, scope, payload_json, fetched_at, expires_at,
            start_date_time, end_date_time, status, location_id, evse_uid, connector_id, token_uid, kwh
        ) VALUES (?1, ?2, 'MOP', 'sess-mop', ?3, ?4, 1_000, 9_999_999, ?5, ?6, 'COMPLETED', 'EVSE-1', NULL, NULL, NULL, NULL)",
        rusqlite::params![
            base_url,
            "user@example.com",
            scope,
            mop_payload,
            "2025-01-01T12:00:00Z",
            "2025-01-01T13:00:00Z"
        ],
    )
    .unwrap();
    conn.execute(
        "UPDATE sync_windows
         SET last_success_epoch_ms = 0
         WHERE resource = 'sessions'",
        [],
    )
    .unwrap();
    drop(conn);

    let refreshed = cli(&base_url, cache_dir.path())
        .args([
            "--json",
            "sessions",
            "list",
            "--location",
            "EVSE-1",
            "--limit",
            "2",
        ])
        .output()
        .expect("refresh canonical sessions");
    assert!(refreshed.status.success());

    let conn = Connection::open(cache_dir.path().join("cache.db")).unwrap();
    let mop_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE profile = 'MOP' AND session_id = 'sess-mop'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        mop_rows, 1,
        "refresh for one profile should not purge canonical rows for another profile with the same scope"
    );
}

#[tokio::test]
async fn migration_backfills_canonical_tables_from_legacy_cache_entries() {
    let cache_dir = tempdir().unwrap();
    let db_path = cache_dir.path().join("cache.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE cache_entries (
            key TEXT PRIMARY KEY,
            resource TEXT NOT NULL,
            scope TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            fetched_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            etag_or_version TEXT
        );",
    )
    .unwrap();

    let base_url = "http://127.0.0.1:9";
    let sessions_scope = serde_json::json!({
        "version": 1,
        "base_url": base_url,
        "user_email": "user@example.com",
        "resource": "sessions",
        "params": {
            "location": "EVSE-1",
            "limit": "1",
            "order": "oldest-first-v1",
            "from": "-",
            "to": "-"
        }
    })
    .to_string();
    let tokens_scope = serde_json::json!({
        "version": 1,
        "base_url": base_url,
        "user_email": "user@example.com",
        "resource": "tokens",
        "params": {
            "limit": "1"
        }
    })
    .to_string();
    let key = serde_json::json!({
        "scope": sessions_scope,
        "profile": "DPC"
    })
    .to_string();
    let token_key = serde_json::json!({
        "scope": tokens_scope,
        "profile": "DPC"
    })
    .to_string();

    conn.execute(
        "INSERT INTO cache_entries (key, resource, scope, payload_json, fetched_at, expires_at, etag_or_version)
         VALUES (?1, 'sessions', ?2, ?3, 1_000, 9_999_999, NULL)",
        rusqlite::params![
            key,
            sessions_scope,
            serde_json::json!([{
                "id": "sess-legacy",
                "start_date_time": "2025-01-01T10:00:00Z",
                "end_date_time": "2025-01-01T11:00:00Z",
                "status": "COMPLETED",
                "location_id": "EVSE-1"
            }])
            .to_string()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO cache_entries (key, resource, scope, payload_json, fetched_at, expires_at, etag_or_version)
         VALUES (?1, 'tokens', ?2, ?3, 2_000, 9_999_999, NULL)",
        rusqlite::params![
            token_key,
            tokens_scope,
            serde_json::json!([{ "token_uid": "token-legacy" }]).to_string()
        ],
    )
    .unwrap();
    drop(conn);

    let output = Command::new(assert_cmd::cargo::cargo_bin!("mobie"))
        .env("MOBIE_BASE_URL", base_url)
        .env("MOBIE_EMAIL", "user@example.com")
        .env("MOBIE_PASSWORD", "password")
        .env("MOBIE_CACHE_DIR", cache_dir.path())
        .args(["--json", "tokens", "list", "--limit", "1"])
        .output()
        .expect("migrated token read");
    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["data"][0]["token_uid"], "token-legacy");

    let conn = Connection::open(db_path).unwrap();
    let session_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    let schema_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cache_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let sync_window_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sync_windows", [], |row| row.get(0))
        .unwrap();
    assert_eq!(session_rows, 1);
    assert_eq!(schema_rows, 1);
    assert_eq!(sync_window_rows, 0);
}
