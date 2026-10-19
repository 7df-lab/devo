use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::ProviderConfigError;

use super::persistence::write_atomic;
use super::schema::AUTH_CONFIG_VERSION;
use super::schema::AuthCredentialConfig;
use super::schema::AuthCredentialKind;
use super::schema::UserAuthConfigFile;

pub const AUTH_CONFIG_FILE_NAME: &str = "auth.json";

/// OAuth token material written into user-scoped `auth.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertOauthCredential {
    pub access: String,
    pub refresh: Option<String>,
    pub expires_at: Option<i64>,
    pub account_id: Option<String>,
    pub enterprise_url: Option<String>,
}

/// Returns the credential id used when onboarding receives an API key without
/// an explicit id.
///
/// The auth.json key is the provider id.
pub fn default_provider_credential_id(provider_id: &str) -> String {
    normalize_provider_key(provider_id)
}

/// Legacy `{provider}_api_key` id kept for migrate-on-read / resolve fallbacks.
pub fn legacy_provider_api_key_credential_id(provider_id: &str) -> String {
    let normalized = normalize_provider_key(provider_id);
    if normalized.is_empty() || normalized == "provider" {
        "provider_api_key".to_string()
    } else {
        format!("{normalized}_api_key")
    }
}

fn normalize_provider_key(provider_id: &str) -> String {
    let normalized = provider_id
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == ':' {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    normalized.trim_matches('_').to_string()
}

/// Maps legacy credential ids (`deepseek_api_key`, `openai_oauth`) to the
/// provider-keyed ids (AuthStorage contract).
pub fn provider_id_from_credential_id(credential_id: &str) -> String {
    let id = credential_id.trim();
    if let Some(provider) = id.strip_suffix("_api_key")
        && !provider.is_empty()
    {
        return provider.to_string();
    }
    if let Some(provider) = id.strip_suffix("_oauth")
        && !provider.is_empty()
    {
        return provider.to_string();
    }
    id.to_string()
}

/// Upserts one API key credential into user-scoped `auth.json`.
pub fn upsert_user_auth_api_key(
    user_config_dir: &Path,
    credential_id: &str,
    value: &str,
) -> Result<(), ProviderConfigError> {
    if value.is_empty() {
        return Err(ProviderConfigError::Validation {
            message: format!("credential `{credential_id}` has an empty API key value"),
        });
    }
    let auth_file = user_config_dir.join(AUTH_CONFIG_FILE_NAME);
    let mut auth = read_user_auth_config(&auth_file)?;
    let key = provider_id_from_credential_id(credential_id);
    auth.credentials.insert(
        key,
        AuthCredentialConfig {
            kind: AuthCredentialKind::ApiKey,
            value: value.to_string(),
            access: None,
            refresh: None,
            expires_at: None,
            account_id: None,
            enterprise_url: None,
        },
    );
    write_user_auth_config(&auth_file, &auth)
}

/// Upserts one OAuth credential into user-scoped `auth.json`.
pub fn upsert_user_auth_oauth(
    user_config_dir: &Path,
    credential_id: &str,
    oauth: UpsertOauthCredential,
) -> Result<(), ProviderConfigError> {
    if oauth.access.is_empty() {
        return Err(ProviderConfigError::Validation {
            message: format!("credential `{credential_id}` has an empty OAuth access token"),
        });
    }
    let auth_file = user_config_dir.join(AUTH_CONFIG_FILE_NAME);
    let mut auth = read_user_auth_config(&auth_file)?;
    let key = provider_id_from_credential_id(credential_id);
    auth.credentials.insert(
        key,
        AuthCredentialConfig {
            kind: AuthCredentialKind::Oauth,
            value: String::new(),
            access: Some(oauth.access),
            refresh: non_empty_optional(oauth.refresh),
            expires_at: oauth.expires_at,
            account_id: non_empty_optional(oauth.account_id),
            enterprise_url: non_empty_optional(oauth.enterprise_url),
        },
    );
    write_user_auth_config(&auth_file, &auth)
}

/// Removes a user-scoped credential from auth.json.
///
/// Returning Ok(false) means the credential id was not present. The file is
/// still written when a credential was removed so the operation is durable.
pub fn remove_user_auth_credential(
    user_config_dir: &Path,
    credential_id: &str,
) -> Result<bool, ProviderConfigError> {
    let auth_file = user_config_dir.join(AUTH_CONFIG_FILE_NAME);
    let mut auth = read_user_auth_config(&auth_file)?;
    let key = provider_id_from_credential_id(credential_id);
    let removed =
        auth.credentials.remove(&key).is_some() || auth.credentials.remove(credential_id).is_some();
    if removed {
        write_user_auth_config(&auth_file, &auth)?;
    }
    Ok(removed)
}

pub fn read_user_auth_config(auth_file: &Path) -> Result<UserAuthConfigFile, ProviderConfigError> {
    if !auth_file.exists() {
        return Ok(UserAuthConfigFile::default());
    }

    let data = fs::read_to_string(auth_file).map_err(|source| ProviderConfigError::Io {
        action: "read",
        path: auth_file.to_path_buf(),
        source,
    })?;
    let (auth, migrated) = parse_user_auth_config(&data, auth_file)?;
    for (credential_id, credential) in &auth.credentials {
        validate_auth_credential(credential_id, credential)?;
    }
    if migrated {
        // Rewrite legacy envelope / `*_api_key` keys into the pi provider-keyed shape.
        write_user_auth_config(auth_file, &auth)?;
    }
    Ok(auth)
}

fn parse_user_auth_config(
    data: &str,
    auth_file: &Path,
) -> Result<(UserAuthConfigFile, bool), ProviderConfigError> {
    let value: Value =
        serde_json::from_str(data).map_err(|error| ProviderConfigError::ParseAuth {
            path: auth_file.to_path_buf(),
            message: error.to_string(),
        })?;
    let Value::Object(root) = value else {
        return Err(ProviderConfigError::ParseAuth {
            path: auth_file.to_path_buf(),
            message: "auth.json must be a JSON object".to_string(),
        });
    };

    let legacy_envelope = root.contains_key("credentials") || root.contains_key("version");
    let mut credentials = BTreeMap::new();
    let mut migrated = legacy_envelope;

    if legacy_envelope {
        if let Some(version) = root.get("version") {
            let version = version.as_u64().unwrap_or(0) as u32;
            if version != 0 && version != AUTH_CONFIG_VERSION {
                return Err(ProviderConfigError::Validation {
                    message: format!(
                        "unsupported auth.json schema version {version} at {}",
                        auth_file.display()
                    ),
                });
            }
        }
        let Some(Value::Object(entries)) = root.get("credentials") else {
            return Ok((UserAuthConfigFile::default(), legacy_envelope));
        };
        for (credential_id, entry) in entries {
            let credential = parse_credential_value(entry).map_err(|message| {
                ProviderConfigError::ParseAuth {
                    path: auth_file.to_path_buf(),
                    message,
                }
            })?;
            let key = provider_id_from_credential_id(credential_id);
            if key != *credential_id {
                migrated = true;
            }
            credentials.entry(key).or_insert(credential);
        }
    } else {
        for (provider_id, entry) in root {
            if provider_id == "version" || provider_id == "credentials" {
                continue;
            }
            let credential = parse_credential_value(&entry).map_err(|message| {
                ProviderConfigError::ParseAuth {
                    path: auth_file.to_path_buf(),
                    message,
                }
            })?;
            let key = provider_id_from_credential_id(&provider_id);
            if key != provider_id {
                migrated = true;
            }
            credentials.entry(key).or_insert(credential);
        }
    }

    Ok((
        UserAuthConfigFile {
            version: AUTH_CONFIG_VERSION,
            credentials,
        },
        migrated,
    ))
}

fn parse_credential_value(value: &Value) -> Result<AuthCredentialConfig, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "credential entry must be an object".to_string())?;
    let kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .and_then(Value::as_str)
        .ok_or_else(|| "credential missing type/kind".to_string())?;
    match kind {
        "api_key" => {
            let key = object
                .get("key")
                .or_else(|| object.get("value"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(AuthCredentialConfig {
                kind: AuthCredentialKind::ApiKey,
                value: key,
                access: None,
                refresh: None,
                expires_at: None,
                account_id: None,
                enterprise_url: None,
            })
        }
        "oauth" => {
            let expires_at = object
                .get("expires")
                .or_else(|| object.get("expires_at"))
                .and_then(Value::as_i64);
            Ok(AuthCredentialConfig {
                kind: AuthCredentialKind::Oauth,
                value: String::new(),
                access: object
                    .get("access")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                refresh: object
                    .get("refresh")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                expires_at,
                account_id: object
                    .get("accountId")
                    .or_else(|| object.get("account_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                enterprise_url: object
                    .get("enterpriseUrl")
                    .or_else(|| object.get("enterprise_url"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        }
        other => Err(format!("unsupported credential type `{other}`")),
    }
}

fn validate_auth_credential(
    credential_id: &str,
    credential: &AuthCredentialConfig,
) -> Result<(), ProviderConfigError> {
    match credential.kind {
        AuthCredentialKind::ApiKey => {
            if credential.value.is_empty() {
                return Err(ProviderConfigError::Validation {
                    message: format!(
                        "credential `{credential_id}` in auth.json has an empty value"
                    ),
                });
            }
        }
        AuthCredentialKind::Oauth => {
            let access_empty = credential
                .access
                .as_ref()
                .is_none_or(|access| access.is_empty());
            if access_empty {
                return Err(ProviderConfigError::Validation {
                    message: format!(
                        "credential `{credential_id}` in auth.json has an empty OAuth access token"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn write_user_auth_config(
    auth_file: &Path,
    auth: &UserAuthConfigFile,
) -> Result<(), ProviderConfigError> {
    if let Some(parent) = auth_file.parent() {
        fs::create_dir_all(parent).map_err(|source| ProviderConfigError::Io {
            action: "create",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let data = serde_json::to_vec_pretty(&auth_to_pi_wire(auth)).map_err(|error| {
        ProviderConfigError::Serialize {
            message: error.to_string(),
        }
    })?;
    write_atomic(auth_file, &data)?;
    restrict_auth_file_permissions(auth_file);
    Ok(())
}

/// Serializes auth in the AuthStorage shape:
/// `{ "<providerId>": { "type": "api_key", "key": "..." }, ... }`.
fn auth_to_pi_wire(auth: &UserAuthConfigFile) -> Value {
    let mut root = serde_json::Map::new();
    for (provider_id, credential) in &auth.credentials {
        let entry = match credential.kind {
            AuthCredentialKind::ApiKey => {
                let mut object = serde_json::Map::new();
                object.insert("type".into(), Value::String("api_key".into()));
                object.insert("key".into(), Value::String(credential.value.clone()));
                Value::Object(object)
            }
            AuthCredentialKind::Oauth => {
                let mut object = serde_json::Map::new();
                object.insert("type".into(), Value::String("oauth".into()));
                if let Some(access) = &credential.access {
                    object.insert("access".into(), Value::String(access.clone()));
                }
                if let Some(refresh) = &credential.refresh {
                    object.insert("refresh".into(), Value::String(refresh.clone()));
                }
                if let Some(expires) = credential.expires_at {
                    object.insert("expires".into(), Value::Number(expires.into()));
                }
                if let Some(account_id) = &credential.account_id {
                    object.insert("accountId".into(), Value::String(account_id.clone()));
                }
                if let Some(enterprise_url) = &credential.enterprise_url {
                    object.insert(
                        "enterpriseUrl".into(),
                        Value::String(enterprise_url.clone()),
                    );
                }
                Value::Object(object)
            }
        };
        root.insert(provider_id.clone(), entry);
    }
    Value::Object(root)
}

/// Sets Unix mode `0600` after write. On Windows this is best-effort and must
/// never panic.
fn restrict_auth_file_permissions(auth_file: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(auth_file, fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        let _ = auth_file;
    }
}

fn non_empty_optional(value: Option<String>) -> Option<String> {
    value.filter(|entry| !entry.is_empty())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::AuthCredentialKind;

    /// Trace: L2-DES-AUTH-001
    /// Verifies: API key upsert/read roundtrip stores under the provider id and
    /// writes the pi AuthStorage wire shape.
    #[test]
    fn upsert_api_key_roundtrips_provider_keyed_pi_shape() {
        let dir = tempfile::tempdir().expect("temp dir");
        upsert_user_auth_api_key(dir.path(), "openai", "sk-test").expect("upsert api key");

        let auth = read_user_auth_config(&dir.path().join(AUTH_CONFIG_FILE_NAME)).expect("read");
        assert_eq!(
            auth.credentials.get("openai"),
            Some(&AuthCredentialConfig {
                kind: AuthCredentialKind::ApiKey,
                value: "sk-test".to_string(),
                access: None,
                refresh: None,
                expires_at: None,
                account_id: None,
                enterprise_url: None,
            })
        );

        let raw = std::fs::read_to_string(dir.path().join(AUTH_CONFIG_FILE_NAME)).expect("raw");
        assert!(!raw.contains("\"credentials\""));
        assert!(!raw.contains("\"version\""));
        assert!(raw.contains("\"openai\""));
        assert!(raw.contains("\"type\": \"api_key\""));
        assert!(raw.contains("\"key\": \"sk-test\""));
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: legacy `*_api_key` ids migrate to provider keys on upsert.
    #[test]
    fn upsert_legacy_api_key_id_normalizes_to_provider() {
        let dir = tempfile::tempdir().expect("temp dir");
        upsert_user_auth_api_key(dir.path(), "deepseek_api_key", "sk-ds").expect("upsert");
        let auth = read_user_auth_config(&dir.path().join(AUTH_CONFIG_FILE_NAME)).expect("read");
        assert!(auth.credentials.contains_key("deepseek"));
        assert!(!auth.credentials.contains_key("deepseek_api_key"));
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: OAuth upsert/read roundtrip persists access and optional fields.
    #[test]
    fn upsert_oauth_roundtrips() {
        let dir = tempfile::tempdir().expect("temp dir");
        upsert_user_auth_oauth(
            dir.path(),
            "openai-codex",
            UpsertOauthCredential {
                access: "access-token".to_string(),
                refresh: Some("refresh-token".to_string()),
                expires_at: Some(1_700_000_000),
                account_id: Some("acct_1".to_string()),
                enterprise_url: Some("https://enterprise.example".to_string()),
            },
        )
        .expect("upsert oauth");

        let auth = read_user_auth_config(&dir.path().join(AUTH_CONFIG_FILE_NAME)).expect("read");
        assert_eq!(
            auth.credentials.get("openai-codex"),
            Some(&AuthCredentialConfig {
                kind: AuthCredentialKind::Oauth,
                value: String::new(),
                access: Some("access-token".to_string()),
                refresh: Some("refresh-token".to_string()),
                expires_at: Some(1_700_000_000),
                account_id: Some("acct_1".to_string()),
                enterprise_url: Some("https://enterprise.example".to_string()),
            })
        );

        let raw = std::fs::read_to_string(dir.path().join(AUTH_CONFIG_FILE_NAME)).expect("raw");
        assert!(!raw.contains("\"value\""));
        assert!(raw.contains("\"type\": \"oauth\""));
        assert!(raw.contains("\"access\": \"access-token\""));
        assert!(raw.contains("\"expires\": 1700000000"));
        assert!(raw.contains("\"accountId\""));
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: empty OAuth access is rejected on upsert.
    #[test]
    fn upsert_oauth_rejects_empty_access() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = upsert_user_auth_oauth(
            dir.path(),
            "anthropic",
            UpsertOauthCredential {
                access: String::new(),
                refresh: None,
                expires_at: None,
                account_id: None,
                enterprise_url: None,
            },
        )
        .expect_err("empty access");
        assert!(error.to_string().contains("empty OAuth access"));
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: empty API key value is rejected on upsert.
    #[test]
    fn upsert_api_key_rejects_empty_value() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = upsert_user_auth_api_key(dir.path(), "openai", "").expect_err("empty value");
        assert!(error.to_string().contains("empty API key value"));
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: API key and OAuth credentials can coexist in one auth.json.
    #[test]
    fn api_key_and_oauth_coexist() {
        let dir = tempfile::tempdir().expect("temp dir");
        upsert_user_auth_api_key(dir.path(), "openai", "sk-test").expect("api key");
        upsert_user_auth_oauth(
            dir.path(),
            "xai",
            UpsertOauthCredential {
                access: "xai-access".to_string(),
                refresh: None,
                expires_at: None,
                account_id: None,
                enterprise_url: None,
            },
        )
        .expect("oauth");

        let auth = read_user_auth_config(&dir.path().join(AUTH_CONFIG_FILE_NAME)).expect("read");
        assert_eq!(auth.credentials.len(), 2);
        assert_eq!(auth.credentials["openai"].kind, AuthCredentialKind::ApiKey);
        assert_eq!(auth.credentials["xai"].kind, AuthCredentialKind::Oauth);
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: legacy envelope auth.json migrates to provider-keyed pi shape.
    #[test]
    fn migrates_legacy_envelope_on_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(AUTH_CONFIG_FILE_NAME);
        std::fs::write(
            &path,
            r#"{
              "version": 1,
              "credentials": {
                "deepseek_api_key": { "kind": "api_key", "value": "sk-legacy" }
              }
            }"#,
        )
        .expect("write legacy");

        let auth = read_user_auth_config(&path).expect("read migrates");
        assert_eq!(auth.credentials["deepseek"].value, "sk-legacy");

        let raw = std::fs::read_to_string(&path).expect("rewritten");
        assert!(!raw.contains("\"credentials\""));
        assert!(raw.contains("\"deepseek\""));
        assert!(raw.contains("\"type\": \"api_key\""));
        assert!(raw.contains("\"key\": \"sk-legacy\""));
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: auth.json is written with mode 0600 on Unix.
    #[cfg(unix)]
    #[test]
    fn auth_file_mode_is_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        upsert_user_auth_api_key(dir.path(), "k", "v").expect("upsert");
        let mode = std::fs::metadata(dir.path().join(AUTH_CONFIG_FILE_NAME))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn default_credential_id_is_provider_id() {
        assert_eq!(default_provider_credential_id("deepseek"), "deepseek");
        assert_eq!(
            legacy_provider_api_key_credential_id("deepseek"),
            "deepseek_api_key"
        );
    }
}
