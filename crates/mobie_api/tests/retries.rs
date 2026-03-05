use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mobie_api::{AccessContext, MobieApiError, MobieClient};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Clone)]
struct FlakyResponder {
    count: Arc<AtomicUsize>,
}

impl Respond for FlakyResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let idx = self.count.fetch_add(1, Ordering::SeqCst);
        if idx < 2 {
            ResponseTemplate::new(500).set_body_string("temporary")
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [],
                "status_code": 1000,
                "status_message": "Success",
                "timestamp": "2025-01-01T00:00:00Z"
            }))
        }
    }
}

#[derive(Clone)]
struct RateLimitResponder;

impl Respond for RateLimitResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(429).set_body_string("rate limit")
    }
}

fn authed_client(server: &MockServer) -> MobieClient {
    MobieClient::new(server.uri())
        .unwrap()
        .with_access(AccessContext {
            user_email: "user@example.com".into(),
            profile: "DPC".into(),
            access_token: "test-token".into(),
            refresh_token: None,
            expires_at_epoch_ms: None,
        })
}

#[tokio::test]
async fn retries_transient_server_errors() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(FlakyResponder {
            count: Arc::new(AtomicUsize::new(0)),
        })
        .mount(&server)
        .await;

    let mut client = authed_client(&server);
    let res = client.list_locations().await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn retries_rate_limited_responses() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/locations"))
        .and(query_param("limit", "0"))
        .and(query_param("offset", "0"))
        .respond_with(RateLimitResponder)
        .mount(&server)
        .await;

    let mut client = authed_client(&server);
    let err = client.list_locations().await.unwrap_err();
    match err {
        MobieApiError::RateLimited { .. } => {}
        other => panic!("unexpected error: {other:?}"),
    }
}
