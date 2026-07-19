//! Middleware configuration types.
//!
//! Deserialized from `ruvyxa.config.ts` via the config renderer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Top-level middleware configuration block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MiddlewareConfig {
    /// Built-in middleware to enable.
    #[serde(default)]
    pub builtin: BuiltinMiddlewareConfig,
}

/// Built-in middleware toggles and config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinMiddlewareConfig {
    /// Enable CORS middleware.
    #[serde(default)]
    pub cors: Option<CorsConfig>,

    /// Enable request/response timing headers.
    #[serde(default = "default_true")]
    pub timing: bool,

    /// Enable request logging.
    #[serde(default = "default_true")]
    #[serde(rename = "log")]
    pub logging: bool,

    /// Rate limiting configuration.
    #[serde(default)]
    #[serde(rename = "rate")]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimitConfig {
    /// Maximum requests per window.
    #[serde(rename = "max")]
    pub max_requests: usize,

    /// Window duration in seconds.
    #[serde(rename = "window")]
    pub window_secs: u64,

    /// Key extraction: "ip", "header:X-Api-Key", etc.
    #[serde(default = "default_rate_key")]
    #[serde(rename = "key")]
    pub key_by: String,
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
