//! Middleware configuration types.
//!
//! Deserialized from `ruvyxa.config.ts` via the config renderer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Largest configurable TypeScript plugin middleware worker pool.
pub const MAX_PLUGIN_MIDDLEWARE_WORKERS: usize = 8;

/// Default upper bound for one TypeScript plugin middleware hook.
pub const DEFAULT_PLUGIN_HOOK_TIMEOUT_MS: u64 = 30_000;

/// Largest configurable hook timeout, preventing accidental effectively
/// unbounded recovery windows.
pub const MAX_PLUGIN_HOOK_TIMEOUT_MS: u64 = 300_000;

/// Top-level middleware configuration block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MiddlewareConfig {
    /// Built-in middleware to enable.
    #[serde(default)]
    pub builtin: BuiltinMiddlewareConfig,

    /// TypeScript plugin middleware worker processes. Workers do not share
    /// module-level plugin state, so the default stays at one process.
    #[serde(default)]
    pub workers: Option<usize>,

    /// Maximum duration of one TypeScript plugin middleware hook.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl MiddlewareConfig {
    /// Validated plugin middleware pool size.
    pub fn plugin_workers(&self) -> Result<usize, String> {
        match self.workers {
            None => Ok(1),
            Some(workers) if (1..=MAX_PLUGIN_MIDDLEWARE_WORKERS).contains(&workers) => Ok(workers),
            Some(workers) => Err(format!(
                "RUV1602 config field `middleware.workers` must be between 1 and \
                 {MAX_PLUGIN_MIDDLEWARE_WORKERS}, got {workers}"
            )),
        }
    }

    /// Validated timeout for one plugin middleware hook.
    pub fn plugin_timeout(&self) -> Result<Duration, String> {
        match self.timeout_ms {
            None => Ok(Duration::from_millis(DEFAULT_PLUGIN_HOOK_TIMEOUT_MS)),
            Some(timeout_ms) if (1..=MAX_PLUGIN_HOOK_TIMEOUT_MS).contains(&timeout_ms) => {
                Ok(Duration::from_millis(timeout_ms))
            }
            Some(timeout_ms) => Err(format!(
                "RUV1602 config field `middleware.timeoutMs` must be between 1 and \
                 {MAX_PLUGIN_HOOK_TIMEOUT_MS}, got {timeout_ms}"
            )),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_workers_defaults_to_one_and_rejects_out_of_range_values() {
        let config = MiddlewareConfig::default();
        assert_eq!(config.plugin_workers(), Ok(1));

        let config: MiddlewareConfig =
            serde_json::from_value(serde_json::json!({ "workers": 4 })).unwrap();
        assert_eq!(config.plugin_workers(), Ok(4));

        for workers in [0usize, MAX_PLUGIN_MIDDLEWARE_WORKERS + 1] {
            let config: MiddlewareConfig =
                serde_json::from_value(serde_json::json!({ "workers": workers })).unwrap();
            let error = config.plugin_workers().unwrap_err();
            assert!(error.contains("middleware.workers"), "{error}");
        }
    }

    #[test]
    fn plugin_timeout_defaults_to_thirty_seconds_and_rejects_out_of_range_values() {
        let config = MiddlewareConfig::default();
        assert_eq!(
            config.plugin_timeout(),
            Ok(Duration::from_millis(DEFAULT_PLUGIN_HOOK_TIMEOUT_MS))
        );

        let config: MiddlewareConfig =
            serde_json::from_value(serde_json::json!({ "timeoutMs": 15_000 })).unwrap();
        assert_eq!(config.plugin_timeout(), Ok(Duration::from_millis(15_000)));

        for timeout_ms in [0_u64, MAX_PLUGIN_HOOK_TIMEOUT_MS + 1] {
            let config: MiddlewareConfig =
                serde_json::from_value(serde_json::json!({ "timeoutMs": timeout_ms })).unwrap();
            let error = config.plugin_timeout().unwrap_err();
            assert!(error.contains("middleware.timeoutMs"), "{error}");
        }
    }
}
