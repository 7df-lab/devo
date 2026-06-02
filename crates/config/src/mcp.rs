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
    /// Whether config reload should refresh running server catalogs.
    #[serde(default = "default_mcp_refresh_on_config_reload")]
    pub refresh_on_config_reload: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            auto_start: true,
            refresh_on_config_reload: true,
        }
    }
}

fn default_mcp_auto_start() -> bool {
    true
}

fn default_mcp_refresh_on_config_reload() -> bool {
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

/// Strongly typed identifier for one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpServerId(pub String);

impl std::fmt::Display for McpServerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Describes how the runtime connects to an MCP server.
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
