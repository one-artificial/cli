use std::collections::HashMap;

use anyhow::Result;
use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// --- Hugging Face OAuth constants ---

const HF_AUTHORIZE_URL: &str = "https://huggingface.co/oauth/authorize";
const HF_TOKEN_URL: &str = "https://huggingface.co/oauth/token";
const HF_USERINFO_URL: &str = "https://huggingface.co/oauth/userinfo";
const HF_CALLBACK_PORT: u16 = 54321;

/// Default HF client ID — can be overridden via HF_OAUTH_CLIENT_ID env var
fn hf_client_id() -> String {
    std::env::var("HF_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "87ad1313-3535-4403-a5b1-1b79721ddeaa".to_string())
}

/// HF client secret — loaded from env var
fn hf_client_secret() -> Option<String> {
    std::env::var("HF_OAUTH_CLIENT_SECRET").ok()
}

const HF_SCOPES: &[&str] = &["openid", "profile", "email", "inference-api"];

/// OAuth provider configuration.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub provider_name: String,
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    /// Redirect URL for manual (non-browser) flow
    pub manual_redirect_url: Option<String>,
}

/// Stored OAuth tokens with expiry tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
    pub provider: String,
    /// Account info returned by token exchange
    pub account_email: Option<String>,
    pub account_uuid: Option<String>,
}

impl OAuthTokens {
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            now >= (expires_at - 300) // 5 minute buffer
        } else {
            false
        }
    }
}

/// PKCE pair for OAuth authorization.
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
}

impl PkcePair {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let verifier_bytes: Vec<u8> = (0..32).map(|_| rng.r#gen()).collect();
        let code_verifier =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&verifier_bytes);

        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let digest = hasher.finalize();
        let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);

        Self {
            code_verifier,
            code_challenge,
        }
    }
}

fn random_state() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
}

/// Known OAuth configurations for supported providers.
pub fn oauth_config_for(provider: &str) -> Option<OAuthConfig> {
    match provider {
        "huggingface" | "hf" => Some(OAuthConfig {
            provider_name: "huggingface".to_string(),
            authorize_url: HF_AUTHORIZE_URL.to_string(),
            token_url: HF_TOKEN_URL.to_string(),
            client_id: hf_client_id(),
            scopes: HF_SCOPES.iter().map(|s| s.to_string()).collect(),
            manual_redirect_url: None,
        }),
        _ => None,
    }
}

/// Build the authorization URL with PKCE parameters.
/// `redirect_uri` is either localhost (auto) or the manual redirect URL.
pub fn build_auth_url(
    config: &OAuthConfig,
    pkce: &PkcePair,
    redirect_uri: &str,
    state: &str,
) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", &config.client_id),
        ("redirect_uri", redirect_uri),
        ("scope", &config.scopes.join(" ")),
        ("code_challenge", &pkce.code_challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];

    let query: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    format!("{}?{}", config.authorize_url, query)
}

/// Exchange authorization code for tokens.
pub async fn exchange_code(
    config: &OAuthConfig,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    state: &str,
) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(&config.token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", &config.client_id),
            ("code_verifier", code_verifier),
            ("state", state),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed: {body}");
    }

    let data: serde_json::Value = resp.json().await?;

    let access_token = data["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in response"))?
        .to_string();

    let refresh_token = data["refresh_token"].as_str().map(String::from);
    let expires_in = data["expires_in"].as_i64();
    let expires_at = expires_in.map(|ei| chrono::Utc::now().timestamp() + ei);

    // Extract account info from token response
    let account_email = data["account"]["email_address"].as_str().map(String::from);
    let account_uuid = data["account"]["uuid"].as_str().map(String::from);

    // Check which scopes were actually granted
    let granted_scopes = data["scope"]
        .as_str()
        .map(|s| s.split(' ').map(String::from).collect())
        .unwrap_or_else(|| config.scopes.clone());

    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_at,
        scopes: granted_scopes,
        provider: config.provider_name.clone(),
        account_email,
        account_uuid,
    })
}

/// Refresh an expired access token using the refresh token.
pub async fn refresh_token(config: &OAuthConfig, refresh_tok: &str) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(&config.token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_tok),
            ("client_id", &config.client_id),
            ("scope", &config.scopes.join(" ")),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token refresh failed: {body}");
    }

    let data: serde_json::Value = resp.json().await?;

    let access_token = data["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in refresh response"))?
        .to_string();

    let new_refresh = data["refresh_token"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| refresh_tok.to_string());

    let expires_in = data["expires_in"].as_i64();
    let expires_at = expires_in.map(|ei| chrono::Utc::now().timestamp() + ei);

    Ok(OAuthTokens {
        access_token,
        refresh_token: Some(new_refresh),
        expires_at,
        scopes: config.scopes.clone(),
        provider: config.provider_name.clone(),
        account_email: None,
        account_uuid: None,
    })
}

/// Result of the login flow — includes messages for the TUI to display.
pub struct LoginResult {
    pub tokens: OAuthTokens,
    pub messages: Vec<String>,
}

/// Run browser-based OAuth login for the given provider.
pub async fn browser_login(provider: &str) -> Result<LoginResult> {
    match provider {
        "huggingface" | "hf" => browser_login_hf().await,
        _ => anyhow::bail!(
            "OAuth login is only supported for Hugging Face. Use `/login {provider} <api_key>` to set an API key."
        ),
    }
}

/// Run the Hugging Face browser-based OAuth PKCE login flow.
///
/// Uses a fixed port (54321) matching the registered redirect URL.
/// After login, fetches userinfo for profile display.
pub async fn browser_login_hf() -> Result<LoginResult> {
    let config = oauth_config_for("huggingface")
        .ok_or_else(|| anyhow::anyhow!("No OAuth config for huggingface"))?;

    let pkce = PkcePair::generate();
    let state = random_state();

    // Bind to the fixed port matching the HF app's registered redirect URL
    let port = std::env::var("HF_OAUTH_REDIRECT_URL")
        .ok()
        .and_then(|url| {
            url.split(':')
                .next_back()?
                .split('/')
                .next()?
                .parse::<u16>()
                .ok()
        })
        .unwrap_or(HF_CALLBACK_PORT);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .map_err(|e| {
            anyhow::anyhow!("Failed to bind to port {port}: {e}. Is another instance running?")
        })?;
    let redirect_uri = format!("http://localhost:{port}/oauth/hf/callback");

    let auth_url = build_auth_url(&config, &pkce, &redirect_uri, &state);

    let mut messages = Vec::new();
    messages.push("Opening browser to sign in with Hugging Face...".to_string());

    let browser_opened = open::that(&auth_url).is_ok();
    if !browser_opened {
        messages.push(format!("Could not open browser. Visit:\n{auth_url}"));
    }

    tracing::info!("Waiting for HF OAuth callback on port {port}...");

    // Wait for the redirect
    let code = wait_for_hf_callback(listener, &state).await?;

    messages.push("Authorization received. Exchanging for tokens...".to_string());

    // Exchange code for tokens — HF requires client_secret
    let tokens =
        exchange_code_hf(&config, &code, &pkce.code_verifier, &redirect_uri, &state).await?;

    // Store OAuth tokens in keychain
    let tokens_json = serde_json::to_string(&tokens)?;
    crate::credentials::CredentialStore::store("huggingface_oauth", &tokens_json)?;
    crate::credentials::CredentialStore::store("huggingface", &tokens.access_token)?;

    // Fetch user profile
    match fetch_hf_userinfo(&tokens.access_token).await {
        Ok(userinfo) => {
            if let Some(name) = userinfo["name"].as_str() {
                messages.push(format!("Logged in as {name}"));
            } else if let Some(sub) = userinfo["sub"].as_str() {
                messages.push(format!("Logged in as {sub}"));
            }
            // Store username for reference
            if let Some(preferred) = userinfo["preferred_username"].as_str() {
                crate::credentials::CredentialStore::store("huggingface_username", preferred).ok();
            }
        }
        Err(e) => {
            tracing::warn!("Failed to fetch HF userinfo: {e}");
        }
    }

    messages.push("Hugging Face login complete. Credentials stored.".to_string());
    messages.push("HF Inference API is now available.".to_string());

    tracing::info!("HF OAuth login successful");
    Ok(LoginResult { tokens, messages })
}

/// Exchange HF authorization code for tokens (includes client_secret).
async fn exchange_code_hf(
    config: &OAuthConfig,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    state: &str,
) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", &config.client_id),
        ("code_verifier", code_verifier),
        ("state", state),
    ];

    // HF requires client_secret for confidential apps
    let secret = hf_client_secret();
    if let Some(ref s) = secret {
        form.push(("client_secret", s));
    }

    let resp = client.post(&config.token_url).form(&form).send().await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("HF token exchange failed: {body}");
    }

    let data: serde_json::Value = resp.json().await?;

    let access_token = data["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in HF response"))?
        .to_string();

    let refresh_token = data["refresh_token"].as_str().map(String::from);
    let expires_in = data["expires_in"].as_i64();
    let expires_at = expires_in.map(|ei| chrono::Utc::now().timestamp() + ei);

    let granted_scopes = data["scope"]
        .as_str()
        .map(|s| s.split(' ').map(String::from).collect())
        .unwrap_or_else(|| config.scopes.clone());

    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_at,
        scopes: granted_scopes,
        provider: "huggingface".to_string(),
        account_email: None,
        account_uuid: None,
    })
}

/// Fetch Hugging Face user profile from the userinfo endpoint.
async fn fetch_hf_userinfo(access_token: &str) -> Result<serde_json::Value> {
    let client = reqwest::Client::new();
    let resp = client
        .get(HF_USERINFO_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("HF userinfo failed: {}", resp.status());
    }

    Ok(resp.json().await?)
}

/// Wait for the HF OAuth callback (handles the /oauth/hf/callback path).
async fn wait_for_hf_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let timeout = tokio::time::timeout(std::time::Duration::from_secs(120), listener.accept())
        .await
        .map_err(|_| anyhow::anyhow!("HF login timed out after 120 seconds"))??;

    let (mut stream, _addr) = timeout;

    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");

    let query = path.split('?').nth(1).unwrap_or("");
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .collect();

    let code = params
        .get("code")
        .ok_or_else(|| anyhow::anyhow!("No code in HF callback. User may have denied access."))?;

    let state = params.get("state").unwrap_or(&"");
    if *state != expected_state {
        anyhow::bail!("State mismatch — possible CSRF attack. Please try again.");
    }

    // Send a simple success page back to the browser
    let success_html = r#"<!DOCTYPE html><html><body style="font-family:sans-serif;text-align:center;padding:40px">
<h1>Login successful!</h1><p>You can close this tab and return to One.</p>
</body></html>"#;

    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        success_html.len(),
        success_html
    );

    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;

    Ok(code.to_string())
}

/// Attempt to refresh stored tokens. Returns the new access token
/// or an error if refresh failed.
pub async fn try_refresh(provider: &str) -> Result<String> {
    let config = oauth_config_for(provider)
        .ok_or_else(|| anyhow::anyhow!("No OAuth config for {provider}"))?;

    let stored_json = crate::credentials::CredentialStore::get(&format!("{provider}_oauth"))?
        .ok_or_else(|| anyhow::anyhow!("No stored OAuth tokens for {provider}"))?;

    let stored: OAuthTokens = serde_json::from_str(&stored_json)?;

    let refresh_tok = stored
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No refresh token stored for {provider}"))?;

    let new_tokens = refresh_token(&config, refresh_tok).await?;

    // Update stored tokens
    let new_json = serde_json::to_string(&new_tokens)?;
    crate::credentials::CredentialStore::store(&format!("{provider}_oauth"), &new_json)?;
    crate::credentials::CredentialStore::store(provider, &new_tokens.access_token)?;

    Ok(new_tokens.access_token)
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 3);
        for byte in s.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(byte as char);
                }
                _ => {
                    result.push_str(&format!("%{:02X}", byte));
                }
            }
        }
        result
    }
}
