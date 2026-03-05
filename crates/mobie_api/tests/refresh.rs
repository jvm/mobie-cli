use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mobie_api::{AccessContext, MobieClient};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn refreshes_token_before_request_when_expired() {
    let server = MockServer::start().await;

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
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
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

    let mut client = MobieClient::new(server.uri())
        .unwrap()
        .with_access(AccessContext {
            user_email: "user@example.com".into(),
            profile: "DPC".into(),
            access_token: "old-token".into(),
            refresh_token: Some("refresh-1".into()),
            expires_at_epoch_ms: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_else(|_| Duration::from_secs(0))
                    .as_millis() as u64
                    - 5_000,
            ),
        });

    let _ = client.list_locations().await.unwrap();
}
