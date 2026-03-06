use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mobie_models::{
    ApiEnvelope, LocationDetail, LocationSummary, LoginResponse, OcppLogEntry, Session, TokenInfo,
};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::{debug, error, info, instrument};

use crate::{MobieApiError, sanitize_error_body};

#[derive(Debug, Clone, Default)]
pub struct SessionFilters {
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccessContext {
    pub user_email: String,
    pub profile: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_epoch_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MobieClient {
    base_url: String,
    http: reqwest::Client,
    access: Option<AccessContext>,
}

impl MobieClient {
    pub fn new_with_timeouts(
        base_url: impl Into<String>,
        timeout: Duration,
        connect_timeout: Duration,
    ) -> Result<Self, MobieApiError> {
        let base_url = validate_base_url(base_url.into())?;
        let http = reqwest::Client::builder()
            .user_agent("mobie-cli/0.1")
            .timeout(timeout)
            .connect_timeout(connect_timeout)
            .build()
            .map_err(MobieApiError::Http)?;
        Ok(Self {
            base_url,
            http,
            access: None,
        })
    }

    pub fn new(base_url: impl Into<String>) -> Result<Self, MobieApiError> {
        Self::new_with_timeouts(base_url, Duration::from_secs(30), Duration::from_secs(10))
    }

    pub fn with_access(mut self, access: AccessContext) -> Self {
        self.access = Some(access);
        self
    }

    pub fn access_context(&self) -> Option<&AccessContext> {
        self.access.as_ref()
    }

    async fn authed_headers(&mut self) -> Result<HeaderMap, MobieApiError> {
        self.ensure_valid_token().await?;
        let access = self.access.as_ref().ok_or_else(|| {
            MobieApiError::Unexpected("missing access context; call login first".into())
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", access.access_token))
                .map_err(|e| MobieApiError::Unexpected(format!("bad token header: {e}")))?,
        );
        headers.insert(
            "user",
            HeaderValue::from_str(&access.user_email)
                .map_err(|e| MobieApiError::Unexpected(format!("bad user header: {e}")))?,
        );
        headers.insert(
            "profile",
            HeaderValue::from_str(&access.profile)
                .map_err(|e| MobieApiError::Unexpected(format!("bad profile header: {e}")))?,
        );
        Ok(headers)
    }

    async fn ensure_valid_token(&mut self) -> Result<(), MobieApiError> {
        let refresh = match &self.access {
            Some(access) => access
                .expires_at_epoch_ms
                .map(|exp| current_epoch_millis() >= exp)
                .unwrap_or(false),
            None => false,
        };
        if refresh {
            self.refresh_token().await?;
        }
        Ok(())
    }

    async fn refresh_token(&mut self) -> Result<(), MobieApiError> {
        #[derive(Serialize)]
        struct RefreshBody<'a> {
            refresh_token: &'a str,
        }

        let access = self
            .access
            .as_ref()
            .ok_or_else(|| MobieApiError::Unexpected("missing access context".into()))?;
        let refresh_token = access
            .refresh_token
            .as_deref()
            .ok_or_else(|| MobieApiError::Unexpected("missing refresh token".into()))?;

        let url = format!("{}/api/refresh", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .post(&url)
            .json(&RefreshBody { refresh_token })
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let body = sanitize_error_body(&res.text().await.unwrap_or_default());
            return Err(classify_error(status, url, body));
        }

        let env: ApiEnvelope<LoginResponse> = res.json().await?;
        self.access = Some(access_context_from_login(env.data));
        Ok(())
    }

    #[instrument(skip(self, password), fields(email = %email))]
    pub async fn login(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<AccessContext, MobieApiError> {
        #[derive(Serialize)]
        struct LoginBody<'a> {
            email: &'a str,
            password: &'a str,
        }

        debug!("api_login_attempt");
        let start = Instant::now();
        let url = format!("{}/api/login", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .post(&url)
            .json(&LoginBody { email, password })
            .send()
            .await?;

        let duration_ms = start.elapsed().as_millis();

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let body = sanitize_error_body(&res.text().await.unwrap_or_default());
            error!(status, duration_ms, "api_login_failed");
            return Err(MobieApiError::LoginFailed { status, url, body });
        }

        let env: ApiEnvelope<LoginResponse> = res.json().await?;
        let access = access_context_from_login(env.data);

        self.access = Some(access.clone());
        info!(duration_ms, profile = %access.profile, "api_login_success");
        Ok(access)
    }

    async fn get_json<T: DeserializeOwned>(
        &mut self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<T, MobieApiError> {
        let start = Instant::now();
        let mut delay = Duration::from_millis(200);
        for attempt in 0..3 {
            let res = self.http.get(url).headers(headers.clone()).send().await;
            match res {
                Ok(resp) => {
                    let duration_ms = start.elapsed().as_millis();
                    if resp.status().is_success() {
                        debug!(url, duration_ms, attempt, "api_request_success");
                        return resp.json::<T>().await.map_err(MobieApiError::Http);
                    }
                    let status = resp.status().as_u16();
                    let body = sanitize_error_body(&resp.text().await.unwrap_or_default());
                    let err = classify_error(status, url.to_string(), body);
                    if attempt < 2 && err.is_transient() {
                        debug!(url, status, attempt, "api_request_retry");
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }
                    error!(url, status, duration_ms, "api_request_failed");
                    return Err(err);
                }
                Err(err) => {
                    let duration_ms = start.elapsed().as_millis();
                    let err = MobieApiError::Http(err);
                    if attempt < 2 && err.is_transient() {
                        debug!(url, attempt, "api_request_retry_transport");
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }
                    error!(url, duration_ms, error = %err, "api_request_transport_failed");
                    return Err(err);
                }
            }
        }
        error!(url, "api_request_exhausted_retries");
        Err(MobieApiError::Unexpected(
            "exhausted retries without response".into(),
        ))
    }

    pub async fn list_locations(&mut self) -> Result<Vec<LocationSummary>, MobieApiError> {
        let url = format!(
            "{}/api/locations?limit=0&offset=0",
            self.base_url.trim_end_matches('/')
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<LocationSummary>> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn get_location(
        &mut self,
        location_id: &str,
    ) -> Result<LocationDetail, MobieApiError> {
        let url = format!(
            "{}/api/locations/{}",
            self.base_url.trim_end_matches('/'),
            location_id
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<LocationDetail> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn get_location_analytics(&mut self) -> Result<Value, MobieApiError> {
        let url = format!(
            "{}/api/locations/analytics",
            self.base_url.trim_end_matches('/')
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Value> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn get_location_geojson(&mut self) -> Result<Value, MobieApiError> {
        let url = format!(
            "{}/api/locations/geojson",
            self.base_url.trim_end_matches('/')
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Value> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    #[instrument(skip(self), fields(location_id = %location_id, limit, offset))]
    pub async fn list_sessions_page(
        &mut self,
        location_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Session>, MobieApiError> {
        let filters = SessionFilters::default();
        self.list_sessions_page_filtered(location_id, limit, offset, &filters)
            .await
    }

    #[instrument(skip(self, filters), fields(location_id = %location_id, limit, offset))]
    pub async fn list_sessions_page_filtered(
        &mut self,
        location_id: &str,
        limit: i64,
        offset: i64,
        filters: &SessionFilters,
    ) -> Result<Vec<Session>, MobieApiError> {
        let base = format!("{}/api/sessions", self.base_url.trim_end_matches('/'));
        let mut url = reqwest::Url::parse(&base)
            .map_err(|e| MobieApiError::Unexpected(format!("bad base url: {e}")))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("limit", &limit.to_string())
                .append_pair("offset", &offset.to_string())
                .append_pair("locationId", location_id);
            if let Some(date_from) = filters.date_from.as_deref() {
                query.append_pair("dateFrom", date_from);
            }
            if let Some(date_to) = filters.date_to.as_deref() {
                query.append_pair("dateTo", date_to);
            }
        }
        let url = url.to_string();
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<Session>> = self.get_json(&url, headers).await?;
        debug!(
            result_count = env.data.len(),
            "api_list_sessions_page_complete"
        );
        Ok(env.data)
    }

    pub async fn list_sessions_paginated(
        &mut self,
        location_id: &str,
        limit: i64,
    ) -> Result<Vec<Session>, MobieApiError> {
        let filters = SessionFilters::default();
        self.list_sessions_paginated_filtered(location_id, limit, &filters)
            .await
    }

    pub async fn list_sessions_paginated_filtered(
        &mut self,
        location_id: &str,
        limit: i64,
        filters: &SessionFilters,
    ) -> Result<Vec<Session>, MobieApiError> {
        let mut limit = clamp_limit(limit);
        let mut out = Vec::new();
        let mut offset: i64 = 0;
        loop {
            let page = match self
                .list_sessions_page_filtered(location_id, limit, offset, filters)
                .await
            {
                Ok(page) => page,
                Err(err) => {
                    if limit > 50 {
                        limit = 50;
                        continue;
                    }
                    return Err(err);
                }
            };
            if page.is_empty() {
                break;
            }
            offset += page.len() as i64;
            out.extend(page);
        }
        Ok(out)
    }

    pub async fn list_ocpp_logs_page(
        &mut self,
        limit: i64,
        offset: i64,
        error_only: bool,
    ) -> Result<Vec<OcppLogEntry>, MobieApiError> {
        let mut url = format!(
            "{}/api/logs/ocpp?limit={}&offset={}",
            self.base_url.trim_end_matches('/'),
            limit,
            offset
        );
        if error_only {
            url.push_str("&error=true");
        }
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<OcppLogEntry>> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn list_ocpi_logs_page(
        &mut self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Value>, MobieApiError> {
        let url = format!(
            "{}/api/logs/ocpi?limit={}&offset={}",
            self.base_url.trim_end_matches('/'),
            limit,
            offset
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<Value>> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn list_tokens_page(
        &mut self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TokenInfo>, MobieApiError> {
        let url = format!(
            "{}/api/tokens?limit={}&offset={}",
            self.base_url.trim_end_matches('/'),
            limit,
            offset
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<TokenInfo>> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn list_tokens_paginated(
        &mut self,
        limit: i64,
    ) -> Result<Vec<TokenInfo>, MobieApiError> {
        self.collect_paginated(limit, |client, page_limit, offset| {
            Box::pin(client.list_tokens_page(page_limit, offset))
        })
        .await
    }

    pub async fn list_ocpp_logs_paginated(
        &mut self,
        limit: i64,
        error_only: bool,
    ) -> Result<Vec<OcppLogEntry>, MobieApiError> {
        self.collect_paginated(limit, |client, page_limit, offset| {
            Box::pin(client.list_ocpp_logs_page(page_limit, offset, error_only))
        })
        .await
    }

    pub async fn list_ocpi_logs_paginated(
        &mut self,
        limit: i64,
    ) -> Result<Vec<Value>, MobieApiError> {
        self.collect_paginated(limit, |client, page_limit, offset| {
            Box::pin(client.list_ocpi_logs_page(page_limit, offset))
        })
        .await
    }

    pub async fn get_entity(&mut self, entity_code: &str) -> Result<Value, MobieApiError> {
        let url = format!(
            "{}/api/entities/{}",
            self.base_url.trim_end_matches('/'),
            entity_code
        );
        let mut headers = HeaderMap::new();
        self.ensure_valid_token().await?;
        let access = self.access.as_ref().ok_or_else(|| {
            MobieApiError::Unexpected("missing access context; call login first".into())
        })?;
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", access.access_token))
                .map_err(|e| MobieApiError::Unexpected(format!("bad token header: {e}")))?,
        );
        headers.insert(
            "user",
            HeaderValue::from_str(&access.user_email)
                .map_err(|e| MobieApiError::Unexpected(format!("bad user header: {e}")))?,
        );
        let env: ApiEnvelope<Value> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn get_role(&mut self, role_name: &str) -> Result<Value, MobieApiError> {
        let url = format!(
            "{}/api/identity/roles/{}",
            self.base_url.trim_end_matches('/'),
            role_name
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Value> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn list_ords(&mut self) -> Result<Vec<Value>, MobieApiError> {
        let url = format!("{}/api/ords", self.base_url.trim_end_matches('/'));
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<Value>> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn get_ord_statistics(&mut self) -> Result<Value, MobieApiError> {
        let url = format!(
            "{}/api/ords/statistics",
            self.base_url.trim_end_matches('/')
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Value> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn list_ords_cpes_integrated(&mut self) -> Result<Vec<Value>, MobieApiError> {
        let url = format!(
            "{}/api/ords/cpesIntegrated",
            self.base_url.trim_end_matches('/')
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<Value>> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    pub async fn list_ords_cpes_to_integrate(&mut self) -> Result<Vec<Value>, MobieApiError> {
        let url = format!(
            "{}/api/ords/cpesToIntegrate",
            self.base_url.trim_end_matches('/')
        );
        let headers = self.authed_headers().await?;
        let env: ApiEnvelope<Vec<Value>> = self.get_json(&url, headers).await?;
        Ok(env.data)
    }

    async fn collect_paginated<T, F>(
        &mut self,
        limit: i64,
        mut fetch_page: F,
    ) -> Result<Vec<T>, MobieApiError>
    where
        F: for<'a> FnMut(
            &'a mut MobieClient,
            i64,
            i64,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<T>, MobieApiError>> + 'a>,
        >,
    {
        let mut page_limit = clamp_limit(limit);
        let mut offset = 0_i64;
        let mut all_items = Vec::new();

        loop {
            let page = match fetch_page(self, page_limit, offset).await {
                Ok(page) => page,
                Err(err) if page_limit > 50 => {
                    page_limit = 50;
                    continue;
                }
                Err(err) => return Err(err),
            };

            if page.is_empty() {
                break;
            }

            offset += page.len() as i64;
            all_items.extend(page);
        }

        Ok(all_items)
    }
}

fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, 1000)
}

fn validate_base_url(base_url: String) -> Result<String, MobieApiError> {
    let parsed = Url::parse(&base_url)
        .map_err(|err| MobieApiError::InvalidBaseUrl(format!("{base_url} ({err})")))?;

    if parsed.scheme() == "https" || is_loopback_http_url(&parsed) {
        return Ok(parsed.to_string());
    }

    Err(MobieApiError::InvalidBaseUrl(format!(
        "{base_url} (expected https://; http:// is only allowed for loopback test hosts)"
    )))
}

fn is_loopback_http_url(url: &Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }

    let Some(host) = url.host_str() else {
        return false;
    };

    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false)
}

fn access_context_from_login(login: LoginResponse) -> AccessContext {
    let LoginResponse { bearer, user } = login;
    let profile = user
        .roles
        .as_ref()
        .and_then(|roles| roles.iter().find_map(|role| role.profile.clone()))
        .unwrap_or_else(|| "DPC".to_string());
    let expires_at = bearer.expires_in.map(|seconds| {
        current_epoch_millis().saturating_add((seconds.max(0) as u64).saturating_mul(1000))
    });

    AccessContext {
        user_email: user.email,
        profile,
        access_token: bearer.access_token,
        refresh_token: bearer.refresh_token,
        expires_at_epoch_ms: expires_at,
    }
}

fn current_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

fn classify_error(status: u16, url: String, body: String) -> MobieApiError {
    match status {
        401 | 403 => MobieApiError::Unauthorized { status, url, body },
        429 => MobieApiError::RateLimited {
            status,
            url,
            body,
            retry_after_secs: None,
        },
        500..=599 => MobieApiError::ServerError { status, url, body },
        _ => MobieApiError::RequestFailed { status, url, body },
    }
}
