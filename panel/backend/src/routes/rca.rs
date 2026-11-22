// MCP-Grounded Root Cause Analysis Agent routes
// Provides endpoints for triggering and managing diagnoses

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, internal_error, ApiError};
use crate::services::rca_agent::{
    DiagnosisFeedback, DiagnosisRecord, DiagnosisRequest, DiagnosisResult, DiagnosisTrigger,
    RcaAgent, RcaConfig,
};
use crate::AppState;
use sqlx;

/// Global RCA agent instance
static RCA_AGENT: std::sync::OnceLock<tokio::sync::Mutex<Option<RcaAgent>>> = std::sync::OnceLock::new();

/// Get or initialize the RCA agent
async fn get_rca_agent(state: &AppState) -> Result<std::sync::Arc<tokio::sync::Mutex<Option<RcaAgent>>>, ApiError> {
    let agent_lock = RCA_AGENT.get_or_init(|| tokio::sync::Mutex::new(None)).clone();
    let mut guard = agent_lock.lock().await;

    if guard.is_none() {
        // Load config from settings
        let settings: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM settings WHERE key IN (
                'rca_enabled', 'rca_mcp_url', 'rca_mcp_api_key', 'rca_auto_trigger_alert',
                'rca_auto_trigger_deploy', 'rca_max_history', 'rca_llm_provider',
                'rca_llm_model', 'rca_llm_api_key'
            )"
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("rca settings", e))?;

        let get = |k: &str| -> String {
            settings.iter()
                .find(|(sk, _)| sk == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };

        let enabled = get("rca_enabled") == "true";
        let mcp_url = get("rca_mcp_url");
        let mcp_api_key = get("rca_mcp_api_key");
        let auto_trigger_alert = get("rca_auto_trigger_alert") == "true";
        let auto_trigger_deploy = get("rca_auto_trigger_deploy") == "true";
        let max_history = get("rca_max_history").parse::<usize>().unwrap_or(100);
        let llm_provider = get("rca_llm_provider");
        let llm_model = get("rca_llm_model");
        let llm_api_key = get("rca_llm_api_key");

        if enabled && (mcp_url.is_empty() || mcp_api_key.is_empty()) {
            return Err(err(StatusCode::SERVICE_UNAVAILABLE, "RCA agent enabled but MCP URL or API key not configured"));
        }

        let config = RcaConfig {
            enabled,
            mcp_server_url: mcp_url,
            mcp_api_key,
            auto_trigger_on_alert: auto_trigger_alert,
            auto_trigger_on_deploy_failure: auto_trigger_deploy,
            max_diagnosis_history: max_history,
            llm_provider,
            llm_model,
            llm_api_key,
        };

        let agent = RcaAgent::new(config).map_err(|e| internal_error("rca agent init", e))?;
        *guard = Some(agent);
    }

    Ok(std::sync::Arc::new(tokio::sync::Mutex::new(guard.take())))
}

/// POST /api/rca/diagnose — Trigger a diagnosis
pub async fn diagnose(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<DiagnosisRequest>,
) -> Result<Json<DiagnosisResult>, ApiError> {
    let agent_lock = get_rca_agent(&state).await?;
    let agent = agent_lock.lock().await;
    
    if agent.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "RCA agent not configured"));
    }
    
    let agent = agent.as_ref().unwrap();
    let result = agent.diagnose(body).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(result))
}

/// GET /api/rca/diagnoses — List diagnosis history
pub async fn list_diagnoses(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Query(params): Query<ListDiagnosesParams>,
) -> Result<Json<Vec<DiagnosisRecord>>, ApiError> {
    let agent_lock = get_rca_agent(&state).await?;
    let agent = agent_lock.lock().await;
    
    if agent.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "RCA agent not configured"));
    }
    
    let agent = agent.as_ref().unwrap();
    let history = agent.get_history().await;

    let limit = params.limit.unwrap_or(50).min(200) as usize;
    let offset = params.offset.unwrap_or(0) as usize;

    let result: Vec<DiagnosisRecord> = history
        .into_iter()
        .rev() // newest first
        .skip(offset)
        .take(limit)
        .collect();

    Ok(Json(result))
}

/// GET /api/rca/diagnoses/{id} — Get specific diagnosis
pub async fn get_diagnosis(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<DiagnosisRecord>, ApiError> {
    let agent_lock = get_rca_agent(&state).await?;
    let agent = agent_lock.lock().await;
    
    if agent.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "RCA agent not configured"));
    }
    
    let agent = agent.as_ref().unwrap();
    let history = agent.get_history().await;

    history
        .into_iter()
        .find(|r| r.id == id)
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Diagnosis not found"))
}

/// GET /api/rca/current — Get current in-progress diagnosis
pub async fn get_current_diagnosis(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<Option<DiagnosisResult>>, ApiError> {
    let agent_lock = get_rca_agent(&state).await?;
    let agent = agent_lock.lock().await;
    
    if agent.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "RCA agent not configured"));
    }
    
    let agent = agent.as_ref().unwrap();
    let current = agent.get_current().await;

    Ok(Json(current))
}

/// POST /api/rca/diagnoses/{id}/feedback — Submit feedback for a diagnosis
pub async fn submit_feedback(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<DiagnosisFeedback>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent_lock = get_rca_agent(&state).await?;
    let agent = agent_lock.lock().await;
    
    if agent.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "RCA agent not configured"));
    }
    
    let agent = agent.as_ref().unwrap();
    agent.submit_feedback(id, body).await
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "rca.feedback",
        Some("diagnosis"), Some(id.to_string()), None, None, claims.key_id,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/rca/config — Get RCA agent configuration (admin only)
pub async fn get_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Only admins can view config
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin access required"));
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'rca_%'"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("rca config", e))?;

    let mut config = serde_json::Map::new();
    for (k, v) in rows {
        // Mask API keys
        if k.contains("api_key") || k.contains("secret") {
            config.insert(k, serde_json::Value::String("***REDACTED***".to_string()));
        } else {
            config.insert(k, serde_json::Value::String(v));
        }
    }

    Ok(Json(serde_json::Value::Object(config)))
}

/// PUT /api/rca/config — Update RCA agent configuration (admin only)
pub async fn update_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Only admins can update config
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin access required"));
    }

    let ip = crate::routes::client_ip(&headers);

    for (key, value) in body.as_object().ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid JSON object"))? {
        if !key.starts_with("rca_") {
            continue; // Only allow rca_* settings
        }

        let value_str = match value {
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        };

        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ($1, $2) \
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()"
        )
        .bind(key)
        .bind(&value_str)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("update rca setting", e))?;

        crate::services::activity::log_activity(
            &state.db, claims.sub, &claims.email, "rca.config_updated",
            Some("setting"), Some(key.clone()), None, ip.as_deref(), claims.key_id,
        ).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ListDiagnosesParams {
    limit: Option<i64>,
    offset: Option<i64>,
}

pub fn router() -> axum::Router<crate::AppState> {
    axum::Router::new()
        .route("/api/rca/diagnose", axum::routing::post(diagnose))
        .route("/api/rca/diagnoses", axum::routing::get(list_diagnoses))
        .route("/api/rca/diagnoses/{id}", axum::routing::get(get_diagnosis))
        .route("/api/rca/current", axum::routing::get(get_current_diagnosis))
        .route("/api/rca/diagnoses/{id}/feedback", axum::routing::post(submit_feedback))
        .route("/api/rca/config", axum::routing::get(get_config).axum::routing::put(update_config))
}