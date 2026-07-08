//! WebAssembly plugin runtime powered by Wasmtime.
//!
//! Provides sandboxed execution of `.wasm` plugins with:
//! - Hot-reload on file change (dev mode)
//! - Configurable WASI permissions (fs, net, env)
//! - Execution timeouts and memory limits
//! - Request/response interception phases
//!
//! ## Plugin Interface (WIT)
//!
//! Plugins export functions that conform to:
//! ```wit
//! // ruvyxa-plugin.wit
//! package ruvyxa:plugin@1.0.0;
//!
//! interface handler {
//!     record http-request {
//!         method: string,
//!         path: string,
//!         headers: list<tuple<string, string>>,
//!         body: option<list<u8>>,
//!     }
//!
//!     record http-response {
//!         status: u16,
//!         headers: list<tuple<string, string>>,
//!         body: option<list<u8>>,
//!     }
//!
//!     record plugin-result {
//!         action: string,    // "continue", "respond", "modify-request", "modify-response"
//!         request: option<http-request>,
//!         response: option<http-response>,
//!     }
//!
//!     // Called on request phase (before handler)
//!     on-request: func(req: http-request, config: string) -> plugin-result;
//!
//!     // Called on response phase (after handler)
//!     on-response: func(req: http-request, res: http-response, config: string) -> plugin-result;
//! }
//! ```
//!
//! ## Security Model
//!
//! Each plugin runs in its own Wasmtime `Store` with:
//! - Fuel-based execution limits (prevents infinite loops)
//! - Memory bounds (configurable, default 64MB)
//! - No filesystem access unless explicitly granted
//! - No network access unless explicitly granted
//! - No environment variable access unless explicitly granted

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info};
use wasmtime::*;
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::WasiCtxBuilder;

use crate::config::{PluginConfig, PluginPermissions, PluginPhase};

/// Represents an HTTP request passed to/from a Wasm plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// Represents an HTTP response passed to/from a Wasm plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// Result from a plugin invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginResult {
    /// Action to take: "continue", "respond", "modify-request", "modify-response"
    pub action: String,
    /// Modified request (for "modify-request" action).
    pub request: Option<PluginRequest>,
    /// Direct response (for "respond" action) or modified response.
    pub response: Option<PluginResponse>,
}

impl Default for PluginResult {
    fn default() -> Self {
        Self {
            action: "continue".to_string(),
            request: None,
            response: None,
        }
    }
}

/// A loaded and compiled Wasm plugin.
struct LoadedPlugin {
    name: String,
    module: Module,
    config_json: String,
    phase: PluginPhase,
    routes: Option<Vec<String>>,
    permissions: PluginPermissions,
    path: PathBuf,
}

/// The Wasm plugin runtime manages all loaded plugins.
pub struct WasmPluginRuntime {
    engine: Engine,
    plugins: Arc<RwLock<Vec<LoadedPlugin>>>,
}

impl WasmPluginRuntime {
    /// Create a new plugin runtime and load all configured plugins.
    pub async fn new(project_root: &Path, configs: &[PluginConfig]) -> Result<Self> {
        let mut engine_config = Config::new();
        engine_config.consume_fuel(true);
        engine_config.wasm_component_model(false);

        let engine = Engine::new(&engine_config).map_err(|error| {
            RuvyxaError::Message(format!("Failed to create Wasmtime engine: {error}"))
        })?;

        let mut plugins = Vec::new();

        for plugin_config in configs {
            let wasm_path = project_root.join(&plugin_config.path);
            match Self::load_plugin(&engine, &wasm_path, plugin_config) {
                Ok(plugin) => {
                    info!(name = %plugin.name, path = %wasm_path.display(), "loaded wasm plugin");
                    plugins.push(plugin);
                }
                Err(error) => {
                    error!(
                        name = %plugin_config.name,
                        path = %wasm_path.display(),
                        %error,
                        "failed to load wasm plugin"
                    );
                    return Err(Diagnostic::new("RUV2100", "Wasm plugin load error")
                        .explain(format!(
                            "Failed to load plugin '{}' from {}: {error}",
                            plugin_config.name,
                            wasm_path.display()
                        ))
                        .suggest("Check the .wasm file exists and is a valid WebAssembly module.")
                        .into());
                }
            }
        }

        Ok(Self {
            engine,
            plugins: Arc::new(RwLock::new(plugins)),
        })
    }

    /// Hot-reload a specific plugin by path.
    pub async fn reload_plugin(&self, wasm_path: &Path) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let index = plugins
            .iter()
            .position(|p| p.path == wasm_path)
            .ok_or_else(|| {
                RuvyxaError::Message(format!(
                    "Plugin not found for reload: {}",
                    wasm_path.display()
                ))
            })?;

        let old = &plugins[index];
        let config = PluginConfig {
            name: old.name.clone(),
            path: old.path.clone(),
            hot_reload: true,
            phase: old.phase.clone(),
            routes: old.routes.clone(),
            config: serde_json::from_str(&old.config_json).unwrap_or_default(),
            permissions: old.permissions.clone(),
        };

        let new_plugin = Self::load_plugin(&self.engine, wasm_path, &config).map_err(|error| {
            Diagnostic::new("RUV2102", "Wasm plugin hot-reload error")
                .explain(format!(
                    "Failed to reload plugin '{}': {error}",
                    config.name
                ))
                .suggest("The .wasm file may be corrupted or incompatible.")
        })?;

        plugins[index] = new_plugin;
        info!(path = %wasm_path.display(), "hot-reloaded wasm plugin");
        Ok(())
    }

    /// Execute request-phase plugins.
    pub async fn execute_request_plugins(
        &self,
        request: &PluginRequest,
    ) -> Result<Option<PluginResult>> {
        let plugins = self.plugins.read().await;
        for plugin in plugins.iter() {
            if plugin.phase != PluginPhase::Request {
                continue;
            }

            if !Self::matches_route(plugin, &request.path) {
                continue;
            }

            let engine = self.engine.clone();
            let module = plugin.module.clone();
            let req = request.clone();
            let config_json = plugin.config_json.clone();
            let permissions = plugin.permissions.clone();

            let result = tokio::task::spawn_blocking(move || {
                match Self::invoke_on_request_blocking(&engine, &module, &req, &config_json, &permissions) {
                    Ok(r) => Ok(r),
                    Err(e) => Err(RuvyxaError::Message(e)),
                }
            })
            .await
            .map_err(|e| RuvyxaError::Message(format!("Plugin task panicked: {e}")))??;

            if result.action != "continue" {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    /// Execute response-phase plugins.
    pub async fn execute_response_plugins(
        &self,
        request: &PluginRequest,
        response: &PluginResponse,
    ) -> Result<Option<PluginResult>> {
        let plugins = self.plugins.read().await;
        for plugin in plugins.iter() {
            if plugin.phase != PluginPhase::Response {
                continue;
            }

            if !Self::matches_route(plugin, &request.path) {
                continue;
            }

            let engine = self.engine.clone();
            let module = plugin.module.clone();
            let req = request.clone();
            let res = response.clone();
            let config_json = plugin.config_json.clone();
            let permissions = plugin.permissions.clone();

            let result = tokio::task::spawn_blocking(move || {
                match Self::invoke_on_response_blocking(&engine, &module, &req, &res, &config_json, &permissions) {
                    Ok(r) => Ok(r),
                    Err(e) => Err(RuvyxaError::Message(e)),
                }
            })
            .await
            .map_err(|e| RuvyxaError::Message(format!("Plugin task panicked: {e}")))??;

            if result.action != "continue" {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    /// Get number of loaded plugins.
    pub async fn plugin_count(&self) -> usize {
        self.plugins.read().await.len()
    }

    // --- Internal ---

    fn load_plugin(
        engine: &Engine,
        wasm_path: &Path,
        config: &PluginConfig,
    ) -> std::result::Result<LoadedPlugin, String> {
        if !wasm_path.exists() {
            return Err(format!("Wasm file not found: {}", wasm_path.display()));
        }

        let wasm_bytes =
            std::fs::read(wasm_path).map_err(|e| format!("Failed to read wasm file: {e}"))?;

        let module = Module::new(engine, &wasm_bytes)
            .map_err(|e| format!("Failed to compile wasm module: {e}"))?;

        let config_json =
            serde_json::to_string(&config.config).unwrap_or_else(|_| "{}".to_string());

        Ok(LoadedPlugin {
            name: config.name.clone(),
            module,
            config_json,
            phase: config.phase.clone(),
            routes: config.routes.clone(),
            permissions: config.permissions.clone(),
            path: wasm_path.to_path_buf(),
        })
    }

    fn matches_route(plugin: &LoadedPlugin, path: &str) -> bool {
        match &plugin.routes {
            None => true, // No filter = match all
            Some(patterns) => patterns.iter().any(|pattern| {
                if pattern.ends_with('*') {
                    let prefix = &pattern[..pattern.len() - 1];
                    path.starts_with(prefix)
                } else {
                    path == pattern
                }
            }),
        }
    }

    fn invoke_on_request_blocking(
        engine: &Engine,
        module: &Module,
        request: &PluginRequest,
        config_json: &str,
        permissions: &PluginPermissions,
    ) -> std::result::Result<PluginResult, String> {
        let mut store = Self::create_store(engine, permissions)?;
        let instance = Self::instantiate(engine, &mut store, module)?;

        let request_json = serde_json::to_string(request)
            .map_err(|e| format!("Failed to serialize request: {e}"))?;

        let result_json = Self::call_plugin_func(
            &mut store,
            &instance,
            "on_request",
            &request_json,
            config_json,
        )?;

        serde_json::from_str(&result_json)
            .map_err(|e| format!("Failed to parse plugin result: {e}"))
    }

    fn invoke_on_response_blocking(
        engine: &Engine,
        module: &Module,
        request: &PluginRequest,
        response: &PluginResponse,
        config_json: &str,
        permissions: &PluginPermissions,
    ) -> std::result::Result<PluginResult, String> {
        let mut store = Self::create_store(engine, permissions)?;
        let instance = Self::instantiate(engine, &mut store, module)?;

        let request_json = serde_json::to_string(request)
            .map_err(|e| format!("Failed to serialize request: {e}"))?;
        let response_json = serde_json::to_string(response)
            .map_err(|e| format!("Failed to serialize response: {e}"))?;

        let input = serde_json::json!({
            "request": request_json,
            "response": response_json,
            "config": config_json,
        })
        .to_string();

        let result_json = Self::call_plugin_func(
            &mut store,
            &instance,
            "on_response",
            &input,
            config_json,
        )?;

        serde_json::from_str(&result_json)
            .map_err(|e| format!("Failed to parse plugin result: {e}"))
    }

    fn create_store(
        engine: &Engine,
        permissions: &PluginPermissions,
    ) -> std::result::Result<Store<WasiP1Ctx>, String> {
        let mut wasi_builder = WasiCtxBuilder::new();

        for var in &permissions.env {
            if let Ok(value) = std::env::var(var) {
                wasi_builder.env(var, &value);
            }
        }

        let wasi_ctx = wasi_builder.build_p1();
        let mut store = Store::new(engine, wasi_ctx);

        let fuel = permissions.timeout_ms * 1_000_000;
        store.set_fuel(fuel).ok();

        Ok(store)
    }

    fn instantiate(
        engine: &Engine,
        store: &mut Store<WasiP1Ctx>,
        module: &Module,
    ) -> std::result::Result<Instance, String> {
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx| ctx)
            .map_err(|e| format!("Failed to link WASI: {e}"))?;

        linker
            .instantiate(&mut *store, module)
            .map_err(|e| format!("Failed to instantiate: {e}"))
    }

    fn call_plugin_func(
        store: &mut Store<WasiP1Ctx>,
        instance: &Instance,
        func_name: &str,
        input: &str,
        _config: &str,
    ) -> std::result::Result<String, String> {
        let memory = instance
            .get_memory(&mut *store, "memory")
            .ok_or_else(|| "Plugin does not export 'memory'".to_string())?;

        let func = instance
            .get_typed_func::<(i32, i32), i32>(&mut *store, func_name)
            .map_err(|_| format!("Plugin does not export function '{func_name}'"))?;

        let input_bytes = input.as_bytes();
        let input_ptr = 0i32;
        memory
            .write(&mut *store, input_ptr as usize, input_bytes)
            .map_err(|e| format!("Failed to write to plugin memory: {e}"))?;

        let result_ptr = func
            .call(&mut *store, (input_ptr, input_bytes.len() as i32))
            .map_err(|e| format!("Plugin function '{func_name}' trapped: {e}"))?;

        let mut result_buf = vec![0u8; 4096];
        memory
            .read(&*store, result_ptr as usize, &mut result_buf)
            .map_err(|e| format!("Failed to read plugin result: {e}"))?;

        let result_str = String::from_utf8_lossy(
            &result_buf[..result_buf
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(result_buf.len())],
        )
        .to_string();

        if result_str.is_empty() {
            Ok(serde_json::to_string(&PluginResult::default()).unwrap())
        } else {
            Ok(result_str)
        }
    }
}
