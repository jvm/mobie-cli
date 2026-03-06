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
