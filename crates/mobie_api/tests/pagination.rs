use mobie_api::{AccessContext, MobieClient, SessionFilters};
use wiremock::matchers::{method, path, query_param, AnyMatcher};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn paginates_sessions_until_empty_page() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("locationId", "EVSE-1"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {
                    "id": "sess-3",
                    "start_date_time": "2025-01-03T00:00:00Z",
                    "end_date_time": "2025-01-03T01:00:00Z",
                    "location_id": "EVSE-1"
                },
                {
                    "id": "sess-2",
                    "start_date_time": "2025-01-02T00:00:00Z",
                    "end_date_time": "2025-01-02T01:00:00Z",
                    "location_id": "EVSE-1"
                },
                {
                    "id": "sess-1",
                    "start_date_time": "2025-01-01T00:00:00Z",
                    "end_date_time": "2025-01-01T01:00:00Z",
                    "location_id": "EVSE-1"
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
        .and(query_param("locationId", "EVSE-1"))
        .and(query_param("offset", "3"))
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

    let sessions = client.list_sessions_paginated("EVSE-1", 3).await.unwrap();
    assert_eq!(sessions.len(), 3);
    assert_eq!(sessions[0].id, "sess-1");
    assert_eq!(sessions[2].id, "sess-3");
}

#[tokio::test]
async fn clamps_pagination_limit() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "1"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
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

    let sessions = client.list_sessions_paginated("EVSE-1", 0).await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn sends_session_date_filters() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(query_param("locationId", "EVSE-1"))
        .and(query_param("dateFrom", "2025-01-02T00:00:00.000Z"))
        .and(query_param("dateTo", "2025-01-02T23:59:59.999Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {
                    "id": "sess-filtered",
                    "start_date_time": "2025-01-02T10:00:00Z",
                    "end_date_time": "2025-01-02T11:00:00Z",
                    "location_id": "EVSE-1"
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
        .and(query_param("offset", "1"))
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

    let mut client = MobieClient::new(server.uri())
        .unwrap()
        .with_access(AccessContext {
            user_email: "user@example.com".into(),
            profile: "DPC".into(),
            access_token: "test-token".into(),
            refresh_token: None,
            expires_at_epoch_ms: None,
        });

    let filters = SessionFilters {
        date_from: Some("2025-01-02T00:00:00.000Z".into()),
        date_to: Some("2025-01-02T23:59:59.999Z".into()),
    };

    let sessions = client
        .list_sessions_paginated_filtered("EVSE-1", 2, &filters)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "sess-filtered");
}
