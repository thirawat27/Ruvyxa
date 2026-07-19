//! Middleware stack builder.
//!
//! Compiles a `MiddlewareConfig` into an axum-compatible layer stack
//! that can be applied to a Router.

use axum::{Router, body::HttpBody};
use tower_http::compression::{
    CompressionLayer,
    predicate::{DefaultPredicate, Predicate},
};
use tracing::{info, warn};

use crate::builtin::{
    CorsLayer, CustomHeadersLayer, RateLimitLayerWithKey, RequestLoggingLayer, TimingLayer,
};
use crate::config::MiddlewareConfig;

/// Compress only response bodies whose complete size is already known.
///
/// Axum bodies backed by a live stream have no exact size hint. Running those
/// bodies through the asynchronous compression adapter can terminate the
/// encoded body before the HTTP/1 chunked response is finalized, which clients
/// report as an incomplete chunked encoding. Buffered responses keep the normal
/// tower-http content-type and minimum-size compression rules.
#[derive(Clone, Default)]
struct CompleteBodyCompressionPredicate {
    default: DefaultPredicate,
}

impl Predicate for CompleteBodyCompressionPredicate {
    fn should_compress<B>(&self, response: &axum::http::Response<B>) -> bool
    where
        B: HttpBody,
    {
        response.body().size_hint().exact().is_some() && self.default.should_compress(response)
    }
}

/// A compiled middleware stack ready to be applied to an axum Router.
#[derive(Default)]
pub struct MiddlewareStack {
    config: MiddlewareConfig,
}

impl MiddlewareStack {
    /// Create a new middleware stack from configuration.
    pub fn new(config: MiddlewareConfig) -> Self {
        Self { config }
    }

    /// Apply the middleware stack to an axum Router.
    ///
    /// Layers are applied in this order (outermost first):
    /// 1. Compression (gzip + brotli)
    /// 2. CORS (if configured)
    /// 3. Rate Limiting (if configured)
    /// 4. Timing (X-Response-Time header)
    /// 5. Request Logging
    /// 6. Custom Headers
    /// 7. Custom layers (validated and applied if supported)
    /// 8. Wasm Plugin layers (if any, via the wasm runtime)
    pub fn apply<S: Clone + Send + Sync + 'static>(&self, router: Router<S>) -> Router<S> {
        let mut app = router;

        // Validate config — log warnings for issues that don't warrant a hard failure
        // in dev mode but should be addressed.
        if let Err(reason) = self.validate() {
            warn!(%reason, "middleware configuration issue detected");
        }

        // Warn about plugins that need the wasm runtime
        if !self.config.plugins.is_empty() {
            #[cfg(not(feature = "wasm-plugins"))]
            {
                warn!(
                    plugins = self.config.plugins.len(),
                    "wasm plugins configured but the 'wasm-plugins' feature is not enabled; \
                     plugins will not be applied"
                );
            }
            #[cfg(feature = "wasm-plugins")]
            {
                info!(
                    plugins = self.config.plugins.len(),
                    "wasm plugins configured — they will be applied by the plugin runtime"
                );
            }
        }

        // Apply custom headers if any
        if !self.config.builtin.headers.is_empty() {
            app = app.layer(CustomHeadersLayer::new(&self.config.builtin.headers));
            info!(
                count = self.config.builtin.headers.len(),
                "custom response headers configured"
            );
        }

        // Apply request logging
        if self.config.builtin.logging {
            app = app.layer(RequestLoggingLayer);
        }

        // Apply timing
        if self.config.builtin.timing {
            app = app.layer(TimingLayer);
        }

        // Apply rate limiting
        if let Some(ref rate_config) = self.config.builtin.rate_limit {
            app = app.layer(RateLimitLayerWithKey::from_config(rate_config));
            info!(
                max = rate_config.max_requests,
                window_secs = rate_config.window_secs,
                key = %rate_config.key_by,
                "rate limiting enabled"
            );
        }

        // Apply CORS
        if let Some(ref cors_config) = self.config.builtin.cors {
            app = app.layer(CorsLayer::from_config(cors_config));
            info!(
                origins = cors_config.origins.len(),
                "CORS middleware enabled"
            );
        }

        // Compression is always applied to complete, sized bodies (outermost).
        // Unknown-size bodies are live streams and must reach HTTP framing
        // without an asynchronous compression adapter in between.
        app = app.layer(
            CompressionLayer::new().compress_when(CompleteBodyCompressionPredicate::default()),
        );

        info!(
            builtin_layers = self.count_builtin_layers(),
            custom_layers = self.config.layers.len(),
            plugins = self.config.plugins.len(),
            "middleware stack applied"
        );

        app
    }

    /// Validate the middleware configuration before applying it.
    ///
    /// Returns an error if unsupported features are configured that would create
    /// a false sense of security (e.g. plugins configured without the wasm feature,
    /// or custom layers that are not recognized).
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Reject custom layers — none are currently supported
        if !self.config.layers.is_empty() {
            let kinds: Vec<&str> = self.config.layers.iter().map(|l| l.kind.as_str()).collect();
            return Err(format!(
                "Custom middleware layers are not yet supported. \
                 Remove or comment out these layers from your config: {:?}",
                kinds,
            ));
        }

        for (name, value) in &self.config.builtin.headers {
            if axum::http::HeaderName::from_bytes(name.as_bytes()).is_err()
                || axum::http::HeaderValue::from_str(value).is_err()
            {
                return Err(format!("Invalid custom response header '{name}'"));
            }
        }

        if let Some(cors) = &self.config.builtin.cors {
            if cors.credentials && cors.origins.iter().any(|origin| origin == "*") {
                return Err(
                    "CORS credentials cannot be enabled with the wildcard origin '*'; use an explicit origin allowlist"
                        .to_string(),
                );
            }
            for method in &cors.methods {
                if axum::http::Method::from_bytes(method.as_bytes()).is_err() {
                    return Err(format!("Invalid CORS method '{method}'"));
                }
            }
            for allowed_header in &cors.headers {
                if axum::http::HeaderName::from_bytes(allowed_header.as_bytes()).is_err() {
                    return Err(format!("Invalid CORS header '{allowed_header}'"));
                }
            }
        }

        if let Some(rate) = &self.config.builtin.rate_limit {
            if rate.max_requests == 0 {
                return Err("Rate limit 'max' must be greater than 0".to_string());
            }
            if rate.window_secs == 0 {
                return Err("Rate limit 'window' must be greater than 0".to_string());
            }
            if rate.key_by == "ip" {
                // The transport peer is the only implicit key source. Forwarded
                // client identity remains opt-in through an explicit header.
            } else if let Some(header_name) = rate.key_by.strip_prefix("header:") {
                if header_name.is_empty()
                    || axum::http::HeaderName::from_bytes(header_name.as_bytes()).is_err()
                {
                    return Err(format!(
                        "Rate limit 'key' must be 'ip' or 'header:<valid-header-name>', got '{}'",
                        rate.key_by
                    ));
                }
            } else {
                return Err(format!(
                    "Rate limit 'key' must be 'ip' or 'header:<valid-header-name>', got '{}'",
                    rate.key_by
                ));
            }
        }

        // Reject plugins when wasm feature is disabled
        #[cfg(not(feature = "wasm-plugins"))]
        if !self.config.plugins.is_empty() {
            return Err(format!(
                "{} wasm plugin(s) configured but the 'wasm-plugins' feature is not enabled. \
                 Either enable the feature or remove plugin config to avoid false security.",
                self.config.plugins.len(),
            ));
        }

        // Validate plugin permissions are within supported bounds
        #[cfg(feature = "wasm-plugins")]
        for plugin in &self.config.plugins {
            if plugin.permissions.timeout_ms == 0 {
                return Err(format!(
                    "Plugin '{}' has timeout_ms set to 0, which would block indefinitely. \
                     Set a positive timeout value.",
                    plugin.name,
                ));
            }
            if plugin.permissions.max_memory_bytes == 0 {
                return Err(format!(
                    "Plugin '{}' has max memory set to 0. Set a positive memory limit.",
                    plugin.name,
                ));
            }
            if !plugin.permissions.fs_read.is_empty() || !plugin.permissions.net.is_empty() {
                return Err(format!(
                    "Plugin '{}' requests filesystem or network permissions, which this runtime does not expose yet.",
                    plugin.name
                ));
            }
        }

        Ok(())
    }

    /// Access the plugin configs for initializing the wasm runtime externally.
    pub fn plugin_configs(&self) -> &[crate::config::PluginConfig] {
        &self.config.plugins
    }

    fn count_builtin_layers(&self) -> usize {
        let mut count = 1; // compression always on
        if self.config.builtin.cors.is_some() {
            count += 1;
        }
        if self.config.builtin.rate_limit.is_some() {
            count += 1;
        }
        if self.config.builtin.timing {
            count += 1;
        }
        if self.config.builtin.logging {
            count += 1;
        }
        if !self.config.builtin.headers.is_empty() {
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LayerConfig, RateLimitConfig};
    use axum::{
        body::{Body, Bytes, to_bytes},
        http::{Request, Response, header},
        routing::get,
    };
    use futures_core::Stream;
    use std::{
        convert::Infallible,
        pin::Pin,
        task::{Context, Poll},
    };
    use tower::ServiceExt;

    struct OneChunk(Option<Bytes>);

    impl Stream for OneChunk {
        type Item = Result<Bytes, Infallible>;

        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.0.take().map(Ok))
        }
    }

    async fn streamed_response() -> Response<Body> {
        Response::new(Body::from_stream(OneChunk(Some(Bytes::from_static(
            b"streamed response that is deliberately larger than thirty-two bytes",
        )))))
    }

    async fn buffered_response() -> &'static str {
        "buffered response that is deliberately larger than thirty-two bytes"
    }

    #[test]
    fn rejects_unsupported_custom_layers_before_server_startup() {
        let mut config = MiddlewareConfig::default();
        config.layers.push(LayerConfig {
            kind: "auth".to_string(),
            options: serde_json::Value::Null,
        });

        assert!(MiddlewareStack::new(config).validate().is_err());
    }

    #[test]
    fn rejects_rate_limits_that_could_disable_protection() {
        for (max_requests, window_secs) in [(0, 60), (10, 0)] {
            let mut config = MiddlewareConfig::default();
            config.builtin.rate_limit = Some(RateLimitConfig {
                max_requests,
                window_secs,
                key_by: "ip".to_string(),
            });

            assert!(MiddlewareStack::new(config).validate().is_err());
        }
    }

    #[test]
    fn rejects_unknown_rate_limit_key_selectors() {
        for key_by in ["forwarded", "header:", "header:invalid header"] {
            let mut config = MiddlewareConfig::default();
            config.builtin.rate_limit = Some(RateLimitConfig {
                max_requests: 10,
                window_secs: 60,
                key_by: key_by.to_string(),
            });

            assert!(MiddlewareStack::new(config).validate().is_err(), "{key_by}");
        }
    }

    #[test]
    fn accepts_ip_and_header_rate_limit_keys() {
        for key_by in ["ip", "header:x-api-key"] {
            let mut config = MiddlewareConfig::default();
            config.builtin.rate_limit = Some(RateLimitConfig {
                max_requests: 10,
                window_secs: 60,
                key_by: key_by.to_string(),
            });

            assert!(MiddlewareStack::new(config).validate().is_ok(), "{key_by}");
        }
    }

    #[test]
    fn rejects_credentialed_wildcard_cors_and_invalid_allowlists() {
        let mut config = MiddlewareConfig::default();
        config.builtin.cors = Some(crate::config::CorsConfig {
            origins: vec!["*".to_string()],
            methods: vec!["POST".to_string()],
            headers: Vec::new(),
            credentials: true,
            max_age: 60,
        });
        assert!(MiddlewareStack::new(config).validate().is_err());

        let mut config = MiddlewareConfig::default();
        config.builtin.cors = Some(crate::config::CorsConfig {
            origins: vec!["https://app.example".to_string()],
            methods: vec!["NOT A METHOD".to_string()],
            headers: Vec::new(),
            credentials: false,
            max_age: 60,
        });
        assert!(MiddlewareStack::new(config).validate().is_err());
    }

    #[tokio::test]
    async fn leaves_unknown_size_streams_uncompressed_and_complete() {
        let app = MiddlewareStack::new(MiddlewareConfig::default())
            .apply(Router::new().route("/stream", get(streamed_response)));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/stream")
                    .header(header::ACCEPT_ENCODING, "gzip, br")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            body,
            &b"streamed response that is deliberately larger than thirty-two bytes"[..]
        );
    }

    #[tokio::test]
    async fn still_compresses_complete_sized_responses() {
        let app = MiddlewareStack::new(MiddlewareConfig::default())
            .apply(Router::new().route("/buffered", get(buffered_response)));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/buffered")
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
        assert!(
            !to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
