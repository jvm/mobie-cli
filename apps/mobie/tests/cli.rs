use std::process::Command;

use tempfile::tempdir;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mount_login(server: &MockServer) {
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
        .mount(server)
        .await;
}

fn cli(server: &MockServer) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("mobie"));
    let cache_dir = tempdir().unwrap().keep();
    cmd.env("MOBIE_BASE_URL", server.uri());
    cmd.env("MOBIE_EMAIL", "user@example.com");
    cmd.env("MOBIE_PASSWORD", "secret");
    cmd.env("MOBIE_CACHE_DIR", cache_dir);
    cmd
}

#[tokio::test]
async fn auth_check_json_reports_identity() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    let output = cli(&server)
        .arg("--json")
        .args(["auth", "check"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["resource"], "auth");
    assert_eq!(body["data"]["email"], "user@example.com");
    assert_eq!(body["data"]["profile"], "DPC");
}

#[tokio::test]
async fn auth_login_uses_env_credentials_without_stdin() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    let output = cli(&server)
        .arg("--json")
        .args(["auth", "login"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["resource"], "auth");
    assert_eq!(body["data"]["source"], "login");
    assert_eq!(body["data"]["email"], "user@example.com");
}

#[tokio::test]
async fn locations_list_json_returns_counted_array() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "MOBI-AAA-00001"},
                {"id": "MOBI-BBB-00002"}
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&server)
        .arg("--json")
        .args(["locations", "list"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["resource"], "locations");
    assert_eq!(body["meta"]["count"], 2);
    assert_eq!(body["data"][0]["location_id"], "MOBI-AAA-00001");
}

#[tokio::test]
async fn locations_get_json_returns_single_object() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations/MOBI-AAA-00001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "id": "MOBI-AAA-00001",
                "status": "ACTIVE"
            },
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&server)
        .arg("--json")
        .args(["locations", "get", "--location", "MOBI-AAA-00001"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["resource"], "location");
    assert_eq!(body["data"]["location_id"], "MOBI-AAA-00001");
    assert_eq!(body["data"]["status"], "ACTIVE");
}

#[tokio::test]
async fn sessions_list_json_collects_all_pages() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {
                    "id": "sess-1",
                    "start_date_time": "2025-01-02T10:00:00Z",
                    "end_date_time": "2025-01-02T11:00:00Z",
                    "status": "COMPLETED",
                    "kwh": 9.0,
                    "cdr_token": {"uid": "token-2"}
                },
                {
                    "id": "sess-2",
                    "start_date_time": "2025-01-01T10:00:00Z",
                    "end_date_time": "2025-01-01T11:00:00Z",
                    "status": "COMPLETED",
                    "kwh": 10.5,
                    "cdr_token": {"uid": "token-1"}
                }
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "2"))
        .and(query_param("locationId", "EVSE-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&server)
        .arg("--json")
        .args(["sessions", "list", "--location", "EVSE-1", "--limit", "2"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["resource"], "sessions");
    assert_eq!(body["meta"]["count"], 2);
    // With oldest-first ordering, sess-2 (Jan 1) comes before sess-1 (Jan 2)
    assert_eq!(body["data"][0]["id"], "sess-2");
    assert_eq!(body["data"][1]["id"], "sess-1");
}

#[tokio::test]
async fn sessions_list_json_sends_api_date_filters() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "10"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
        .and(query_param("dateFrom", "2025-01-02T00:00:00.000Z"))
        .and(query_param("dateTo", "2025-01-02T23:59:59.999Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {
                    "id": "sess-inside",
                    "start_date_time": "2025-01-02T10:00:00Z",
                    "end_date_time": "2025-01-02T11:00:00Z",
                    "status": "COMPLETED",
                    "kwh": 7.0
                },
                {
                    "id": "sess-server-overlap",
                    "start_date_time": "2025-01-01T23:30:00Z",
                    "end_date_time": "2025-01-02T00:30:00Z",
                    "status": "COMPLETED",
                    "kwh": 5.0
                },
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "10"))
        .and(query_param("offset", "2"))
        .and(query_param("locationId", "EVSE-1"))
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

    let output = cli(&server)
        .arg("--json")
        .args([
            "sessions",
            "list",
            "--location",
            "EVSE-1",
            "--limit",
            "10",
            "--from",
            "2025-01-02",
            "--to",
            "2025-01-02",
        ])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["meta"]["count"], 2);
    let ids = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["sess-server-overlap", "sess-inside"]);
}

#[tokio::test]
async fn tokens_and_logs_queries_work() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/tokens"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"uid": "token-1"}],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/tokens"))
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

    let tokens = cli(&server)
        .arg("--json")
        .args(["tokens", "list", "--limit", "2"])
        .output()
        .expect("run mobie");

    assert!(tokens.status.success());
    let token_body: serde_json::Value = serde_json::from_slice(&tokens.stdout).unwrap();
    assert_eq!(token_body["resource"], "tokens");
    assert_eq!(token_body["meta"]["count"], 1);
    assert_eq!(token_body["data"][0]["token_uid"], "token-1");

    Mock::given(method("GET"))
        .and(path("/api/logs/ocpp"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("startDate", "2025-01-01T00:00:00.000Z"))
        .and(query_param("endDate", "2025-01-02T23:59:59.999Z"))
        .and(query_param("id", "MOBI-AAA-00001"))
        .and(query_param("messageType", "Heartbeat"))
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
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/logs/ocpp"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "1"))
        .and(query_param("startDate", "2025-01-01T00:00:00.000Z"))
        .and(query_param("endDate", "2025-01-02T23:59:59.999Z"))
        .and(query_param("id", "MOBI-AAA-00001"))
        .and(query_param("messageType", "Heartbeat"))
        .and(query_param("error", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let logs = cli(&server)
        .arg("--json")
        .args([
            "logs",
            "list",
            "--limit",
            "2",
            "--error-only",
            "--location",
            "MOBI-AAA-00001",
            "--message-type",
            "Heartbeat",
            "--from",
            "2025-01-01",
            "--to",
            "2025-01-02",
        ])
        .output()
        .expect("run mobie");

    assert!(logs.status.success());
    let log_body: serde_json::Value = serde_json::from_slice(&logs.stdout).unwrap();
    assert_eq!(log_body["resource"], "logs");
    assert_eq!(log_body["meta"]["count"], 1);
    assert_eq!(log_body["data"][0]["messageType"], "Heartbeat");
}

#[tokio::test]
async fn logs_list_rejects_ranges_longer_than_seven_days() {
    let server = MockServer::start().await;

    let output = cli(&server)
        .arg("--json")
        .args([
            "logs",
            "list",
            "--location",
            "MOBI-AAA-00001",
            "--from",
            "2025-01-01",
            "--to",
            "2025-01-10",
        ])
        .output()
        .expect("run mobie");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("7 days or less"), "stderr was: {stderr}");
}

#[tokio::test]
async fn human_output_stays_readable() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "MOBI-AAA-00001"}],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&server)
        .args(["locations", "list"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Locations (1)"));
    assert!(stdout.contains("location_id"));
    assert!(stdout.contains("MOBI-AAA-00001"));
    assert!(!stdout.trim_start().starts_with('{'));
    assert!(!stdout.contains("| location_id |"));
}

#[tokio::test]
async fn ords_list_human_output_is_markdown() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/ords"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {
                    "cpe": "PT0001",
                    "cpeStatus": "Integrated",
                    "location_id": "LOC-1",
                    "integrationDate": "2025-01-01T00:00:00Z",
                    "entityCode": "MOBI"
                }
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&server)
        .args(["ords", "list"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ORDs (1)"));
    assert!(stdout.contains("cpe"));
    assert!(stdout.contains("PT0001"));
    assert!(!stdout.trim_start().starts_with('['));
    assert!(!stdout.contains("| cpe |"));
}

#[tokio::test]
async fn markdown_option_emits_raw_markdown() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "MOBI-AAA-00001"}],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&server)
        .args(["--markdown", "locations", "list"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("# Locations"));
    assert!(stdout.contains("| location_id |"));
    assert!(stdout.contains("| MOBI-AAA-00001 |"));
}

#[tokio::test]
async fn toon_option_emits_toon() {
    let server = MockServer::start().await;
    mount_login(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/ords"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {
                    "cpe": "PT0001",
                    "cpeStatus": "Integrated",
                    "location_id": "LOC-1",
                    "integrationDate": "2025-01-01T00:00:00Z",
                    "entityCode": "MOBI"
                }
            ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let output = cli(&server)
        .args(["--toon", "ords", "list"])
        .output()
        .expect("run mobie");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ok: true"));
    assert!(stdout.contains("resource: ords"));
    assert!(stdout.contains("data[1]{"));
    assert!(stdout.contains("PT0001"));
    assert!(stdout.contains("Integrated"));
    assert!(stdout.contains("LOC-1"));
    assert!(stdout.contains("MOBI"));
    assert!(!stdout.trim_start().starts_with('{'));
}

#[test]
fn help_mentions_toon_for_agents() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("mobie"))
        .arg("--help")
        .output()
        .expect("run mobie --help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--toon"));
    assert!(stdout.contains("preferred structured format for agents"));
}
