use assert_cmd::assert::OutputAssertExt;
use serde_json::Value;
use std::fs;
use std::process::Command;
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

fn mobie_command() -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("mobie"));
    let cache_dir = tempdir().unwrap().keep();
    cmd.env("MOBIE_CACHE_DIR", cache_dir);
    cmd
}

fn write_config(dir: &std::path::Path, contents: &str) {
    let config_dir = dir.join("mobie");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.toml"), contents).unwrap();
}

#[tokio::test]
async fn auth_check_json_returns_safe_fields_only() {
    let server = MockServer::start().await;
    mock_login(&server).await;

    let output = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "auth",
            "check",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["resource"], "auth");
    assert_eq!(value["data"]["email"], "user@example.com");
    assert_eq!(value["data"]["profile"], "DPC");
    assert!(value["data"].get("access_token").is_none());
}

#[test]
fn rejects_insecure_non_loopback_base_url() {
    let assert = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            "http://example.com",
            "--email",
            "user@example.com",
            "--json",
            "auth",
            "check",
        ])
        .assert()
        .failure();

    let stderr = assert.get_output().stderr.clone();
    let value: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid base url")
    );
}

#[tokio::test]
async fn locations_list_json_returns_envelope_and_count() {
    let server = MockServer::start().await;
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
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "locations",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["resource"], "locations");
    assert_eq!(value["meta"]["count"], 2);
    assert_eq!(value["data"][0]["location_id"], "LOC-1");
}

#[tokio::test]
async fn locations_get_accepts_positional_location_id() {
    let server = MockServer::start().await;
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations/LOC-55"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "id": "LOC-55",
                "status": "ACTIVE"
            },
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "location",
            "LOC-55",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["resource"], "location");
    assert_eq!(value["data"]["location_id"], "LOC-55");
}

#[tokio::test]
async fn locations_get_uses_default_location_from_xdg_config() {
    let server = MockServer::start().await;
    mock_login(&server).await;
    let xdg_dir = tempdir().unwrap();
    write_config(xdg_dir.path(), "default_location = \"LOC-55\"\n");

    Mock::given(method("GET"))
        .and(path("/api/locations/LOC-55"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "id": "LOC-55",
                "status": "ACTIVE"
            },
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "location",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["resource"], "location");
    assert_eq!(value["data"]["location_id"], "LOC-55");
}

#[test]
fn locations_get_without_location_or_config_is_structured_json_error() {
    let xdg_dir = tempdir().unwrap();

    let assert = mobie_command()
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .args(["--json", "location"])
        .assert()
        .failure();

    let stderr = assert.get_output().stderr.clone();
    let value: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("location is mandatory")
    );
}

#[tokio::test]
async fn json_errors_are_structured_and_sanitized() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "access_token": "leaky-token",
            "password": "super-secret"
        })))
        .mount(&server)
        .await;

    let assert = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "auth",
            "check",
        ])
        .assert()
        .failure();

    let stderr = assert.get_output().stderr.clone();
    let value: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "login_failed");
    assert!(value["error"]["body"].is_null());
    assert!(!String::from_utf8_lossy(&stderr).contains("leaky-token"));
    assert!(!String::from_utf8_lossy(&stderr).contains("super-secret"));
}

#[tokio::test]
async fn invalid_session_range_is_reported_as_structured_json() {
    let server = MockServer::start().await;
    mock_login(&server).await;

    let assert = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "sessions",
            "LOC-1",
            "--from",
            "2025-01-03",
            "--to",
            "2025-01-02",
        ])
        .assert()
        .failure();

    let stderr = assert.get_output().stderr.clone();
    let value: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid range")
    );
}

#[tokio::test]
async fn sessions_list_uses_default_location_from_xdg_config_in_json_mode() {
    let server = MockServer::start().await;
    mock_login(&server).await;
    let xdg_dir = tempdir().unwrap();
    write_config(xdg_dir.path(), "default_location = \"LOC-7\"\n");

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "1"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "LOC-7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "sess-7", "start_date_time": "2025-01-01T10:00:00Z" }],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "1"))
        .and(query_param("offset", "1"))
        .and(query_param("locationId", "LOC-7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "sessions",
            "--limit",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["resource"], "sessions");
    assert_eq!(value["data"][0]["id"], "sess-7");
}

#[test]
fn missing_location_without_config_is_structured_json_error() {
    let xdg_dir = tempdir().unwrap();

    let assert = mobie_command()
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .args(["--json", "sessions"])
        .assert()
        .failure();

    let stderr = assert.get_output().stderr.clone();
    let value: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("location is mandatory")
    );
}

#[tokio::test]
async fn entity_get_json_returns_single_object() {
    let server = MockServer::start().await;
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/entities/0315"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "code": "0315",
                "name": "Entity Name",
                "dpc": true
            },
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "entity",
            "0315",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["resource"], "entity");
    assert_eq!(value["data"]["code"], "0315");
    assert_eq!(value["data"]["dpc"], true);
}

#[tokio::test]
async fn location_analytics_json_returns_object() {
    let server = MockServer::start().await;
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations/analytics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "locationsTotalCount": 1,
                "evsesTotalCount": 2
            },
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "locations",
            "analytics",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["resource"], "location_analytics");
    assert_eq!(value["data"]["locationsTotalCount"], 1);
}

#[tokio::test]
async fn ords_list_json_returns_array() {
    let server = MockServer::start().await;
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/ords"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                { "cpe": "PT0001", "cpeStatus": "Integrated" }
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "ords",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["resource"], "ords");
    assert_eq!(value["meta"]["count"], 1);
    assert_eq!(value["data"][0]["cpe"], "PT0001");
}

#[tokio::test]
async fn logs_ocpi_json_returns_array() {
    let server = MockServer::start().await;
    mock_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/logs/ocpi"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                { "id": "log-1", "messageType": "PATCH" }
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/logs/ocpi"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = mobie_command()
        .env("MOBIE_PASSWORD", "password")
        .args([
            "--base-url",
            server.uri().as_str(),
            "--email",
            "user@example.com",
            "--json",
            "logs",
            "ocpi",
            "--limit",
            "2",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["resource"], "ocpi_logs");
    assert_eq!(value["meta"]["count"], 1);
    assert_eq!(value["data"][0]["id"], "log-1");
}

#[test]
fn rejects_password_passed_on_argv() {
    let assert = mobie_command()
        .args([
            "--base-url",
            "https://pgm.mobie.pt",
            "--email",
            "user@example.com",
            "--password",
            "password",
            "--json",
            "auth",
            "check",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("unexpected argument '--password'"),
        "stderr was: {stderr}"
    );
}
