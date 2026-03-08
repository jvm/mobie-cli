use keyring::Entry;
use mobie_api::AccessContext;
use mobie_models::LoginResponse;
use serde::{Deserialize, Serialize};

const SERVICE_NAME: &str = "dev.mocito.mobie-cli";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub base_url: String,
    pub access: AccessContext,
    #[serde(default)]
    pub login: Option<LoginResponse>,
}

pub trait SessionStore {
    fn load(&self, base_url: &str) -> Result<Option<StoredSession>, String>;
    fn save(&self, session: &StoredSession) -> Result<(), String>;
    fn delete(&self, base_url: &str) -> Result<bool, String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringSessionStore;

impl SessionStore for KeyringSessionStore {
    fn load(&self, base_url: &str) -> Result<Option<StoredSession>, String> {
        for key in lookup_keys(base_url) {
            let entry = entry(&key)?;
            match entry.get_password() {
                Ok(secret) => {
                    return serde_json::from_str(&secret)
                        .map(Some)
                        .map_err(|err| format!("invalid stored session payload: {err}"));
                }
                Err(keyring::Error::NoEntry) => continue,
                Err(err) => return Err(format!("failed to read stored session: {err}")),
            }
        }
        Ok(None)
    }

    fn save(&self, session: &StoredSession) -> Result<(), String> {
        let canonical_base_url = canonical_key(&session.base_url);
        let keyring_entry = entry(&canonical_base_url)?;
        let payload = serde_json::to_string(session)
            .map_err(|err| format!("failed to serialize stored session: {err}"))?;
        keyring_entry
            .set_password(&payload)
            .map_err(|err| format!("failed to save stored session: {err}"))?;

        for key in lookup_keys(&session.base_url) {
            if key != canonical_base_url {
                let legacy_entry = entry(&key)?;
                match legacy_entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(err) => {
                        return Err(format!("failed to clean up legacy stored session: {err}"));
                    }
                }
            }
        }

        Ok(())
    }

    fn delete(&self, base_url: &str) -> Result<bool, String> {
        let mut removed = false;
        for key in lookup_keys(base_url) {
            let entry = entry(&key)?;
            match entry.delete_credential() {
                Ok(()) => removed = true,
                Err(keyring::Error::NoEntry) => {}
                Err(err) => return Err(format!("failed to delete stored session: {err}")),
            }
        }
        Ok(removed)
    }
}

fn entry(base_url: &str) -> Result<Entry, String> {
    Entry::new(SERVICE_NAME, base_url).map_err(|err| format!("failed to open keyring entry: {err}"))
}

fn lookup_keys(base_url: &str) -> Vec<String> {
    let canonical = canonical_key(base_url);
    let mut keys = vec![base_url.to_string()];

    if canonical != base_url {
        keys.push(canonical.clone());
    }

    let with_trailing_slash = if canonical.is_empty() {
        String::new()
    } else {
        format!("{canonical}/")
    };
    if !with_trailing_slash.is_empty() && !keys.iter().any(|key| key == &with_trailing_slash) {
        keys.push(with_trailing_slash);
    }

    keys
}

fn canonical_key(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let canonical = trimmed.trim_end_matches('/');
    if canonical.is_empty() {
        trimmed.to_string()
    } else {
        canonical.to_string()
    }
}
