use anyhow::Result;

const SERVICE_NAME: &str = "one-cli";

pub struct CredentialStore;

impl CredentialStore {
    pub fn store(provider: &str, api_key: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE_NAME, provider)?;
        entry.set_password(api_key)?;
        Ok(())
    }

    pub fn get(provider: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(SERVICE_NAME, provider)?;
        match entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
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
    /// 1. OAuth tokens (stored by /login)
    /// 2. API key (stored by /login <provider> <key>)
    /// 3. Config file
    /// 4. Environment variable
    pub fn resolve(provider: &str, config_key: Option<&str>, env_var: &str) -> String {
        // 1. OAuth tokens
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

        // 2. API key from keychain
        if let Ok(Some(key)) = Self::get(provider)
            && !key.is_empty()
        {
            return key;
        }

        // 3. Config file
        if let Some(key) = config_key
            && !key.is_empty()
        {
            return key.to_string();
        }

        // 4. Environment variable
        std::env::var(env_var).unwrap_or_default()
    }
}
