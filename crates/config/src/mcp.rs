use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Environment variable forwarding rule for configured MCP servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerEnvVar {
    /// Legacy config shape where the string is the environment variable name.
    Name(String),
    /// Explicit config shape that may choose where the value should be read.
    Config {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
}

impl McpServerEnvVar {
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) | Self::Config { name, .. } => name,
        }
    }

    pub fn is_remote_source(&self) -> bool {
        matches!(
            self,
            Self::Config {
                source: Some(source),
                ..
            } if source == "remote"
        )
    }
}

impl From<String> for McpServerEnvVar {
    fn from(value: String) -> Self {
        Self::Name(value)
    }
}

impl From<&str> for McpServerEnvVar {
    fn from(value: &str) -> Self {
        Self::Name(value.to_string())
    }
}

/// Stores normalized MCP runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// The configured MCP servers.
    #[serde(default)]
    pub servers: Vec<McpServerRecord>,
    /// Whether enabled servers should be auto-started during bootstrap.
    #[serde(default = "default_mcp_auto_start")]
    pub auto_start: bool,
}

/// MCP host-level configuration stored under `[mcp]`.
///
/// Server records live under `[mcp_servers.<server_id>]` and are merged by
/// TOML table key (server id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpHostConfig {
    /// Whether enabled servers should be auto-started during bootstrap.
    #[serde(default = "default_mcp_auto_start")]
    pub auto_start: bool,
}

impl Default for McpHostConfig {
    fn default() -> Self {
        Self {
            auto_start: default_mcp_auto_start(),
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            auto_start: true,
        }
    }
}

fn default_mcp_auto_start() -> bool {
    true
}

/// Stores the configured metadata for one MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerRecord {
    /// The stable unique server identifier.
    pub id: McpServerId,
    /// The human-readable display name for the server.
    pub display_name: String,
    /// The transport configuration used to connect to the server.
    pub transport: McpTransportConfig,
    /// The startup policy applied to the server.
    #[serde(default)]
    pub startup_policy: McpStartupPolicy,
    /// Whether the server is enabled for runtime use.
    #[serde(default = "default_mcp_server_enabled")]
    pub enabled: bool,
    /// Trust policy for this MCP server.
    #[serde(default)]
    pub trust_policy: McpTrustPolicy,
    /// Allowed capabilities.
    #[serde(default)]
    pub allowed_capabilities: Vec<McpCapability>,
    /// Filesystem roots policy for resource access.
    #[serde(default)]
    pub roots_policy: McpRootsPolicy,
    /// Output limits for tool invocations.
    #[serde(default)]
    pub output_limits: McpOutputLimits,
    /// Optional auth credential reference.
    #[serde(default)]
    pub auth_ref: Option<String>,
}

fn default_mcp_server_enabled() -> bool {
    true
}

fn default_mcp_server_enabled_is_true(enabled: &bool) -> bool {
    *enabled
}

fn default_mcp_startup_policy() -> McpStartupPolicy {
    McpStartupPolicy::Lazy
}

/// Strongly typed identifier for one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpServerId(pub String);

impl std::fmt::Display for McpServerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Describes how the runtime connects to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportConfig {
    /// Launch the server as a stdio child process.
    Stdio {
        /// The command and arguments used to launch the server.
        command: Vec<String>,
        /// The working directory for the child process, if any.
        cwd: Option<PathBuf>,
        /// Environment variables provided directly to the child process.
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Environment variables inherited from the local process.
        #[serde(default)]
        env_vars: Vec<McpServerEnvVar>,
    },
    /// Connect to the server over streamable HTTP.
    StreamableHttp {
        /// The MCP server endpoint URL.
        url: String,
        /// Optional authentication configuration.
        #[serde(default)]
        auth: Option<McpAuthConfig>,
        /// Static HTTP headers sent to the MCP server.
        #[serde(default)]
        http_headers: BTreeMap<String, String>,
        /// HTTP headers loaded from local environment variables.
        #[serde(default)]
        env_http_headers: BTreeMap<String, String>,
    },
    /// Connect to the server over the deprecated MCP HTTP+SSE transport.
    Sse {
        /// The MCP server SSE endpoint URL.
        url: String,
        /// Optional authentication configuration.
        #[serde(default)]
        auth: Option<McpAuthConfig>,
        /// Static HTTP headers sent to the MCP server.
        #[serde(default)]
        http_headers: BTreeMap<String, String>,
        /// HTTP headers loaded from local environment variables.
        #[serde(default)]
        env_http_headers: BTreeMap<String, String>,
    },
}

/// Stores authentication configuration for MCP HTTP transports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAuthConfig {
    /// Use a bearer token for authorization.
    BearerToken {
        /// The bearer token value.
        token: String,
    },
}

/// Controls when an enabled MCP server should be started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupPolicy {
    /// Start the server automatically during runtime bootstrap.
    #[default]
    Eager,
    /// Start the server lazily on first use.
    Lazy,
    /// Never auto-start the server; start only by explicit request.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTrustPolicy {
    #[default]
    User,
    Workspace,
    Untrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpCapability {
    Tools,
    Resources,
    Prompts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpRootsPolicy {
    #[default]
    None,
    Workspace,
    Custom(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOutputLimits {
    #[serde(default)]
    pub max_tool_output_bytes: Option<u64>,
    #[serde(default)]
    pub max_resource_bytes: Option<u64>,
}

impl Default for McpOutputLimits {
    fn default() -> Self {
        Self {
            max_tool_output_bytes: Some(1_048_576),
            max_resource_bytes: Some(10_485_760),
        }
    }
}

impl McpOutputLimits {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpHttpTransportTypeToml {
    /// HTTP using the streamable HTTP transport.
    Http,
    /// Explicit `streamable_http` value (alias of `http`).
    StreamableHttp,
    /// Deprecated SSE transport.
    Sse,
}

/// Persisted MCP server record shape matching `[mcp_servers.<server_id>]`.
///
/// This is the only shape written by `devo mcp` and loaded from user/workspace
/// `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerRecordToml {
    /// Whether the server may be used.
    #[serde(default = "default_mcp_server_enabled")]
    #[serde(skip_serializing_if = "default_mcp_server_enabled_is_true")]
    pub enabled: bool,

    /// User-facing server name.
    ///
    /// When omitted, the runtime defaults it to the server id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    /// When an enabled MCP server should be started.
    #[serde(default = "default_mcp_startup_policy")]
    #[serde(skip_serializing_if = "McpStartupPolicy::is_default_lazy")]
    pub startup_policy: McpStartupPolicy,

    /// Trust policy for this MCP server.
    #[serde(default)]
    #[serde(skip_serializing_if = "McpTrustPolicy::is_default_user")]
    pub trust_policy: McpTrustPolicy,

    /// Optional allowlist for server capabilities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_capabilities: Vec<McpCapability>,

    /// Filesystem roots policy.
    #[serde(default)]
    #[serde(skip_serializing_if = "McpRootsPolicy::is_default_none")]
    pub roots_policy: McpRootsPolicy,

    /// Optional output limits.
    #[serde(default, skip_serializing_if = "McpOutputLimits::is_default")]
    pub output_limits: McpOutputLimits,

    /// Optional auth credential reference (not yet wired by runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_ref: Option<String>,

    // ----- Stdio -----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_vars: Vec<McpServerEnvVar>,

    // ----- HTTP / SSE -----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// MCP transport selector for HTTP/SSE.
    ///
    /// When omitted, `streamable_http` is assumed.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub transport_type: Option<McpHttpTransportTypeToml>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub http_headers: BTreeMap<String, String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_http_headers: BTreeMap<String, String>,
}

impl McpServerRecordToml {
    pub fn into_runtime(self, id: McpServerId) -> Result<McpServerRecord, String> {
        let enabled = self.enabled;
        let startup_policy = self.startup_policy;
        let trust_policy = self.trust_policy;
        let allowed_capabilities = self.allowed_capabilities;
        let roots_policy = self.roots_policy;
        let output_limits = self.output_limits;
        let auth_ref = self.auth_ref;
        let display_name = self.display_name.unwrap_or_else(|| id.0.clone());

        let has_command = self.command.is_some();
        let has_url = self.url.is_some();
        match (has_command, has_url) {
            (true, false) => {
                let command = self.command.unwrap_or_default();
                let mut argv = Vec::with_capacity(1 + self.args.len());
                argv.push(command);
                argv.extend(self.args);

                let transport = McpTransportConfig::Stdio {
                    command: argv,
                    cwd: self.cwd,
                    env: self.env,
                    env_vars: self.env_vars,
                };

                Ok(McpServerRecord {
                    id,
                    display_name,
                    transport,
                    startup_policy,
                    enabled,
                    trust_policy,
                    allowed_capabilities,
                    roots_policy,
                    output_limits,
                    auth_ref,
                })
            }
            (false, true) => {
                let url = self.url.unwrap();
                let transport = match self.transport_type {
                    Some(McpHttpTransportTypeToml::Sse) => McpTransportConfig::Sse {
                        url,
                        auth: None,
                        http_headers: self.http_headers,
                        env_http_headers: self.env_http_headers,
                    },
                    Some(McpHttpTransportTypeToml::Http)
                    | Some(McpHttpTransportTypeToml::StreamableHttp)
                    | None => McpTransportConfig::StreamableHttp {
                        url,
                        auth: None,
                        http_headers: self.http_headers,
                        env_http_headers: self.env_http_headers,
                    },
                };

                Ok(McpServerRecord {
                    id,
                    display_name,
                    transport,
                    startup_policy,
                    enabled,
                    trust_policy,
                    allowed_capabilities,
                    roots_policy,
                    output_limits,
                    auth_ref,
                })
            }
            (true, true) => {
                Err("invalid mcp server record: both `command` and `url` are set".to_string())
            }
            (false, false) => {
                Err("invalid mcp server record: neither `command` nor `url` is set".to_string())
            }
        }
    }
}

impl From<&McpServerRecord> for McpServerRecordToml {
    fn from(record: &McpServerRecord) -> Self {
        let enabled = record.enabled;
        let display_name = if record.display_name == record.id.0 {
            None
        } else {
            Some(record.display_name.clone())
        };
        let startup_policy = record.startup_policy.clone();
        let trust_policy = record.trust_policy;
        let allowed_capabilities = record.allowed_capabilities.clone();
        let roots_policy = record.roots_policy.clone();
        let output_limits = record.output_limits;
        let auth_ref = record.auth_ref.clone();

        match &record.transport {
            McpTransportConfig::Stdio {
                command,
                cwd,
                env,
                env_vars,
            } => {
                let cmd = command.first().cloned().unwrap_or_default();
                let args = command.iter().skip(1).cloned().collect::<Vec<_>>();
                Self {
                    enabled,
                    display_name,
                    startup_policy,
                    trust_policy,
                    allowed_capabilities,
                    roots_policy,
                    output_limits,
                    auth_ref,
                    command: Some(cmd.clone()),
                    args,
                    cwd: cwd.clone(),
                    env: env.clone(),
                    env_vars: env_vars.clone(),
                    url: None,
                    transport_type: None,
                    http_headers: BTreeMap::new(),
                    env_http_headers: BTreeMap::new(),
                }
            }
            McpTransportConfig::StreamableHttp {
                url,
                auth,
                http_headers,
                env_http_headers,
            } => {
                let mut headers = http_headers.clone();
                if let Some(McpAuthConfig::BearerToken { token }) = auth {
                    headers
                        .entry("Authorization".to_string())
                        .or_insert(format!("Bearer {token}"));
                }

                Self {
                    enabled,
                    display_name,
                    startup_policy,
                    trust_policy,
                    allowed_capabilities,
                    roots_policy,
                    output_limits,
                    auth_ref,
                    command: None,
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                    url: Some(url.clone()),
                    transport_type: None,
                    http_headers: headers,
                    env_http_headers: env_http_headers.clone(),
                }
            }
            McpTransportConfig::Sse {
                url,
                auth,
                http_headers,
                env_http_headers,
            } => {
                let mut headers = http_headers.clone();
                if let Some(McpAuthConfig::BearerToken { token }) = auth {
                    headers
                        .entry("Authorization".to_string())
                        .or_insert(format!("Bearer {token}"));
                }

                Self {
                    enabled,
                    display_name,
                    startup_policy,
                    trust_policy,
                    allowed_capabilities,
                    roots_policy,
                    output_limits,
                    auth_ref,
                    command: None,
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                    url: Some(url.clone()),
                    transport_type: Some(McpHttpTransportTypeToml::Sse),
                    http_headers: headers,
                    env_http_headers: env_http_headers.clone(),
                }
            }
        }
    }
}

impl McpStartupPolicy {
    fn is_default_lazy(policy: &Self) -> bool {
        matches!(policy, McpStartupPolicy::Lazy)
    }
}

impl McpTrustPolicy {
    fn is_default_user(policy: &Self) -> bool {
        matches!(policy, McpTrustPolicy::User)
    }
}

impl McpRootsPolicy {
    fn is_default_none(policy: &Self) -> bool {
        matches!(policy, McpRootsPolicy::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn default_has_no_bundled_servers() {
        assert_eq!(
            McpConfig::default(),
            McpConfig {
                servers: Vec::new(),
                auto_start: true
            }
        );
    }
}
