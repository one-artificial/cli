use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE_NAME: &str = "one-cli";

/// In-memory cache of keychain credentials to avoid repeated system keychain prompts.
/// Once a credential is read from the keychain, it's cached for the session.
static CREDENTIAL_CACHE: Lazy<Mutex<HashMap<String, Option<String>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub struct CredentialStore;

impl CredentialStore {
    pub fn store(provider: &str, api_key: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE_NAME, provider)?;
        entry.set_password(api_key)?;
        // Invalidate cache on write so next read gets fresh value
        if let Ok(mut cache) = CREDENTIAL_CACHE.lock() {
            cache.remove(provider);
        }
        Ok(())
    }

    pub fn get(provider: &str) -> Result<Option<String>> {
        // Check cache first
        if let Ok(cache) = CREDENTIAL_CACHE.lock()
            && let Some(cached) = cache.get(provider)
        {
            return Ok(cached.clone());
        }

        // Cache miss: read from keychain
        let entry = keyring::Entry::new(SERVICE_NAME, provider)?;
        let result = match entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        };

        // Cache the result (including None)
        if let Ok(result_ref) = &result
            && let Ok(mut cache) = CREDENTIAL_CACHE.lock()
        {
            cache.insert(provider.to_string(), result_ref.clone());
        }

        result
    }

    pub fn delete(provider: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE_NAME, provider)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve API key for a provider.
    ///
    /// Resolution order:
    /// 1. Environment variable — cheapest, no syscall, checked first to avoid
    ///    unnecessary keychain round-trips when the user already has keys in env.
    /// 2. Config file — keys pasted during onboarding are stored here, not keychain.
    /// 3. OAuth tokens (keychain) — HF identity stored by /login.
    /// 4. API key (keychain) — legacy/manual keychain entries only.
    pub fn resolve(provider: &str, config_key: Option<&str>, env_var: &str) -> String {
        // 1. Environment variable — if present, use it directly; skip all keychain I/O.
        if let Ok(key) = std::env::var(env_var)
            && !key.is_empty()
        {
            return key;
        }

        // 2. Config file
        if let Some(key) = config_key
            && !key.is_empty()
        {
            return key.to_string();
        }

        // 3. OAuth tokens (keychain — only reached when env/config both absent)
        if let Ok(Some(oauth_json)) = Self::get(&format!("{provider}_oauth"))
            && let Ok(tokens) = serde_json::from_str::<crate::oauth::OAuthTokens>(&oauth_json)
        {
            if !tokens.is_expired() {
                return tokens.access_token;
            }

            if tokens.refresh_token.is_some() {
                let provider_owned = provider.to_string();
                tokio::spawn(async move {
                    match crate::oauth::try_refresh(&provider_owned).await {
                        Ok(new_token) => {
                            tracing::info!("OAuth token refreshed for {provider_owned}");
                            let _ = Self::store(&provider_owned, &new_token);
                        }
                        Err(e) => {
                            tracing::warn!("OAuth refresh failed for {provider_owned}: {e}");
                        }
                    }
                });
            }

            return tokens.access_token;
        }

        // 4. Legacy keychain API key entry
        if let Ok(Some(key)) = Self::get(provider)
            && !key.is_empty()
        {
            return key;
        }

        String::new()
    }
}
