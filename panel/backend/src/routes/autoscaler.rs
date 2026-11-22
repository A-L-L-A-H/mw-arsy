// Horizontal Autoscaler routes
// Provides endpoints for managing autoscaling rules and viewing status

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, internal_error, ApiError};
use crate::services::autoscaler::{
    Autoscaler, AutoscalerConfig, AutoscalingRule, ScaleAction, ScaleTriggerType, ScalingStatus,
};
use crate::AppState;

/// Global Autoscaler instance
static AUTOSCALER: std::sync::OnceLock<tokio::sync::Mutex<Option<Autoscaler>>> = std::sync::OnceLock::new();

/// Get or initialize the Autoscaler
async fn get_autoscaler(state: &AppState) -> Result<std::sync::Arc<tokio::sync::Mutex<Option<Autoscaler>>>, ApiError> {
    let autoscaler_lock = AUTOSCALER.get_or_init(|| tokio::sync::Mutex::new(None)).clone();
    let mut guard = autoscaler_lock.lock().await;

    if guard.is_none() {
        let config = crate::services::autoscaler::create_autoscaler_from_settings(state).await
            .map_err(|e| internal_error("autoscaler config", e))?;
        
        let autoscaler = Autoscaler::new(config);
        *guard = Some(autoscaler);
    }

    Ok(std::sync::Arc::new(tokio::sync::Mutex::new(guard.take())))
}

/// POST /api/autoscaler/rules — Create a new autoscaling rule
pub async fn create_rule(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateRuleRequest>,
) -> Result<(StatusCode, Json<AutoscalingRule>), ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();

    let rule = AutoscalingRule {
        id: Uuid::new_v4(),
        resource_type: body.resource_type,
        resource_id: body.resource_id,
        name: body.name,
        min_replicas: body.min_replicas,
        max_replicas: body.max_replicas,
        cooldown_seconds: body.cooldown_seconds.unwrap_or(300),
        triggers: body.triggers,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    autoscaler.upsert_rule(rule.clone()).await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "autoscaler.rule_created",
        Some("autoscaling_rule"), Some(rule.id.to_string()), None, None, claims.key_id,
    ).await;

    Ok((StatusCode::CREATED, Json(rule)))
}

/// GET /api/autoscaler/rules — List all autoscaling rules
pub async fn list_rules(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<Vec<AutoscalingRule>>, ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();
    let rules = autoscaler.get_rules().await;

    Ok(Json(rules))
}

/// GET /api/autoscaler/rules/{id} — Get a specific rule
pub async fn get_rule(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<AutoscalingRule>, ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();
    let rule = autoscaler.get_rule(id).await
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Rule not found"))?;

    Ok(Json(rule))
}

/// PUT /api/autoscaler/rules/{id} — Update a rule
pub async fn update_rule(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRuleRequest>,
) -> Result<Json<AutoscalingRule>, ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();
    
    let mut rule = autoscaler.get_rule(id).await
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Rule not found"))?;

    // Update fields if provided
    if let Some(name) = body.name {
        rule.name = name;
    }
    if let Some(min_replicas) = body.min_replicas {
        rule.min_replicas = min_replicas;
    }
    if let Some(max_replicas) = body.max_replicas {
        rule.max_replicas = max_replicas;
    }
    if let Some(cooldown_seconds) = body.cooldown_seconds {
        rule.cooldown_seconds = cooldown_seconds;
    }
    if let Some(triggers) = body.triggers {
        rule.triggers = triggers;
    }
    
    if rule.min_replicas > rule.max_replicas {
        return Err(err(StatusCode::BAD_REQUEST, "min_replicas cannot exceed max_replicas"));
    }
    if rule.triggers.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "At least one trigger is required"));
    }

    rule.updated_at = Utc::now();
    autoscaler.upsert_rule(rule.clone()).await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "autoscaler.rule_updated",
        Some("autoscaling_rule"), Some(id.to_string()), None, None, claims.key_id,
    ).await;

    Ok(Json(rule))
}

/// DELETE /api/autoscaler/rules/{id} — Delete a rule
pub async fn delete_rule(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();
    autoscaler.delete_rule(id).await
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "autoscaler.rule_deleted",
        Some("autoscaling_rule"), Some(id.to_string()), None, None, claims.key_id,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/autoscaler/status — Get all scaling statuses
pub async fn get_status(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<Vec<ScalingStatus>>, ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();
    let statuses = autoscaler.get_all_statuses().await;

    Ok(Json(statuses))
}

/// GET /api/autoscaler/status/{rule_id} — Get status for a specific rule
pub async fn get_rule_status(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ScalingStatus>, ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();
    let status = autoscaler.get_status(id).await
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Status not found for rule"))?;

    Ok(Json(status))
}

/// GET /api/autoscaler/history — Get scale action history
pub async fn get_history(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Vec<ScaleAction>>, ApiError> {
    let autoscaler_lock = get_autoscaler(&state).await?;
    let autoscaler = autoscaler_lock.lock().await;
    
    if autoscaler.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "Autoscaler not configured"));
    }
    
    let autoscaler = autoscaler.as_ref().unwrap();
    let history = autoscaler.get_history(params.limit).await;

    Ok(Json(history))
}

/// GET /api/autoscaler/config — Get autoscaler configuration (admin only)
pub async fn get_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Only admins can view config
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin access required"));
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'autoscaler_%'"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("autoscaler config", e))?;

    let mut config = serde_json::Map::new();
    for (k, v) in rows {
        if k.contains("secret") || k.contains("key") {
            config.insert(k, serde_json::Value::String("***REDACTED***".to_string()));
        } else {
            config.insert(k, serde_json::Value::String(v));
        }
    }

    Ok(Json(serde_json::Value::Object(config)))
}

/// PUT /api/autoscaler/config — Update autoscaler configuration (admin only)
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
        if !key.starts_with("autoscaler_") {
            continue; // Only allow autoscaler_* settings
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
        .map_err(|e| internal_error("update autoscaler setting", e))?;

        crate::services::activity::log_activity(
            &state.db, claims.sub, &claims.email, "autoscaler.config_updated",
            Some("setting"), Some(key.clone()), None, ip.as_deref(), claims.key_id,
        ).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct CreateRuleRequest {
    pub resource_type: ResourceType,
    pub resource_id: Uuid,
    pub name: String,
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub cooldown_seconds: Option<u64>,
    pub triggers: Vec<ScaleTriggerType>,
}

#[derive(Deserialize)]
struct UpdateRuleRequest {
    pub name: Option<String>,
    pub min_replicas: Option<u32>,
    pub max_replicas: Option<u32>,
    pub cooldown_seconds: Option<u64>,
    pub triggers: Option<Vec<ScaleTriggerType>>,
}

#[derive(Deserialize)]
struct HistoryParams {
    limit: Option<usize>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    App,
    Stack,
    Database,
}

pub fn router() -> axum::Router<crate::AppState> {
    axum::Router::new()
        .route("/api/autoscaler/rules", axum::routing::get(list_rules).post(create_rule))
        .route("/api/autoscaler/rules/{id}", axum::routing::get(get_rule).put(update_rule).delete(delete_rule))
        .route("/api/autoscaler/status", axum::routing::get(get_status))
        .route("/api/autoscaler/status/{id}", axum::routing::get(get_rule_status))
        .route("/api/autoscaler/history", axum::routing::get(get_history))
        .route("/api/autoscaler/config", axum::routing::get(get_config).axum::routing::put(update_config))
}