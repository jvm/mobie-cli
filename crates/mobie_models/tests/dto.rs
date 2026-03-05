use mobie_models::{ApiEnvelope, LocationSummary, LoginResponse, OcppLogEntry, Session};

fn read_fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read(path).unwrap()
}

#[test]
fn parses_login_response_fixture() {
    let bytes = read_fixture("login-response.json");
    let env: ApiEnvelope<LoginResponse> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env.data.user.email, "user@example.com");
    assert_eq!(env.data.bearer.access_token, "test-access-token");
}

#[test]
fn parses_sessions_fixture() {
    let bytes = read_fixture("sessions-page.json");
    let env: ApiEnvelope<Vec<Session>> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env.data.len(), 1);
    let s = &env.data[0];
    assert_eq!(s.id, "sess-1");
    assert_eq!(
        s.cdr_token.as_ref().and_then(|t| t.uid.as_deref()),
        Some("token-1")
    );
}

#[test]
fn parses_locations_fixture() {
    let bytes = read_fixture("locations.json");
    let env: ApiEnvelope<Vec<LocationSummary>> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env.data.len(), 2);
    assert_eq!(env.data[0].location_id, "MOBI-XXX-00000");
}

#[test]
fn parses_ocpp_logs_fixture() {
    let bytes = read_fixture("ocpp-logs.json");
    let env: ApiEnvelope<Vec<OcppLogEntry>> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env.data.len(), 1);
    assert_eq!(env.data[0].message_type.as_deref(), Some("MeterValues"));
}
