use std::time::Duration;

use mobie_api::{AccessContext, MobieApiError, MobieClient};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn times_out_slow_requests() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(200))
                .set_body_json(serde_json::json!({
                    "data": [],
                    "status_code": 1000,
                    "status_message": "Success",
                    "timestamp": "2025-01-01T00:00:00Z"
                })),
        )
        .mount(&server)
        .await;

    let mut client = MobieClient::new_with_timeouts(
        server.uri(),
        Duration::from_millis(50),
        Duration::from_millis(50),
    )
    .unwrap()
    .with_access(AccessContext {
        user_email: "user@example.com".into(),
        profile: "DPC".into(),
        access_token: "test-token".into(),
        refresh_token: None,
        expires_at_epoch_ms: None,
    });

    let err = client.list_locations().await.unwrap_err();
    match err {
        MobieApiError::Http(e) => assert!(e.is_timeout()),
        other => panic!("unexpected error: {other:?}"),
    }
}
