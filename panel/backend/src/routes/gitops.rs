// GitOps Fleet Sync Controller routes
// Provides endpoints for managing GitOps synchronization

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, internal_error, ApiError};
use crate::services::gitops_controller::{
    DesiredResource, GitOpsConfig, ReconciliationReport, ReconciliationResult,
};
use crate::AppState;
use sqlx;

/// Global GitOps controller instance
static GITOPS_CONTROLLER: std::sync::OnceLock<tokio::sync::Mutex<Option<crate::services::gitops_controller::GitOpsController>>> = std::sync::OnceLock::new();

/// Get or initialize the GitOps controller
async fn get_gitops_controller(state: &crate::AppState) -> Result<std::sync::Arc<tokio::sync::Mutex<Option<crate::services::gitops_controller::GitOpsController>>>, ApiError> {
    let controller_lock = GITOPS_CONTROLLER.get_or_init(|| tokio::sync::Mutex::new(None)).clone();
    let mut guard = controller_lock.lock().await;

    if guard.is_none() {
        // Load config from settings
        let settings: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM settings WHERE key IN (
                'gitops_enabled', 'gitops_git_repo_url', 'gitops_git_branch',
                'gitops_git_ssh_key_path', 'gitops_sync_interval_seconds',
                'gitops_dry_run', 'gitops_auto_reconcile', 'gitops_prune_orphaned'
            )"
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("gitops settings", e))?;

        let get = |k: &str| -> String {
            settings.iter()
                .find(|(sk, _)| sk == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };

        let enabled = get("gitops_enabled") == "true";
        let git_repo_url = get("gitops_git_repo_url");
        let git_branch = get("gitops_git_branch").unwrap_or_else(|| "main".to_string());
        let git_ssh_key_path = if get("gitops_git_ssh_key_path").is_empty() { None } else { Some(get("gitops_git_ssh_key_path")) };
        let sync_interval_seconds = get("gitops_sync_interval_seconds").parse::<u64>().unwrap_or(300);
        let dry_run = get("gitops_dry_run") == "true";
        let auto_reconcile = get("gitops_auto_reconcile") == "true";
        let prune_orphaned = get("gitops_prune_orphaned") == "true";

        if enabled && git_repo_url.is_empty() {
            return Err(err(StatusCode::SERVICE_UNAVAILABLE, "GitOps enabled but git_repo_url not configured"));
        }

        let config = GitOpsConfig {
            enabled,
            git_repo_url,
            git_branch,
            git_ssh_key_path,
            sync_interval_seconds,
            dry_run,
            auto_reconcile,
            prune_orphaned,
        };

        let controller = crate::services::gitops_controller::GitOpsController::new(config);
        *guard = Some(controller);
    }

    Ok(std::sync::Arc::new(tokio::sync::Mutex::new(guard.take())))
}

/// POST /api/gitops/reconcile — Trigger a manual reconciliation
pub async fn reconcile(
    State(state): State<crate::AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<ReconciliationReport>, ApiError> {
    let controller_lock = get_gitops_controller(&state).await?;
    let controller = controller_lock.lock().await;
    
    if controller.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "GitOps controller not configured"));
    }
    
    let controller = controller.as_ref().unwrap();
    let result = controller.reconcile(&state).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "gitops.reconcile",
        Some("gitops"), None, None, None, claims.key_id,
    ).await;

    Ok(Json(result))
}

/// GET /api/gitops/reconcile — Get last reconciliation report
pub async fn get_last_report(
    State(state): State<crate::AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<Option<ReconciliationReport>>, ApiError> {
    let controller_lock = get_gitops_controller(&state).await?;
    let controller = controller_lock.lock().await;
    
    if controller.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "GitOps controller not configured"));
    }
    
    let controller = controller.as_ref().unwrap();
    let report = controller.last_report.lock().await.clone();

    Ok(Json(report))
}

/// GET /api/gitops/status — Get GitOps controller status
pub async fn get_status(
    State(state): State<crate::AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let controller_lock = get_gitops_controller(&state).await?;
    let controller = controller_lock.lock().await;
    
    if controller.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "GitOps controller not configured"));
    }
    
    let controller = controller.as_ref().unwrap();
    let running = *controller.running.lock().await;
    let last_sync = *controller.last_sync.lock().await;
    let last_report = controller.last_report.lock().await.clone();

    Ok(Json(serde_json::json!({
        "enabled": controller.config.enabled,
        "running": running,
        "last_sync": last_sync,
        "last_report": last_report.map(|r| serde_json::json!({
            "timestamp": r.timestamp,
            "git_commit": r.git_commit,
            "total_resources": r.total_resources,
            "synced": r.synced,
            "drift_detected": r.drift_detected,
            "applied": r.applied,
            "failed": r.failed,
            "skipped": r.skipped,
        })),
        "config": {
            "git_repo_url": controller.config.git_repo_url,
            "git_branch": controller.config.git_branch,
            "sync_interval_seconds": controller.config.sync_interval_seconds,
            "dry_run": controller.config.dry_run,
            "auto_reconcile": controller.config.auto_reconcile,
            "prune_orphaned": controller.config.prune_orphaned,
        }
    })))
}

/// POST /api/gitops/preview — Preview reconciliation without applying (dry run)
pub async fn preview(
    State(state): State<crate::AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<ReconciliationReport>, ApiError> {
    let controller_lock = get_gitops_controller(&state).await?;
    let mut controller = controller_lock.lock().await;
    
    if controller.is_none() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "GitOps controller not configured"));
    }
    
    // Temporarily enable dry_run
    let mut controller = controller.take().unwrap();
    let original_dry_run = controller.config.dry_run;
    controller.config.dry_run = true;
    
    let result = controller.reconcile(&state).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    
    // Restore original config
    controller.config.dry_run = original_dry_run;
    *controller = Some(controller);

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "gitops.preview",
        Some("gitops"), None, None, None, claims.key_id,
    ).await;

    Ok(Json(result))
}

/// GET /api/gitops/config — Get GitOps configuration (admin only)
pub async fn get_config(
    State(state): State<crate::AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Only admins can view config
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin access required"));
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'gitops_%'"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("gitops config", e))?;

    let mut config = serde_json::Map::new();
    for (k, v) in rows {
        // Mask sensitive values
        if k.contains("ssh_key") || k.contains("secret") || k.contains("key") {
            config.insert(k, serde_json::Value::String("***REDACTED***".to_string()));
        } else {
            config.insert(k, serde_json::Value::String(v));
        }
    }

    Ok(Json(serde_json::Value::Object(config)))
}

/// PUT /api/gitops/config — Update GitOps configuration (admin only)
pub async fn update_config(
    State(state): State<crate::AppState>,
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
        if !key.starts_with("gitops_") {
            continue; // Only allow gitops_* settings
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
        .map_err(|e| internal_error("update gitops setting", e))?;

        crate::services::activity::log_activity(
            &state.db, claims.sub, &claims.email, "gitops.config_updated",
            Some("setting"), Some(key.clone()), None, ip.as_deref(), claims.key_id,
        ).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub fn router() -> axum::Router<crate::AppState> {
    axum::Router::new()
        .route("/api/gitops/reconcile", axum::routing::post(reconcile))
        .route("/api/gitops/reconcile", axum::routing::get(get_last_report))
        .route("/api/gitops/status", axum::routing::get(get_status))
        .route("/api/gitops/preview", axum::routing::post(preview))
        .route("/api/gitops/config", axum::routing::get(get_config).axum::routing::put(update_config))
}