//! Provider OAuth access-token refresh at the server boundary.

use std::path::Path;

use anyhow::{Context, Result};
use devo_core::{AuthCredentialConfig, UpsertOauthCredential, upsert_user_auth_oauth};
use reqwest::Client;
use serde::Deserialize;

/// Refresh slightly before expiry so a token cannot expire during a request.
const REFRESH_WINDOW_SECONDS: i64 = 60;
/// ChatGPT/Codex OAuth token endpoint.
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Public OAuth client used by the Codex CLI authorization flow.
const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Exchanges a durable GitHub token for a short-lived Copilot API token.
const GITHUB_COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
/// Anthropic OAuth token endpoint (Claude Pro/Max).
const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// xAI device OAuth token endpoint.
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.48.1";
const COPILOT_EDITOR_VERSION: &str = "vscode/1.136.1";
const COPILOT_EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.48.1";
const COPILOT_INTEGRATION_ID: &str = "vscode-chat";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshDecision {
    UseCurrent,
    Refresh,
    ExpiredWithoutRefresh,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: i64,
}

pub(crate) async fn resolve_oauth_access(
    provider_id: &str,
    credential_id: &str,
    credential: &AuthCredentialConfig,
    user_config_dir: &Path,
) -> Result<String> {
    let now = unix_timestamp();
    match refresh_decision(credential, now) {
        RefreshDecision::UseCurrent => current_access(credential_id, credential),
        RefreshDecision::ExpiredWithoutRefresh => anyhow::bail!(
            "oauth credential `{credential_id}` for provider `{provider_id}` is expired and has no refresh token; run /login to re-authenticate"
        ),
        RefreshDecision::Refresh => {
            let refresh = credential
                .refresh
                .as_deref()
                .context("OAuth refresh token disappeared during refresh")?;
            let refreshed = match refresh_provider_token(provider_id, refresh, now).await {
                Ok(token) => token,
                Err(error) => {
                    anyhow::bail!(
                        "oauth refresh failed for provider `{provider_id}` (credential `{credential_id}`): {error}; run /login to re-authenticate"
                    );
                }
            };
            let access = refreshed.access_token.clone();
            upsert_user_auth_oauth(
                user_config_dir,
                credential_id,
                UpsertOauthCredential {
                    access: refreshed.access_token,
                    // OAuth servers may rotate refresh tokens. When they do
                    // not, retain the existing token rather than deleting it.
                    refresh: refreshed
                        .refresh_token
                        .filter(|token| !token.is_empty())
                        .or_else(|| Some(refresh.to_string())),
                    expires_at: refreshed
                        .expires_at
                        .or_else(|| refreshed.expires_in.map(|seconds| now + seconds)),
                    account_id: credential.account_id.clone(),
                    enterprise_url: credential.enterprise_url.clone(),
                },
            )
            .context("failed to persist refreshed OAuth credential")?;
            Ok(access)
        }
    }
}

fn refresh_decision(credential: &AuthCredentialConfig, now: i64) -> RefreshDecision {
    let Some(expires_at) = credential.expires_at else {
        return RefreshDecision::UseCurrent;
    };
    if expires_at > now + REFRESH_WINDOW_SECONDS {
        return RefreshDecision::UseCurrent;
    }
    if credential
        .refresh
        .as_deref()
        .is_some_and(|refresh| !refresh.is_empty())
    {
        RefreshDecision::Refresh
    } else {
        RefreshDecision::ExpiredWithoutRefresh
    }
}

async fn refresh_provider_token(
    provider_id: &str,
    refresh_token: &str,
    now: i64,
) -> Result<TokenResponse> {
    match provider_id {
        "openai-codex" => refresh_openai(refresh_token).await,
        "github-copilot" => refresh_github_copilot(refresh_token, now).await,
        "anthropic" => refresh_anthropic(refresh_token).await,
        "xai" => refresh_xai(refresh_token).await,
        _ => anyhow::bail!(
            "OAuth refresh is not supported for provider `{provider_id}`; run /login to re-authenticate"
        ),
    }
}

async fn refresh_openai(refresh_token: &str) -> Result<TokenResponse> {
    let response = Client::new()
        .post(OPENAI_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OPENAI_CODEX_CLIENT_ID),
        ])
        .send()
        .await
        .context("failed to contact OpenAI OAuth token endpoint")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("OpenAI OAuth refresh failed with HTTP {status}");
    }
    let token: TokenResponse = response
        .json()
        .await
        .context("OpenAI OAuth refresh returned an invalid response")?;
    if token.access_token.is_empty() {
        anyhow::bail!("OpenAI OAuth refresh returned an empty access token");
    }
    Ok(token)
}

async fn refresh_github_copilot(refresh_token: &str, now: i64) -> Result<TokenResponse> {
    let response = Client::new()
        .get(GITHUB_COPILOT_TOKEN_URL)
        .header("Authorization", format!("Bearer {refresh_token}"))
        .header("Accept", "application/json")
        .header("User-Agent", COPILOT_USER_AGENT)
        .header("Editor-Version", COPILOT_EDITOR_VERSION)
        .header("Editor-Plugin-Version", COPILOT_EDITOR_PLUGIN_VERSION)
        .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
        .send()
        .await
        .context("failed to contact GitHub Copilot token endpoint")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("GitHub Copilot token refresh failed with HTTP {status}");
    }
    let token: CopilotTokenResponse = response
        .json()
        .await
        .context("GitHub Copilot token refresh returned an invalid response")?;
    if token.token.is_empty() {
        anyhow::bail!("GitHub Copilot token refresh returned an empty access token");
    }
    Ok(TokenResponse {
        access_token: token.token,
        refresh_token: None,
        expires_in: None,
        expires_at: Some(token.expires_at.max(now)),
    })
}

async fn refresh_anthropic(refresh_token: &str) -> Result<TokenResponse> {
    let response = Client::new()
        .post(ANTHROPIC_TOKEN_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": ANTHROPIC_CLIENT_ID,
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .context("failed to contact Anthropic OAuth token endpoint")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Anthropic OAuth refresh failed with HTTP {status}");
    }
    let token: TokenResponse = response
        .json()
        .await
        .context("Anthropic OAuth refresh returned an invalid response")?;
    if token.access_token.is_empty() {
        anyhow::bail!("Anthropic OAuth refresh returned an empty access token");
    }
    Ok(token)
}

async fn refresh_xai(refresh_token: &str) -> Result<TokenResponse> {
    let response = Client::new()
        .post(XAI_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", XAI_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .context("failed to contact xAI OAuth token endpoint")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("xAI OAuth refresh failed with HTTP {status}");
    }
    let token: TokenResponse = response
        .json()
        .await
        .context("xAI OAuth refresh returned an invalid response")?;
    if token.access_token.is_empty() {
        anyhow::bail!("xAI OAuth refresh returned an empty access token");
    }
    Ok(token)
}

fn current_access(credential_id: &str, credential: &AuthCredentialConfig) -> Result<String> {
    credential
        .access
        .as_deref()
        .filter(|access| !access.is_empty())
        .map(str::to_string)
        .with_context(|| format!("oauth credential `{credential_id}` is missing an access token"))
}

fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use devo_core::AuthCredentialKind;
    use pretty_assertions::assert_eq;

    use super::*;

    fn oauth(expires_at: Option<i64>, refresh: Option<&str>) -> AuthCredentialConfig {
        AuthCredentialConfig {
            kind: AuthCredentialKind::Oauth,
            value: String::new(),
            access: Some("access".to_string()),
            refresh: refresh.map(str::to_string),
            expires_at,
            account_id: None,
            enterprise_url: None,
        }
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: an OAuth access token near expiry is refreshed when possible.
    #[test]
    fn near_expiry_with_refresh_token_refreshes() {
        assert_eq!(
            refresh_decision(&oauth(Some(1_050), Some("refresh")), 1_000),
            RefreshDecision::Refresh
        );
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: an OAuth access token outside the refresh window is reused.
    #[test]
    fn token_outside_refresh_window_is_reused() {
        assert_eq!(
            refresh_decision(&oauth(Some(1_061), Some("refresh")), 1_000),
            RefreshDecision::UseCurrent
        );
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: an expired OAuth token without refresh material is stale.
    #[test]
    fn expired_token_without_refresh_is_stale() {
        assert_eq!(
            refresh_decision(&oauth(Some(999), None), 1_000),
            RefreshDecision::ExpiredWithoutRefresh
        );
    }
}
