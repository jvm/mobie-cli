use mobie_api::MobieClient;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn sends_authorization_user_profile_headers() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("user", "user@example.com"))
        .and(header("profile", "DPC"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let mut client =
        MobieClient::new(server.uri())
            .unwrap()
            .with_access(mobie_api::AccessContext {
                user_email: "user@example.com".into(),
                profile: "DPC".into(),
                access_token: "test-token".into(),
                refresh_token: None,
                expires_at_epoch_ms: None,
            });

    let _ = client.list_locations().await.unwrap();
}
