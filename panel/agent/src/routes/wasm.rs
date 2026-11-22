// Wasm Component Model routes for agent
// Provides component upload, execution, and management

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::AppState;
use crate::services::wasm_runtime::{
    ComponentExecuteRequest, ComponentUploadRequest, WasmComponentMeta, WasmExecutionResult,
    WasmResourceLimits, WasmRuntime,
};

/// Global Wasm runtime instance
static WASM_RUNTIME: std::sync::OnceLock<Arc<Mutex<Option<WasmRuntime>>>> = std::sync::OnceLock::new();

/// Get or initialize the Wasm runtime
async fn get_runtime() -> Arc<Mutex<Option<WasmRuntime>>> {
    WASM_RUNTIME
        .get_or_init(|| Arc::new(Mutex::new(None)))
        .clone()
}

/// Ensure runtime is initialized
async fn ensure_runtime_initialized() -> Result<(), String> {
    let runtime = get_runtime().await;
    let mut guard = runtime.lock().await;
    if guard.is_none() {
        *guard = Some(WasmRuntime::new().await?);
    }
    Ok(())
}

/// POST /wasm/upload -- Upload a Wasm component
async fn upload_component(
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<WasmComponentMeta>, (StatusCode, Json<serde_json::Value>)> {
    ensure_runtime_initialized().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let mut name = String::new();
    let mut version = String::new();
    let mut description = String::new();
    let mut wit = None;
    let mut resource_limits = WasmResourceLimits::default();
    let mut component_bytes = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Multipart error: {e}") })),
        )
    })? {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "name" => name = field.text().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("Failed to read name: {e}") })),
                )
            })?,
            "version" => version = field.text().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("Failed to read version: {e}") })),
                )
            })?,
            "description" => description = field.text().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("Failed to read description: {e}") })),
                )
            })?,
            "wit" => wit = Some(field.text().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("Failed to read wit: {e}") })),
                )
            })?),
            "resource_limits" => {
                let text = field.text().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": format!("Failed to read resource_limits: {e}") })),
                    )
                })?;
                resource_limits = serde_json::from_str(&text).map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": format!("Invalid resource_limits JSON: {e}") })),
                    )
                })?;
            }
            "component" => {
                component_bytes = field.bytes().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": format!("Failed to read component: {e}") })),
                    )
                })?.to_vec();
            }
            _ => {}
        }
    }

    if name.is_empty() || version.is_empty() || component_bytes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "name, version, and component are required" })),
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let meta = WasmComponentMeta {
        id: id.clone(),
        name: name.clone(),
        version: version.clone(),
        description,
        component_path: format!("/var/lib/arsy/wasm-components/{id}.wasm"),
        wit_path: wit.map(|w| format!("/var/lib/arsy/wasm-components/{id}.wit")),
        resource_limits,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    // Save component bytes to disk
    let runtime = get_runtime().await;
    let runtime_guard = runtime.lock().await;
    if let Some(ref runtime) = *runtime_guard {
        runtime
            .load_component_from_bytes(meta.clone(), component_bytes)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
            })?;
    }

    // Save bytes to disk
    tokio::fs::write(&meta.component_path, &meta.component_path).await.ok();
    if let Some(ref wit_path) = meta.wit_path {
        tokio::fs::write(wit_path, wit.unwrap()).await.ok();
    }

    Ok(Json(meta))
}

/// GET /wasm/components -- List all loaded components
async fn list_components(
    State(_state): State<AppState>,
) -> Result<Json<Vec<WasmComponentMeta>>, (StatusCode, Json<serde_json::Value>)> {
    ensure_runtime_initialized().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let runtime = get_runtime().await;
    let runtime_guard = runtime.lock().await;
    if let Some(ref runtime) = *runtime_guard {
        let components = runtime.list_components().await;
        Ok(Json(components))
    } else {
        Ok(Json(vec![]))
    }
}

/// POST /wasm/execute -- Execute a Wasm component
async fn execute_component(
    State(_state): State<AppState>,
    Json(body): Json<ComponentExecuteRequest>,
) -> Result<Json<WasmExecutionResult>, (StatusCode, Json<serde_json::Value>)> {
    ensure_runtime_initialized().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let runtime = get_runtime().await;
    let runtime_guard = runtime.lock().await;
    if let Some(ref runtime) = *runtime_guard {
        let result = runtime
            .execute(&body.component_id, body.args)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
            })?;
        Ok(Json(result))
    } else {
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Wasm runtime not initialized" })),
        ))
    }
}

/// DELETE /wasm/components/{id} -- Unload a component
async fn unload_component(
    State(_state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    ensure_runtime_initialized().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let runtime = get_runtime().await;
    let runtime_guard = runtime.lock().await;
    if let Some(ref runtime) = *runtime_guard {
        runtime
            .unload_component(&id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
            })?;
    }

    // Remove from disk
    let path = format!("/var/lib/arsy/wasm-components/{id}.wasm");
    tokio::fs::remove_file(&path).await.ok();
    let wit_path = format!("/var/lib/arsy/wasm-components/{id}.wit");
    tokio::fs::remove_file(&wit_path).await.ok();

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /wasm/resource-limits/default -- Get default resource limits
async fn default_resource_limits() -> Json<WasmResourceLimits> {
    Json(WasmResourceLimits::default())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/wasm/components", get(list_components).post(upload_component))
        .route("/wasm/components/{id}", delete(unload_component))
        .route("/wasm/execute", post(execute_component))
        .route("/wasm/resource-limits/default", get(default_resource_limits))
}