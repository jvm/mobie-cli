use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
    pub status_code: Option<i64>,
    pub status_message: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub bearer: Bearer,
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bearer {
    pub access_token: String,
    pub expires_in: Option<i64>,
    pub refresh_expires_in: Option<i64>,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub id_token: Option<String>,
    #[serde(rename = "not-before-policy")]
    pub not_before_policy: Option<i64>,
    pub session_state: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub email: String,
    pub entity: Option<String>,
    pub roles: Option<Vec<UserRole>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRole {
    pub profile: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationSummary {
    #[serde(alias = "id")]
    pub location_id: String,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationDetail {
    #[serde(alias = "id")]
    pub location_id: String,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdrToken {
    pub uid: Option<String>,
    #[serde(rename = "type")]
    pub token_type: Option<String>,
    pub contract_id: Option<String>,
    #[serde(rename = "pdgrPartyId")]
    pub pdgr_party_id: Option<String>,
    #[serde(rename = "pdgrVisualNumber")]
    pub pdgr_visual_number: Option<String>,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    #[serde(alias = "uid", alias = "token_uid")]
    pub token_uid: Option<String>,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub start_date_time: String,
    pub end_date_time: Option<String>,
    pub kwh: Option<f64>,
    pub status: Option<String>,

    pub location_id: Option<String>,
    pub evse_uid: Option<String>,
    pub connector_id: Option<String>,

    #[serde(rename = "pdgrTransactionId")]
    pub pdgr_transaction_id: Option<serde_json::Value>,

    pub cdr_token: Option<CdrToken>,
    pub charging_periods: Option<Vec<ChargingPeriod>>,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargingPeriod {
    pub start_date_time: String,
    pub dimensions: Option<Vec<Dimension>>,
    pub tariff_id: Option<String>,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    #[serde(rename = "type")]
    pub dim_type: String,
    pub volume: DimensionVolume,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DimensionVolume {
    Number(f64),
    String(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OcppLogs {
    String(String),
    Array(serde_json::Value),
}

impl OcppLogs {
    pub fn as_str(&self) -> String {
        match self {
            OcppLogs::String(s) => s.clone(),
            OcppLogs::Array(v) => v.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcppLogEntry {
    pub id: Option<String>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    pub direction: Option<String>,
    pub timestamp: Option<String>,
    pub logs: Option<OcppLogs>,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}
