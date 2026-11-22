// Policy-as-Code Admission Controller routes
// Provides endpoints for managing OPA/Rego policies and evaluating admissions

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, internal_error, ApiError};
use crate::services::policy_engine::{
    EvaluateRequest, EvaluateResponse, Policy, PolicyDecision, PolicyEngine, PolicyEngineConfig,
    PolicyInput, PolicyResult,
};
use crate::AppState;

/// Global Policy Engine instance
static POLICY_ENGINE: std::sync::OnceLock<tokio::sync::Mutex<Option<PolicyEngine>>> = std::sync::OnceLock::new();

/// Get or initialize the Policy Engine
async fn get_policy_engine(state: &AppState) -> Result<std::sync::Arc<tokio::sync::Mutex<Option<PolicyEngine>>>, ApiError> {
    let engine_lock = POLICY_ENGINE.get_or_init(|| tokio::sync::Mutex::new(None)).clone();
    let mut guard = engine_lock.lock().await;

    if guard.is_none() {
        let config = create_policy_engine_from_settings(state).await
            .map_err(|e| internal_error("policy engine config", e))?;
        
        let mut engine = PolicyEngine::new(config);
        engine.load_builtin_policies().await
            .map_err(|e| internal_error("load builtin policies", e))?;
        
        *guard = Some(engine);
    }

    Ok(std::sync::Arc::new(tokio::sync::Mutex::new(guard.take())))
}

/// POST /api/policy/evaluate — Evaluate a policy input
pub async fn evaluate(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<EvaluateRequest>,
) -> Result<Json<EvaluateResponse>, ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();
    let result = engine.evaluate(body.input.clone()).await;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "policy.evaluate",
        Some("policy"), None, None, None, claims.key_id,
    ).await;

    Ok(Json(EvaluateResponse {
        result,
        input: body.input,
    }))
}

/// POST /api/policy/policies — Create a new custom policy
pub async fn create_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<Policy>,
) -> Result<(StatusCode, Json<Policy>), ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();

    if body.id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Policy ID is required"));
    }
    if body.rego.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Policy Rego code is required"));
    }

    engine.upsert_policy(body.clone()).await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "policy.created",
        Some("policy"), Some(body.id.clone()), None, None, claims.key_id,
    ).await;

    Ok((StatusCode::CREATED, Json(body)))
}

/// GET /api/policy/policies — List all policies
pub async fn list_policies(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<Vec<Policy>>, ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();
    let policies = engine.get_policies().await;

    Ok(Json(policies))
}

/// GET /api/policy/policies/{id} — Get a specific policy
pub async fn get_policy(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Policy>, ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();
    let policy = engine.get_policy(&id).await
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Policy not found"))?;

    Ok(Json(policy))
}

/// PUT /api/policy/policies/{id} — Update a policy
pub async fn update_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<Policy>,
) -> Result<Json<Policy>, ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();
    
    let existing = engine.get_policy(&id).await
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Policy not found"))?;

    if body.id != id {
        return Err(err(StatusCode::BAD_REQUEST, "Policy ID in body must match path"));
    }
    if body.rego.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Policy Rego code is required"));
    }

    let mut updated = body.clone();
    updated.updated_at = chrono::Utc::now();
    engine.upsert_policy(updated.clone()).await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "policy.updated",
        Some("policy"), Some(id.to_string()), None, None, claims.key_id,
    ).await;

    Ok(Json(updated))
}

/// DELETE /api/policy/policies/{id} — Delete a policy
pub async fn delete_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();
    engine.delete_policy(&id).await
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "policy.deleted",
        Some("policy"), Some(id.to_string()), None, None, claims.key_id,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/policy/evaluate — Evaluate a custom input (for testing)
pub async fn test_evaluate(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<PolicyInput>,
) -> Result<Json<PolicyResult>, ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();
    let result = engine.evaluate(body).await;

    Ok(Json(result))
}

/// GET /api/policy/policies — List all policies (alias for GET /api/policy/policies)
pub async fn list_policies(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<Vec<Policy>>, ApiError> {
    let engine_lock = get_policy_engine(&state).await?;
    let engine = engine_lock.lock().await;
    
    if engine.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Policy engine not configured"));
    }
    
    let engine = engine.as_ref().unwrap();
    let policies = engine.get_policies().await;

    Ok(Json(policies))
}

/// GET /api/policy/config — Get policy engine configuration (admin only)
pub async fn get_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Only admins can view config
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin access required"));
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'policy_engine_%'"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("policy engine config", e))?;

    let mut config = serde_json::Map::new();
    for (k, v) in rows {
        if k.contains("secret") || k.contains("key") || k.contains("url") {
            config.insert(k, serde_json::Value::String("***REDACTED***".to_string()));
        } else {
            config.insert(k, serde_json::Value::String(v));
        }
    }

    Ok(Json(serde_json::Value::Object(config)))
}

/// PUT /api/policy/config — Update policy engine configuration (admin only)
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
        if !key.starts_with("policy_engine_") {
            continue; // Only allow policy_engine_* settings
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
        .map_err(|e| internal_error("update policy engine setting", e))?;

        crate::services::activity::log_activity(
            &state.db, claims.sub, &claims.email, "policy.config_updated",
            Some("setting"), Some(key.clone()), None, ip.as_deref(), claims.key_id,
        ).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Create policy engine from settings
async fn create_policy_engine_from_settings(state: &AppState) -> Result<PolicyEngine, String> {
    let settings: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN (
            'policy_engine_enabled', 'policy_engine_opa_url',
            'policy_engine_bundled_policies_path', 'policy_engine_custom_policies_path',
            'policy_engine_default_decision', 'policy_engine_audit_mode'
        )"
    )
    .fetch_all(state.db)
    .await
    .map_err(|e| format!("Failed to load policy engine settings: {e}"))?;

    let get = |k: &str| -> String {
        settings.iter()
            .find(|(sk, _)| sk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };

    let enabled = get("policy_engine_enabled") == "true";
    let opa_url = if get("policy_engine_opa_url").is_empty() { None } else { Some(get("policy_engine_opa_url")) };
    let bundled_policies_path = get("policy_engine_bundled_policies_path").unwrap_or_else(|| "/etc/dockpanel/policies/bundled".to_string());
    let custom_policies_path = get("policy_engine_custom_policies_path").unwrap_or_else(|| "/etc/dockpanel/policies/custom".to_string());
    let default_decision = match get("policy_engine_default_decision").as_str() {
        "deny" => PolicyDecision::Deny,
        "warn" => PolicyDecision::Warn,
        _ => PolicyDecision::Allow,
    };
    let audit_mode = get("policy_engine_audit_mode") == "true";

    let config = PolicyEngineConfig {
        enabled,
        opa_url,
        bundled_policies_path,
        custom_policies_path,
        default_decision,
        audit_mode,
    };

    let mut engine = PolicyEngine::new(config);
    engine.load_builtin_policies().await
        .map_err(|e| format!("Failed to load builtin policies: {e}"))?;

    Ok(engine)
}

pub fn router() -> axum::Router<crate::AppState> {
    axum::Router::new()
        .route("/api/policy/evaluate", axum::routing::post(evaluate))
        .route("/api/policy/test-evaluate", axum::routing::post(test_evaluate))
        .route("/api/policy/policies", axum::routing::get(list_policies).post(create_policy))
        .route("/api/policy/policies/{id}", axum::routing::get(get_policy).put(update_policy).delete(delete_policy))
        .route("/api/policy/config", axum::routing::get(get_config).axum::routing::put(update_config))
}