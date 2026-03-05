use mobie_api::{AccessContext, MobieApiError, MobieClient, sanitize_error_body};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn sanitize_error_body_redacts_and_truncates() {
    let body = r#"{\"access_token\":\"abc\",\"password\":\"secret\",\"nested\":{\"refresh_token\":\"xyz\"}}"#;
    let out = sanitize_error_body(body);
    assert!(!out.contains("abc"));
    assert!(!out.contains("secret"));
    assert!(out.contains("[REDACTED]"));

    let long_body = "a".repeat(800);
    let truncated = sanitize_error_body(&long_body);
    assert!(truncated.len() <= 503);
    assert!(truncated.ends_with("..."));
}

#[test]
fn rejects_non_https_non_loopback_base_urls() {
    let err = MobieClient::new("http://example.com").unwrap_err();

    match err {
        MobieApiError::InvalidBaseUrl(message) => {
            assert!(message.contains("expected https://"));
            assert!(message.contains("http://example.com"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn error_includes_url_and_sanitized_body() {
    let server = MockServer::start().await;

    let body = r#"{\"access_token\":\"leaky\"}"#;
    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(500).set_body_raw(body, "application/json"))
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

    let err = client.list_locations().await.unwrap_err();
    match err {
        MobieApiError::ServerError { url, body, .. } => {
            assert!(url.contains("/api/locations"));
            assert!(!body.contains("leaky"));
            assert!(body.contains("[REDACTED]"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn maps_unauthorized_status() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
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

    let err = client.list_locations().await.unwrap_err();
    match err {
        MobieApiError::Unauthorized { .. } => {}
        other => panic!("unexpected error: {other:?}"),
    }
}
