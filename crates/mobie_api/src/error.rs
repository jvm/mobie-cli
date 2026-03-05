use serde_json::Value;
use thiserror::Error;

const MAX_ERROR_BODY_LEN: usize = 500;

#[derive(Debug, Error)]
pub enum MobieApiError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("invalid base url: {0}")]
    InvalidBaseUrl(String),

    #[error("unexpected response: {0}")]
    Unexpected(String),

    #[error("login failed: {status} {url} {body}")]
    LoginFailed {
        status: u16,
        url: String,
        body: String,
    },

    #[error("request failed: {status} {url} {body}")]
    RequestFailed {
        status: u16,
        url: String,
        body: String,
    },

    #[error("unauthorized: {status} {url} {body}")]
    Unauthorized {
        status: u16,
        url: String,
        body: String,
    },

    #[error("rate limited: {status} {url} {body}")]
    RateLimited {
        status: u16,
        url: String,
        body: String,
        retry_after_secs: Option<u64>,
    },

    #[error("server error: {status} {url} {body}")]
    ServerError {
        status: u16,
        url: String,
        body: String,
    },
}

impl MobieApiError {
    pub fn is_transient(&self) -> bool {
        match self {
            MobieApiError::Http(err) => err.is_timeout() || err.is_connect(),
            MobieApiError::RateLimited { .. } => true,
            MobieApiError::ServerError { .. } => true,
            _ => false,
        }
    }
}

pub fn sanitize_error_body(body: &str) -> String {
    let trimmed = body.trim();
    let mut out = if let Ok(mut value) = serde_json::from_str::<Value>(trimmed) {
        redact_sensitive(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| trimmed.to_string())
    } else if trimmed.contains("\\\"") {
        let unescaped = trimmed.replace("\\\"", "\"").replace("\\\\", "\\");
        if let Ok(mut value) = serde_json::from_str::<Value>(&unescaped) {
            redact_sensitive(&mut value);
            serde_json::to_string(&value).unwrap_or(unescaped)
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };
    if out.len() > MAX_ERROR_BODY_LEN {
        out.truncate(MAX_ERROR_BODY_LEN);
        out.push_str("...");
    }
    out
}

fn redact_sensitive(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let key = k.to_ascii_lowercase();
                if key.contains("token")
                    || key.contains("password")
                    || key.contains("authorization")
                    || key.contains("jwt")
                {
                    *v = Value::String("[REDACTED]".into());
                } else {
                    redact_sensitive(v);
                }
            }
        }
        Value::Array(items) => {
            for v in items {
                redact_sensitive(v);
            }
        }
        Value::String(s) => {
            if looks_like_jwt(s) {
                *s = "[REDACTED]".into();
            }
        }
        _ => {}
    }
}

fn looks_like_jwt(s: &str) -> bool {
    let mut parts = s.split('.');
    let (Some(a), Some(b), Some(c), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    is_base64ish(a) && is_base64ish(b) && is_base64ish(c)
}

fn is_base64ish(s: &str) -> bool {
    !s.is_empty()
        && s.len() >= 8
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '+')
}
