//! Middleware configuration types.
//!
//! Deserialized from `ruvyxa.config.ts` via the config renderer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Top-level middleware configuration block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiddlewareConfig {
    /// Built-in middleware to enable.
    #[serde(default)]
    pub builtin: BuiltinMiddlewareConfig,

    /// Custom middleware layers (order matters — applied top to bottom).
    #[serde(default)]
    pub layers: Vec<LayerConfig>,

    /// Wasm plugin configuration.
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
}

/// Built-in middleware toggles and config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinMiddlewareConfig {
    /// Enable CORS middleware.
    #[serde(default)]
    pub cors: Option<CorsConfig>,

    /// Enable request/response timing headers.
    #[serde(default = "default_true")]
    pub timing: bool,

    /// Enable request logging.
    #[serde(default = "default_true")]
    pub logging: bool,

    /// Rate limiting configuration.
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,

    /// Custom response headers applied to all responses.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl Default for BuiltinMiddlewareConfig {
    fn default() -> Self {
        Self {
            cors: None,
            timing: true,
            logging: true,
            rate_limit: None,
            headers: BTreeMap::new(),
        }
    }
}

/// CORS configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorsConfig {
    /// Allowed origins. Use `["*"]` for permissive.
    #[serde(default)]
    pub origins: Vec<String>,

    /// Allowed methods.
    #[serde(default = "default_cors_methods")]
    pub methods: Vec<String>,

    /// Allowed headers.
    #[serde(default)]
    pub headers: Vec<String>,

    /// Whether to allow credentials.
    #[serde(default)]
    pub credentials: bool,

    /// Max age for preflight cache (seconds).
    #[serde(default = "default_cors_max_age")]
    pub max_age: u64,
}

/// Rate limiting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitConfig {
    /// Maximum requests per window.
    pub max_requests: usize,

    /// Window duration in seconds.
    pub window_secs: u64,

    /// Key extraction: "ip", "header:X-Api-Key", etc.
    #[serde(default = "default_rate_key")]
    pub key_by: String,
}

/// Custom Tower layer configuration (for advanced users).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerConfig {
    /// Layer type identifier.
    pub kind: String,

    /// Arbitrary JSON options for the layer.
    #[serde(default)]
    pub options: serde_json::Value,
}

/// Wasm plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfig {
    /// Plugin name (used for logging and diagnostics).
    pub name: String,

    /// Path to the `.wasm` module (relative to project root).
    pub path: PathBuf,

    /// Whether to enable hot-reload for this plugin.
    #[serde(default = "default_true")]
    pub hot_reload: bool,

    /// Execution phase: "request" (before handler) or "response" (after handler).
    #[serde(default = "default_phase")]
    pub phase: PluginPhase,

    /// Route pattern filter (only apply to matching routes).
    #[serde(default)]
    pub routes: Option<Vec<String>>,

    /// Plugin-specific configuration passed as JSON to the wasm module.
    #[serde(default)]
    pub config: serde_json::Value,

    /// WASI permissions granted to the plugin.
    #[serde(default)]
    pub permissions: PluginPermissions,
}

/// When a plugin executes in the request lifecycle.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginPhase {
    /// Execute before the route handler.
    #[default]
    Request,
    /// Execute after the route handler (can modify response).
    Response,
}

/// WASI permissions for sandboxed plugin execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPermissions {
    /// Allow environment variable access (specific vars only).
    #[serde(default)]
    pub env: Vec<String>,

    /// Allow filesystem read access to specific directories.
    #[serde(default)]
    pub fs_read: Vec<PathBuf>,

    /// Allow network access to specific hosts.
    #[serde(default)]
    pub net: Vec<String>,

    /// Maximum execution time in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Maximum memory usage in bytes.
    #[serde(default = "default_max_memory")]
    pub max_memory_bytes: u64,
}

fn default_true() -> bool {
    true
}

fn default_cors_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "POST".to_string(),
        "PUT".to_string(),
        "DELETE".to_string(),
        "OPTIONS".to_string(),
    ]
}

fn default_cors_max_age() -> u64 {
    86400
}

fn default_rate_key() -> String {
    "ip".to_string()
}

fn default_phase() -> PluginPhase {
    PluginPhase::Request
}

fn default_timeout_ms() -> u64 {
    5000
}

fn default_max_memory() -> u64 {
    64 * 1024 * 1024 // 64MB
}
