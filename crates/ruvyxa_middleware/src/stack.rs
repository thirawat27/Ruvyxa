//! Middleware stack builder.
//!
//! Compiles a `MiddlewareConfig` into an axum-compatible layer stack
//! that can be applied to a Router.

use axum::Router;
use tower_http::compression::CompressionLayer;
use tracing::info;

use crate::builtin::{CorsLayer, CustomHeadersLayer, RequestLoggingLayer, TimingLayer};
use crate::config::MiddlewareConfig;

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
    /// 3. Timing (X-Response-Time header)
    /// 4. Request Logging
    /// 5. Custom Headers
    /// 6. Wasm Plugin layers (if any)
    pub fn apply<S: Clone + Send + Sync + 'static>(&self, router: Router<S>) -> Router<S> {
        let mut app = router;

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

        // Apply CORS
        if let Some(ref cors_config) = self.config.builtin.cors {
            app = app.layer(CorsLayer::from_config(cors_config));
            info!(
                origins = cors_config.origins.len(),
                "CORS middleware enabled"
            );
        }

        // Compression is always applied (outermost)
        app = app.layer(CompressionLayer::new());

        info!(
            builtin_layers = self.count_builtin_layers(),
            plugins = self.config.plugins.len(),
            "middleware stack applied"
        );

        app
    }

    fn count_builtin_layers(&self) -> usize {
        let mut count = 1; // compression always on
        if self.config.builtin.cors.is_some() {
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
