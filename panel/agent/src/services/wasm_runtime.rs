// Wasm Component Model Runtime for DockPanel
// Provides sandboxed execution of WebAssembly components with WASI 0.2
// Used for: webhook handlers, edge functions, user-provided code

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use wasmtime::{
    component::{Component, Linker, ResourceTable},
    Config, Engine, Store,
};
use wasmtime_wasi::preview2::{
    command::{Command, StdinStream, StdoutStream, StderrStream},
    WasiCtx, WasiCtxBuilder, WasiView,
};
use crate::safe_cmd::safe_command;
use std::collections::HashMap;

const WASM_COMPONENTS_DIR: &str = "/var/lib/dockpanel/wasm-components";
const WASM_CACHE_DIR: &str = "/var/lib/dockpanel/wasm-cache";

/// Resource limits for a Wasm component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmResourceLimits {
    pub max_memory_mb: u64,      // Maximum linear memory in MB
    pub max_fuel: u64,           // Maximum fuel (instruction count)
    pub epoch_deadline: u64,     // Epoch deadline for cooperative yielding
    pub max_table_elements: u32, // Maximum table elements
    pub allowed_hosts: Vec<String>, // HTTP allowlist for outbound requests
}

impl Default for WasmResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: 64,
            max_fuel: 10_000_000,
            epoch_deadline: 1000,
            max_table_elements: 1024,
            allowed_hosts: vec![],
        }
    }
}

/// Wasm component metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmComponentMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub component_path: String,
    pub wit_path: Option<String>,
    pub resource_limits: WasmResourceLimits,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Wasm runtime host - manages component execution
pub struct WasmRuntime {
    engine: Engine,
    linker: Linker<WasmState>,
    components: Arc<Mutex<HashMap<String, WasmComponent>>>,
}

/// State for a Wasm component execution
pub struct WasmState {
    ctx: WasiCtx,
    resource_table: ResourceTable,
    limits: WasmResourceLimits,
    http_client: Option<reqwest::Client>,
}

impl WasmView for WasmState {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }
}

/// A loaded Wasm component
struct WasmComponent {
    component: Component,
    meta: WasmComponentMeta,
}

impl WasmRuntime {
    /// Create a new Wasm runtime
    pub async fn new() -> Result<Self, String> {
        // Create engine with fuel and epoch interruption for resource limits
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        config.max_wasm_stack(1024 * 1024); // 1MB stack

        let engine = Engine::new(&config).map_err(|e| format!("Failed to create Wasmtime engine: {e}"))?;

        // Create linker with WASI preview2
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::preview2::command::add_to_linker(&mut linker, |state: &mut WasmState| state)
            .map_err(|e| format!("Failed to add WASI to linker: {e}"))?;

        // Ensure directories exist
        tokio::fs::create_dir_all(WASM_COMPONENTS_DIR)
            .await
            .map_err(|e| format!("Failed to create components dir: {e}"))?;
        tokio::fs::create_dir_all(WASM_CACHE_DIR)
            .await
            .map_err(|e| format!("Failed to create cache dir: {e}"))?;

        Ok(Self {
            engine,
            linker,
            components: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Load a Wasm component from a file
    pub async fn load_component(&self, meta: WasmComponentMeta) -> Result<String, String> {
        let component = Component::from_file(&self.engine, &meta.component_path)
            .map_err(|e| format!("Failed to load component: {e}"))?;

        let mut components = self.components.lock().await;
        let id = meta.id.clone();
        components.insert(
            id.clone(),
            WasmComponent {
                component,
                meta,
            },
        );

        Ok(id)
    }

    /// Load a component from bytes (for upload)
    pub async fn load_component_from_bytes(
        &self,
        meta: WasmComponentMeta,
        bytes: Vec<u8>,
    ) -> Result<String, String> {
        let component = Component::new(&self.engine, &bytes)
            .map_err(|e| format!("Failed to parse component: {e}"))?;

        let mut components = self.components.lock().await;
        let id = meta.id.clone();
        components.insert(
            id.clone(),
            WasmComponent {
                component,
                meta,
            },
        );

        Ok(id)
    }

    /// Execute a component with the given arguments
    pub async fn execute(
        &self,
        component_id: &str,
        args: Vec<String>,
    ) -> Result<WasmExecutionResult, String> {
        let component = {
            let components = self.components.lock().await;
            components
                .get(component_id)
                .ok_or_else(|| format!("Component not found: {component_id}"))?
                .component
                .clone()
        };

        let meta = {
            let components = self.components.lock().await;
            components
                .get(component_id)
                .ok_or_else(|| format!("Component not found: {component_id}"))?
                .meta
                .clone()
        };

        // Create WASI context with resource limits
        let mut ctx_builder = WasiCtxBuilder::new();
        ctx_builder.args(&args);

        // Set resource limits
        ctx_builder.memory_limit(meta.resource_limits.max_memory_mb * 1024 * 1024);

        // Configure allowed hosts for HTTP
        if !meta.resource_limits.allowed_hosts.is_empty() {
            // Note: WASI preview2 HTTP is not yet stable, this is a placeholder
            // for when the wasi:http interface is available
        }

        let ctx = ctx_builder.build();
        let mut store = Store::new(&self.engine, WasmState {
            ctx,
            resource_table: ResourceTable::new(),
            limits: meta.resource_limits,
            http_client: Some(reqwest::Client::new()),
        });

        // Set fuel limit
        store.add_fuel(meta.resource_limits.max_fuel)
            .map_err(|e| format!("Failed to set fuel: {e}"))?;

        // Set epoch deadline for cooperative yielding
        store.set_epoch_deadline(meta.resource_limits.epoch_deadline);

        // Get the command interface
        let command = Command::instantiate_async(&mut store, &component, &self.linker)
            .await
            .map_err(|e| format!("Failed to instantiate component: {e}"))?;

        // Execute the command
        let start = std::time::Instant::now();
        let result = command.wasi_cli_run(&mut store).await;
        let execution_time = start.elapsed();

        // Consume remaining fuel
        let fuel_consumed = store.fuel_consumed().unwrap_or(0);

        match result {
            Ok(()) => Ok(WasmExecutionResult {
                success: true,
                exit_code: 0,
                stdout: String::new(), // Would be captured from WASI
                stderr: String::new(),
                execution_time_ms: execution_time.as_millis() as u64,
                fuel_consumed,
            }),
            Err(e) => Ok(WasmExecutionResult {
                success: false,
                exit_code: 1,
                stdout: String::new(),
                stderr: e.to_string(),
                execution_time_ms: execution_time.as_millis() as u64,
                fuel_consumed,
            }),
        }
    }

    /// List all loaded components
    pub async fn list_components(&self) -> Vec<WasmComponentMeta> {
        let components = self.components.lock().await;
        components.values().map(|c| c.meta.clone()).collect()
    }

    /// Unload a component
    pub async fn unload_component(&self, component_id: &str) -> Result<(), String> {
        let mut components = self.components.lock().await;
        components
            .remove(component_id)
            .ok_or_else(|| format!("Component not found: {component_id}"))?;
        Ok(())
    }

    /// Install wasmtime if not available
    pub async fn ensure_wasmtime() -> Result<(), String> {
        // wasmtime is a Rust library, not a binary - it's linked at compile time
        Ok(())
    }
}

/// Result of a Wasm component execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmExecutionResult {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub execution_time_ms: u64,
    pub fuel_consumed: u64,
}

/// Component upload request
#[derive(Debug, Deserialize)]
pub struct ComponentUploadRequest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub wit: Option<String>,
    #[serde(default)]
    pub resource_limits: WasmResourceLimits,
}

/// Component execution request
#[derive(Debug, Deserialize)]
pub struct ComponentExecuteRequest {
    pub component_id: String,
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_wasm_runtime_creation() {
        let runtime = WasmRuntime::new().await;
        assert!(runtime.is_ok());
    }

    #[test]
    fn test_default_resource_limits() {
        let limits = WasmResourceLimits::default();
        assert_eq!(limits.max_memory_mb, 64);
        assert_eq!(limits.max_fuel, 10_000_000);
    }
}