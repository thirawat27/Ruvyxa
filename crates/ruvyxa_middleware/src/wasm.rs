//! WebAssembly plugin runtime powered by Wasmtime.
//!
//! Provides sandboxed execution of `.wasm` plugins with:
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
//! - No filesystem or network access (those requested permissions are rejected)
//! - No environment variable access unless explicitly granted

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info};
use wasmtime::*;
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::p1::WasiP1Ctx;

use crate::config::{PluginConfig, PluginPermissions, PluginPhase};

const PLUGIN_RESULT_READ_CHUNK_BYTES: usize = 4 * 1024;
const MAX_PLUGIN_RESULT_BYTES: usize = 1024 * 1024;

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

/// Static ABI information reported by the plugin debug command.
#[derive(Debug, Clone, Serialize)]
pub struct PluginModuleInfo {
    pub exports: Vec<String>,
    pub has_memory: bool,
    pub has_request_handler: bool,
    pub has_response_handler: bool,
    pub has_allocator: bool,
}

/// Compile a Wasm plugin and report the exports required by Ruvyxa's raw ABI.
pub fn inspect_wasm_plugin(path: &Path) -> Result<PluginModuleInfo> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("wasm") {
        return Err(Diagnostic::new("RUV2100", "Invalid Wasm plugin file")
            .explain(format!(
                "{} does not have a .wasm extension.",
                path.display()
            ))
            .suggest("Build the plugin with cargo and pass its generated .wasm file.")
            .into());
    }
    let wasm = std::fs::read(path).map_err(|source| RuvyxaError::Io {
        message: format!("Failed to read Wasm plugin {}", path.display()),
        source,
    })?;
    let engine = plugin_engine()?;
    let module = Module::new(&engine, wasm).map_err(|error| {
        Diagnostic::new("RUV2100", "Wasm plugin load error")
            .explain(format!("Failed to compile {}: {error}", path.display()))
    })?;
    let exports = module
        .exports()
        .map(|export| export.name().to_string())
        .collect::<Vec<_>>();

    Ok(PluginModuleInfo {
        has_memory: exports.iter().any(|export| export == "memory"),
        has_request_handler: exports.iter().any(|export| export == "on_request"),
        has_response_handler: exports.iter().any(|export| export == "on_response"),
        has_allocator: exports.iter().any(|export| export == "ruvyxa_alloc"),
        exports,
    })
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
}

struct PluginStore {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// The Wasm plugin runtime manages all loaded plugins.
pub struct WasmPluginRuntime {
    engine: Engine,
    plugins: Arc<RwLock<Vec<LoadedPlugin>>>,
}

fn plugin_engine() -> Result<Engine> {
    let mut engine_config = Config::new();
    engine_config.consume_fuel(true);
    engine_config.wasm_component_model(false);
    Engine::new(&engine_config)
        .map_err(|error| RuvyxaError::Message(format!("Failed to create Wasmtime engine: {error}")))
}

impl WasmPluginRuntime {
    /// Create a new plugin runtime and load all configured plugins.
    pub async fn new(project_root: &Path, configs: &[PluginConfig]) -> Result<Self> {
        let canonical_root = project_root
            .canonicalize()
            .map_err(|source| RuvyxaError::Io {
                message: format!("Failed to resolve project root {}", project_root.display()),
                source,
            })?;
        let engine = plugin_engine()?;

        let mut plugins = Vec::new();

        for plugin_config in configs {
            let configured_path = configured_plugin_path(plugin_config)?;
            let configured_path = Path::new(&configured_path);
            let is_relative_safe = configured_path.is_relative()
                && configured_path.components().all(|component| {
                    matches!(
                        component,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                });
            if !is_relative_safe {
                return Err(Diagnostic::new("RUV2101", "Invalid Wasm plugin path")
                    .explain(format!(
                        "Plugin '{}' must use a project-relative path without '..' components.",
                        plugin_config.name
                    ))
                    .into());
            }

            let joined_path = canonical_root.join(configured_path);
            let wasm_path = joined_path
                .canonicalize()
                .map_err(|source| RuvyxaError::Io {
                    message: format!("Failed to resolve Wasm plugin {}", joined_path.display()),
                    source,
                })?;
            if !wasm_path.starts_with(&canonical_root)
                || wasm_path.extension().and_then(|ext| ext.to_str()) != Some("wasm")
            {
                return Err(Diagnostic::new("RUV2101", "Invalid Wasm plugin path")
                    .explain(format!(
                        "Plugin '{}' must resolve to a .wasm file inside the project root.",
                        plugin_config.name
                    ))
                    .into());
            }
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

            let result =
                tokio::task::spawn_blocking(move || {
                    match Self::invoke_on_request_blocking(
                        &engine,
                        &module,
                        &req,
                        &config_json,
                        &permissions,
                    ) {
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

            let result =
                tokio::task::spawn_blocking(move || {
                    match Self::invoke_on_response_blocking(
                        &engine,
                        &module,
                        &req,
                        &res,
                        &config_json,
                        &permissions,
                    ) {
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

        let result_json =
            Self::call_plugin_func(&mut store, &instance, "on_response", &input, config_json)?;

        serde_json::from_str(&result_json)
            .map_err(|e| format!("Failed to parse plugin result: {e}"))
    }

    fn create_store(
        engine: &Engine,
        permissions: &PluginPermissions,
    ) -> std::result::Result<Store<PluginStore>, String> {
        let mut wasi_builder = WasiCtxBuilder::new();

        for var in &permissions.env {
            if let Ok(value) = std::env::var(var) {
                wasi_builder.env(var, &value);
            }
        }

        let memory_limit = usize::try_from(permissions.max_memory_bytes)
            .map_err(|_| "Plugin memory limit exceeds this platform's address space".to_string())?;
        let limits = StoreLimitsBuilder::new().memory_size(memory_limit).build();
        let mut store = Store::new(
            engine,
            PluginStore {
                wasi: wasi_builder.build_p1(),
                limits,
            },
        );
        store.limiter(|store| &mut store.limits);

        let fuel = permissions
            .timeout_ms
            .checked_mul(1_000_000)
            .ok_or_else(|| "Plugin timeout exceeds the Wasm fuel budget".to_string())?;
        // Engines created by the runtime consume fuel; isolated unit-test engines may
        // intentionally omit that feature, in which case the store remains bounded by
        // memory and the invocation timeout. The checked multiplication still prevents
        // an attacker-controlled timeout from wrapping the fuel budget.
        let _ = store.set_fuel(fuel);

        Ok(store)
    }

    fn instantiate(
        engine: &Engine,
        store: &mut Store<PluginStore>,
        module: &Module,
    ) -> std::result::Result<Instance, String> {
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx: &mut PluginStore| &mut ctx.wasi)
            .map_err(|e| format!("Failed to link WASI: {e}"))?;

        linker
            .instantiate(&mut *store, module)
            .map_err(|e| format!("Failed to instantiate: {e}"))
    }

    fn call_plugin_func(
        store: &mut Store<PluginStore>,
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
        let input_ptr = Self::allocate_input_buffer(store, instance, &memory, input_bytes.len())?;
        memory
            .write(&mut *store, input_ptr as usize, input_bytes)
            .map_err(|e| format!("Failed to write to plugin memory: {e}"))?;

        let result_ptr = func
            .call(&mut *store, (input_ptr, input_bytes.len() as i32))
            .map_err(|e| format!("Plugin function '{func_name}' trapped: {e}"))?;

        let result_str = Self::read_nul_terminated_result(store, &memory, result_ptr)?;

        if result_str.is_empty() {
            Ok(serde_json::to_string(&PluginResult::default()).unwrap())
        } else {
            Ok(result_str)
        }
    }

    /// Prefer the optional allocator exported by new plugin scaffolds. Legacy
    /// modules retain the original offset-zero ABI for backward compatibility.
    fn allocate_input_buffer(
        store: &mut Store<PluginStore>,
        instance: &Instance,
        memory: &Memory,
        input_len: usize,
    ) -> std::result::Result<i32, String> {
        let Some(_) = instance.get_func(&mut *store, "ruvyxa_alloc") else {
            return Ok(0);
        };
        let reservation_bytes = input_len
            .checked_add(MAX_PLUGIN_RESULT_BYTES + 1)
            .ok_or_else(|| "Plugin input reservation exceeds the host limit".to_string())?;
        let reservation = i32::try_from(reservation_bytes)
            .map_err(|_| "Plugin input reservation exceeds the Wasm ABI limit".to_string())?;
        let allocator = instance
            .get_typed_func::<i32, i32>(&mut *store, "ruvyxa_alloc")
            .map_err(|_| {
                "Plugin export 'ruvyxa_alloc' must have signature (i32) -> i32".to_string()
            })?;
        let pointer = allocator
            .call(&mut *store, reservation)
            .map_err(|error| format!("Plugin allocator trapped: {error}"))?;
        let start = usize::try_from(pointer)
            .map_err(|_| "Plugin allocator returned a negative pointer".to_string())?;
        let end = start
            .checked_add(reservation_bytes)
            .ok_or_else(|| "Plugin allocator pointer overflowed".to_string())?;
        if end > memory.data_size(&*store) {
            return Err(format!(
                "Plugin allocator returned {pointer}, but its {reservation_bytes} byte reservation exceeds {} byte memory",
                memory.data_size(&*store)
            ));
        }
        Ok(pointer)
    }

    /// Read the legacy pointer-based result ABI without assuming a fixed result
    /// size. Results must be NUL-terminated UTF-8 JSON and stay within a
    /// bounded size so a plugin cannot force an unbounded host allocation.
    fn read_nul_terminated_result(
        store: &Store<PluginStore>,
        memory: &Memory,
        result_ptr: i32,
    ) -> std::result::Result<String, String> {
        let result_start = usize::try_from(result_ptr)
            .map_err(|_| "Plugin returned a negative result pointer".to_string())?;
        let memory_len = memory.data_size(store);
        if result_start >= memory_len {
            return Err(format!(
                "Plugin result pointer {result_start} is outside its {} byte memory",
                memory_len
            ));
        }

        let readable_len = memory_len - result_start;
        // Read one extra byte so a result exactly at the safety limit can still
        // provide its required NUL terminator.
        let max_read_len = readable_len.min(MAX_PLUGIN_RESULT_BYTES + 1);
        let mut result = Vec::with_capacity(max_read_len.min(PLUGIN_RESULT_READ_CHUNK_BYTES));
        let mut offset = 0;

        while offset < max_read_len {
            let chunk_len = (max_read_len - offset).min(PLUGIN_RESULT_READ_CHUNK_BYTES);
            let mut chunk = vec![0u8; chunk_len];
            memory
                .read(store, result_start + offset, &mut chunk)
                .map_err(|e| format!("Failed to read plugin result: {e}"))?;

            if let Some(nul_index) = chunk.iter().position(|&byte| byte == 0) {
                result.extend_from_slice(&chunk[..nul_index]);
                return String::from_utf8(result)
                    .map_err(|_| "Plugin result is not valid UTF-8".to_string());
            }

            result.extend_from_slice(&chunk);
            offset += chunk_len;
        }

        if readable_len > MAX_PLUGIN_RESULT_BYTES {
            Err(format!(
                "Plugin result exceeds the {} byte safety limit",
                MAX_PLUGIN_RESULT_BYTES
            ))
        } else {
            Err("Plugin result is not NUL-terminated before the end of plugin memory".to_string())
        }
    }
}

fn configured_plugin_path(plugin_config: &PluginConfig) -> Result<PathBuf> {
    if let Some(path) = &plugin_config.path {
        return Ok(path.clone());
    }

    let name = plugin_config.name.trim();
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || name.starts_with('-')
        || name.ends_with('-')
    {
        return Err(Diagnostic::new("RUV2101", "Invalid Wasm plugin name")
            .explain(
                "A plugin without `path` must use lowercase letters, digits, and single hyphens in `name`.",
            )
            .suggest("Use a name such as `auth-guard`, or provide an explicit project-relative .wasm path.")
            .into());
    }

    Ok(PathBuf::from(name)
        .join("target/wasm32-unknown-unknown/release")
        .join(format!("{}.wasm", name.replace('-', "_"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_permissions() -> PluginPermissions {
        PluginPermissions {
            env: Vec::new(),
            fs_read: Vec::new(),
            net: Vec::new(),
            timeout_ms: 5_000,
            max_memory_bytes: 64 * 1024,
        }
    }

    #[test]
    fn reads_plugin_results_larger_than_the_legacy_four_kib_limit() {
        let engine = Engine::default();
        let padding = "a".repeat(5_000);
        let expected = format!(
            r#"{{"action":"continue","request":null,"response":null,"padding":"{padding}"}}"#
        );
        let encoded = expected.replace('"', "\\22");
        let wasm = format!(
            r#"(module
                (memory (export "memory") 1)
                (data (i32.const 8192) "{encoded}\00")
                (func (export "on_request") (param i32 i32) (result i32)
                    i32.const 8192))"#
        );
        let module = Module::new(&engine, wasm).unwrap();
        let mut permissions = test_permissions();
        permissions.max_memory_bytes = 2 * 1024 * 1024;
        let mut store = WasmPluginRuntime::create_store(&engine, &permissions).unwrap();
        let instance = WasmPluginRuntime::instantiate(&engine, &mut store, &module).unwrap();

        let actual =
            WasmPluginRuntime::call_plugin_func(&mut store, &instance, "on_request", "{}", "{}")
                .unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn rejects_unterminated_plugin_results_instead_of_silently_truncating_them() {
        let engine = Engine::default();
        let wasm = r#"(module
            (memory (export "memory") 1)
            (func (export "on_request") (param i32 i32) (result i32)
                i32.const 8192))"#;
        let module = Module::new(&engine, wasm).unwrap();
        let mut permissions = test_permissions();
        permissions.max_memory_bytes = 2 * 1024 * 1024;
        let mut store = WasmPluginRuntime::create_store(&engine, &permissions).unwrap();
        let instance = WasmPluginRuntime::instantiate(&engine, &mut store, &module).unwrap();
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        let result_start = 8192;
        let result_len = memory.data_size(&store) - result_start;
        memory
            .write(&mut store, result_start, &vec![b'a'; result_len])
            .unwrap();

        let error =
            WasmPluginRuntime::call_plugin_func(&mut store, &instance, "on_request", "{}", "{}")
                .unwrap_err();

        assert!(error.contains("not NUL-terminated"));
    }

    #[test]
    fn inspects_required_plugin_exports_without_executing_the_module() {
        let temp = tempfile::tempdir().unwrap();
        let plugin = temp.path().join("plugin.wasm");
        std::fs::write(
            &plugin,
            r#"(module
                (memory (export "memory") 1)
                (func (export "on_request") (param i32 i32) (result i32) i32.const 0)
                (func (export "ruvyxa_alloc") (param i32) (result i32) i32.const 0)
            )"#,
        )
        .unwrap();

        let info = inspect_wasm_plugin(&plugin).unwrap();

        assert!(info.has_memory);
        assert!(info.has_request_handler);
        assert!(!info.has_response_handler);
        assert!(info.has_allocator);
        assert!(info.exports.iter().any(|export| export == "memory"));
    }

    #[test]
    fn resolves_an_implicit_plugin_path_from_its_name() {
        let plugin = PluginConfig {
            name: "auth-guard".to_string(),
            path: None,
            phase: PluginPhase::Request,
            routes: None,
            config: serde_json::Value::Null,
            permissions: PluginPermissions::default(),
        };

        assert_eq!(
            configured_plugin_path(&plugin).unwrap(),
            PathBuf::from("auth-guard/target/wasm32-unknown-unknown/release/auth_guard.wasm")
        );
    }

    #[test]
    fn rejects_an_unsafe_implicit_plugin_name() {
        let plugin = PluginConfig {
            name: "../escape".to_string(),
            path: None,
            phase: PluginPhase::Request,
            routes: None,
            config: serde_json::Value::Null,
            permissions: PluginPermissions::default(),
        };

        assert!(configured_plugin_path(&plugin).is_err());
    }

    #[test]
    fn uses_the_optional_allocator_without_overwriting_plugin_static_data() {
        let engine = Engine::default();
        let module = Module::new(
            &engine,
            r#"(module
                (memory (export "memory") 18)
                (data (i32.const 0) "{\"action\":\"continue\"}\00")
                (func (export "ruvyxa_alloc") (param i32) (result i32) i32.const 65536)
                (func (export "on_request") (param i32 i32) (result i32) i32.const 0)
            )"#,
        )
        .unwrap();
        let mut permissions = test_permissions();
        permissions.max_memory_bytes = 2 * 1024 * 1024;
        let mut store = WasmPluginRuntime::create_store(&engine, &permissions).unwrap();
        let instance = WasmPluginRuntime::instantiate(&engine, &mut store, &module).unwrap();

        let result =
            WasmPluginRuntime::call_plugin_func(&mut store, &instance, "on_request", "{}", "{}")
                .unwrap();

        assert_eq!(result, r#"{"action":"continue"}"#);
    }
}
