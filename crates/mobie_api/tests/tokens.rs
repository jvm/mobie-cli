use mobie_api::{AccessContext, MobieClient};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn paginates_tokens_until_empty_page() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/tokens"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [ {"uid": "token-1"}, {"uid": "token-2"} ],
            "status_code": 1000,
            "status_message": "Success",
            "timestamp": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/tokens"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "2"))
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
            access_token: "test-token".into(),
            refresh_token: None,
            expires_at_epoch_ms: None,
        });

    let tokens = client.list_tokens_paginated(2).await.unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0].token_uid.as_deref(), Some("token-1"));
}
